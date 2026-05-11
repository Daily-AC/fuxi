"""手挑派蒙 ref：从 shard 1 派蒙 row 里 filter 短句。

Phase 1：默认（normal）= 含语气词的"日常活泼"语调，输出 paimon.wav。
Phase 3：按 emotion 关键词另存 paimon-{happy,surprise,worry,serious,sad}.wav。

每情绪挑第一个匹配候选（4-8s + 8-30 字），输出 wav + 同名 .txt（prompt_text）。
用 `python pick_paimon.py [emotion ...]` 选要拉的情绪；不传 = 全部 + normal。
"""
from __future__ import annotations
import glob
import io
import sys
from pathlib import Path
import pandas as pd
import soundfile as sf

OUT = Path.home() / ".fuxi" / "sovits-ref"
OUT.mkdir(parents=True, exist_ok=True)

# Phase 3 改：扫 huggingface cache 下所有已下载的 parquet shard（fetch_paimon_v2.py 已 download
# shard 0-6），合并后再 filter——单 shard 派蒙 row 仅 ~130 条，情绪关键词
# (happy/worry/sad) 很可能没命中，多 shard 合并大大提高命中率。
_PARQUET_GLOB = (
    "/home/e0-7/.cache/huggingface/hub/"
    "datasets--hanamizuki-ai--genshin-voice-v3.5-mandarin/"
    "snapshots/*/data/train-*.parquet"
)


def _load_paimon_df() -> pd.DataFrame:
    shards = sorted(glob.glob(_PARQUET_GLOB))
    if not shards:
        raise SystemExit(f"未找到 parquet shard，glob={_PARQUET_GLOB}")
    print(f"loading {len(shards)} parquet shard(s):")
    dfs = []
    for p in shards:
        print(f"  {p}")
        df = pd.read_parquet(p)
        dfs.append(df[df["npcName"] == "派蒙"])
    merged = pd.concat(dfs, ignore_index=True)
    return merged


# 情绪 filter 规则：每情绪一组 must-have 关键词（任一命中即算）。
# normal 沿用 Phase 1 老规则（语气词），其余按 Wikipedia 派蒙词库 / 实际原神台词归纳。
PARTICLES_NORMAL = ["嘛", "呀", "呢", "哎", "哦", "啦", "耶", "呐"]

EMOTION_RULES: dict[str, list[str]] = {
    "happy": [
        "太好了", "真厉害", "嘿嘿", "好棒", "厉害", "好耶", "哇",
        "哈哈", "嘿", "棒", "真好", "终于", "成功",
    ],
    "surprise": ["诶？", "诶！", "啊？", "啊！", "什么？", "怎么会", "居然", "竟然", "诶"],
    "worry": [
        "怎么办", "好担心", "好怕", "不安", "糟糕", "怎么这样", "不会吧",
        "不好", "麻烦", "完蛋", "怎么办呀", "怎么办呢",
    ],
    "serious": [],  # 由"无叹号无问号 + 平叙"规则筛，下面单独处理
    "sad": ["呜呜", "好伤心", "难过", "好可怜", "好痛", "好惨", "呜", "唉", "可惜"],
}

# 派蒙台词通常很短 + 第一人称 + 口语化。npcName 数据集偶尔标错（shard 1 serious 拉到的
# 「愤怒对这座工厂里的魔神怨念」是稻妻 boss 台词被错标为派蒙）。加 blacklist 关键词
# 兜底，命中即剔除——只针对 serious 的"无叹号"规则；其他 emotion 由关键词自筛已经足够。
SERIOUS_BLACKLIST = [
    "魔神", "天守阁", "将军", "雷电", "愚人众", "执行官", "稻妻", "国家",
    "敌人", "誓", "荣耀", "胜者", "怨念", "工厂", "诅咒", "永恒", "宿命",
    "蜕变", "仙祖", "法蜕",
]


def _row_to_clip(row, min_dur: float = 3.0, max_dur: float = 8.0, min_len: int = 6, max_len: int = 30):
    """row → (duration, text, audio_data, sr) 或 None。粗筛长度 + 音频时长。"""
    text = str(row["text"] or "").strip()
    if not (min_len <= len(text) <= max_len):
        return None
    if "[" in text or "{" in text:
        return None
    audio = row["audio"]
    b = audio.get("bytes") if isinstance(audio, dict) else None
    if not b:
        return None
    try:
        data, sr = sf.read(io.BytesIO(b))
    except Exception:
        return None
    duration = len(data) / sr
    if not (min_dur <= duration <= max_dur):
        return None
    return duration, text, data, sr


def pick_normal(paimon_df) -> None:
    """Phase 1 老规则：含语气词的活泼短句 → paimon.wav / paimon.txt。"""
    candidates = []
    for _, row in paimon_df.iterrows():
        clip = _row_to_clip(row, min_dur=4.0, max_dur=8.0, min_len=8, max_len=30)
        if not clip:
            continue
        _, text, _, _ = clip
        if not any(p in text for p in PARTICLES_NORMAL):
            continue
        candidates.append(clip)
    print(f"normal · 含语气词候选 {len(candidates)} 条")
    if not candidates:
        print("  无候选——试 shard 0 / 2")
        return
    for i, (d, t, _, _) in enumerate(candidates[:8]):
        print(f"  [{i}] {d:.2f}s · {t}")
    d, t, data, sr = candidates[0]
    sf.write(str(OUT / "paimon.wav"), data, sr)
    (OUT / "paimon.txt").write_text(t, encoding="utf-8")
    print(f"  saved {OUT/'paimon.wav'} · {d:.2f}s · {t}")


def _fallback_to_normal(emotion: str) -> bool:
    """缺 ref 时，cp normal paimon.wav 当占位——保持派蒙音色，视觉切由桌宠端
    HAPPY_SET / POOR_CONDITION_SET sprite 兜。比起完全无 ref 进退化路径，
    cp 出来后 tts_proxy.py 的 EMOTION_REFS 字典能加载到，emotion 路由不空转。"""
    src_wav = OUT / "paimon.wav"
    src_txt = OUT / "paimon.txt"
    if not src_wav.exists() or not src_txt.exists():
        return False
    dst_wav = OUT / f"paimon-{emotion}.wav"
    dst_txt = OUT / f"paimon-{emotion}.txt"
    dst_wav.write_bytes(src_wav.read_bytes())
    dst_txt.write_text(src_txt.read_text(encoding="utf-8"), encoding="utf-8")
    print(f"  fallback {emotion} = cp normal → {dst_wav}")
    return True


def pick_emotion(paimon_df, emotion: str) -> None:
    """按 emotion 关键词 filter，挑第一个最干净的当 ref。"""
    keywords = EMOTION_RULES.get(emotion, [])
    candidates = []
    for _, row in paimon_df.iterrows():
        clip = _row_to_clip(row)
        if not clip:
            continue
        _, text, _, _ = clip
        if emotion == "serious":
            # 平叙：无 ! 无 ? 无明显语气词。降级匹配；如果还是没有，下面会跳出
            if "！" in text or "？" in text or "!" in text or "?" in text:
                continue
            if any(p in text for p in PARTICLES_NORMAL):
                continue
            # npcName 误标兜底：黑名单关键词命中即剔（多为稻妻 boss / 旁白台词）
            if any(bad in text for bad in SERIOUS_BLACKLIST):
                continue
        else:
            if not any(k in text for k in keywords):
                continue
        candidates.append(clip)
    print(f"{emotion} · 候选 {len(candidates)} 条")
    if not candidates:
        print(f"  无候选——cp normal 当占位（音色保派蒙，视觉差异由桌宠 sprite 兜）")
        _fallback_to_normal(emotion)
        return
    for i, (d, t, _, _) in enumerate(candidates[:8]):
        print(f"  [{i}] {d:.2f}s · {t}")
    d, t, data, sr = candidates[0]
    wav_out = OUT / f"paimon-{emotion}.wav"
    txt_out = OUT / f"paimon-{emotion}.txt"
    sf.write(str(wav_out), data, sr)
    txt_out.write_text(t, encoding="utf-8")
    print(f"  saved {wav_out} · {d:.2f}s · {t}")


def main() -> None:
    args = sys.argv[1:]
    paimon = _load_paimon_df()
    print(f"merged 派蒙 rows: {len(paimon)}")
    if not args:
        # 默认：normal + 全部情绪
        pick_normal(paimon)
        for e in EMOTION_RULES.keys():
            pick_emotion(paimon, e)
        return
    for a in args:
        if a == "normal":
            pick_normal(paimon)
        elif a in EMOTION_RULES:
            pick_emotion(paimon, a)
        else:
            print(f"未知 emotion `{a}`，合法：normal/{'/'.join(EMOTION_RULES.keys())}")


if __name__ == "__main__":
    main()

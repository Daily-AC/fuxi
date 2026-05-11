"""手挑派蒙 ref：从 shard 1 派蒙 row 里 filter 短句。

Phase 1：默认（normal）= 含语气词的"日常活泼"语调，输出 paimon.wav。
Phase 3：按 emotion 关键词另存 paimon-{happy,surprise,worry,serious,sad}.wav。

每情绪挑第一个匹配候选（4-8s + 8-30 字），输出 wav + 同名 .txt（prompt_text）。
用 `python pick_paimon.py [emotion ...]` 选要拉的情绪；不传 = 全部 + normal。
"""
from __future__ import annotations
import io
import sys
from pathlib import Path
import pandas as pd
import soundfile as sf

OUT = Path.home() / ".fuxi" / "sovits-ref"
OUT.mkdir(parents=True, exist_ok=True)
PARQUET = "/home/e0-7/.cache/huggingface/hub/datasets--hanamizuki-ai--genshin-voice-v3.5-mandarin/snapshots/2f4853c15597e96e18386609869381cdccb92f07/data/train-00001-of-00067-85b85af193671aab.parquet"


# 情绪 filter 规则：每情绪一组 must-have 关键词（任一命中即算）。
# normal 沿用 Phase 1 老规则（语气词），其余按 Wikipedia 派蒙词库 / 实际原神台词归纳。
PARTICLES_NORMAL = ["嘛", "呀", "呢", "哎", "哦", "啦", "耶", "呐"]

EMOTION_RULES: dict[str, list[str]] = {
    "happy": ["太好了", "真厉害", "嘿嘿", "好棒", "厉害", "好耶", "哇"],
    "surprise": ["诶？", "啊？", "诶！", "啊！", "什么？", "怎么会", "居然"],
    "worry": ["怎么办", "好担心", "好怕", "不安", "糟糕", "怎么这样", "不会吧"],
    "serious": [],  # 由"无叹号无问号 + 平叙"规则筛，下面单独处理
    "sad": ["呜呜", "好伤心", "难过", "好可怜", "好痛", "好惨"],
}


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
        else:
            if not any(k in text for k in keywords):
                continue
        candidates.append(clip)
    print(f"{emotion} · 候选 {len(candidates)} 条")
    if not candidates:
        print(f"  无候选——跳过 {emotion}（可手工 cp 一条已有 wav 占位）")
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
    print(f"reading {PARQUET}")
    df = pd.read_parquet(PARQUET)
    paimon = df[df["npcName"] == "派蒙"]
    print(f"派蒙 rows: {len(paimon)}")
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

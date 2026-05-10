"""
v2 派蒙 ref 抓取 —— 直接下 parquet shard + pandas filter，比 datasets streaming 快多了。

策略：
1. HF API list parquet shards
2. 下第一个 shard（~几百 MB；HF mirror 速度 10-30 MB/s）
3. pandas read, filter speaker == 派蒙 + duration 4.5-9s + transcription clean
4. take 一个写 wav + txt
"""
from __future__ import annotations
import io
import os
import sys
from pathlib import Path

OUT_DIR = Path.home() / ".fuxi" / "sovits-ref"
OUT_DIR.mkdir(parents=True, exist_ok=True)
WAV = OUT_DIR / "paimon.wav"
TXT = OUT_DIR / "paimon.txt"

if WAV.exists() and TXT.exists():
    print(f"[skip] 已有 {WAV} ({WAV.stat().st_size}B) + {TXT}")
    print(TXT.read_text())
    sys.exit(0)

import pandas as pd
import soundfile as sf
from huggingface_hub import hf_hub_download

PAIMON_KEYS = {"派蒙", "Paimon", "PAIMON", "paimon"}
REPO = "hanamizuki-ai/genshin-voice-v3.5-mandarin"

# 顺序试 shard 0..6（派蒙是高频角色，前 7 个 shard 大概率命中）。
# 一旦命中 break。
chosen = None
for shard_idx in range(7):
    fname = None
    # API list 给我们文件名，但我们不知 hash 后缀；用 hf_hub_download 直接 by index pattern？
    # 简单：先 hf_hub_download 文件名带通配，但 HF 不支持 glob——必须精确名。
    # 用 list_repo_files 拿到完整名。
    from huggingface_hub import HfApi
    api = HfApi()
    files = api.list_repo_files(repo_id=REPO, repo_type="dataset")
    for f in files:
        if f.startswith(f"data/train-{shard_idx:05d}-of-"):
            fname = f
            break
    if not fname:
        continue
    print(f"[shard {shard_idx}] downloading {fname} ...")
    local = hf_hub_download(repo_id=REPO, filename=fname, repo_type="dataset")
    print(f"[shard {shard_idx}] read parquet {local} (size={os.path.getsize(local) / 1e6:.1f}MB)")

    df = pd.read_parquet(local)
    print(f"[shard {shard_idx}] columns: {list(df.columns)} rows={len(df)}")
    speaker_col = next((c for c in ("npcName", "speaker", "Speaker", "name", "character") if c in df.columns), None)
    text_col = next((c for c in ("transcription", "text", "raw_text") if c in df.columns), None)
    audio_col = next((c for c in ("audio", "Audio") if c in df.columns), None)
    print(f"[shard {shard_idx}] cols: speaker={speaker_col} text={text_col} audio={audio_col}")
    if not (speaker_col and text_col and audio_col):
        print(f"[shard {shard_idx}] schema 不全，跳")
        continue

    # 派蒙 row
    paimon = df[df[speaker_col].isin(PAIMON_KEYS)]
    print(f"[shard {shard_idx}] 派蒙 rows={len(paimon)}")
    if len(paimon) == 0:
        continue

    # 解一行 audio bytes 看 schema —— HF audio feature parquet 里通常是 dict {bytes, path}
    sample0 = paimon.iloc[0][audio_col]
    print(f"[shard {shard_idx}] audio sample type: {type(sample0).__name__}, keys: {sample0.keys() if hasattr(sample0, 'keys') else 'n/a'}")

    # filter 文本：6-30 字、无 [] 标记
    candidates = []
    for idx, row in paimon.iterrows():
        text = str(row[text_col] or "").strip()
        if not (6 <= len(text) <= 30):
            continue
        if "[" in text or "{" in text:
            continue
        audio = row[audio_col]
        if not isinstance(audio, dict):
            continue
        b = audio.get("bytes")
        if not b:
            continue
        # 用 soundfile 读看 duration
        try:
            data, sr = sf.read(io.BytesIO(b))
        except Exception as e:
            continue
        duration = len(data) / sr
        if not (4.0 <= duration <= 9.0):
            continue
        candidates.append({"duration": duration, "text": text, "data": data, "sr": sr})

    print(f"[shard {shard_idx}] candidates after filter: {len(candidates)}")
    if not candidates:
        continue

    # 评分：偏好短句（避口齿黏连）+ 6-7s 时长 + 无强情绪标点
    def score(c):
        s = abs(c["duration"] - 6.0)
        for ch in "！!？?…":
            s += c["text"].count(ch) * 1.5
        # 偏好简单陈述句，长度居中
        s += abs(len(c["text"]) - 14) * 0.1
        return s
    candidates.sort(key=score)
    print("[top 5]")
    for c in candidates[:5]:
        print(f"  {c['duration']:.2f}s · {c['text']}")
    chosen = candidates[0]
    break

if not chosen:
    print("[fatal] 7 个 shard 都没找到合适派蒙 sample")
    sys.exit(1)

print(f"\n[pick] {chosen['duration']:.2f}s · {chosen['text']}")
sf.write(str(WAV), chosen["data"], chosen["sr"])
TXT.write_text(chosen["text"], encoding="utf-8")
print(f"[saved] {WAV} ({WAV.stat().st_size}B)")
print(f"[saved] {TXT}")

"""手挑派蒙 ref：从 shard 1 派蒙 row 里 filter 含语气词短句（更像派蒙）。"""
from __future__ import annotations
import io
from pathlib import Path
import pandas as pd
import soundfile as sf

OUT = Path.home() / ".fuxi" / "sovits-ref"
OUT.mkdir(parents=True, exist_ok=True)
PARQUET = "/home/e0-7/.cache/huggingface/hub/datasets--hanamizuki-ai--genshin-voice-v3.5-mandarin/snapshots/2f4853c15597e96e18386609869381cdccb92f07/data/train-00001-of-00067-85b85af193671aab.parquet"

df = pd.read_parquet(PARQUET)
paimon = df[df["npcName"] == "派蒙"]

PARTICLES = ["嘛", "呀", "呢", "哎", "哦", "啦", "耶", "呐"]

candidates = []
for _, row in paimon.iterrows():
    text = str(row["text"] or "").strip()
    if not (8 <= len(text) <= 30):
        continue
    if "[" in text or "{" in text:
        continue
    if not any(p in text for p in PARTICLES):
        continue
    audio = row["audio"]
    b = audio.get("bytes") if isinstance(audio, dict) else None
    if not b:
        continue
    try:
        data, sr = sf.read(io.BytesIO(b))
    except Exception:
        continue
    duration = len(data) / sr
    if not (4.0 <= duration <= 8.0):
        continue
    candidates.append({"d": duration, "t": text, "data": data, "sr": sr})

print(f"派蒙含语气词短句候选 {len(candidates)} 条:")
for i, c in enumerate(candidates[:15]):
    d = c["d"]
    t = c["t"]
    print(f"  [{i}] {d:.2f}s · {t}")

if not candidates:
    raise SystemExit("无候选——试 shard 0 / 2 之后再来")

chosen = candidates[0]
sf.write(str(OUT / "paimon.wav"), chosen["data"], chosen["sr"])
(OUT / "paimon.txt").write_text(chosen["t"], encoding="utf-8")
print()
print(f"[saved] {chosen['d']:.2f}s · {chosen['t']}")
print(f"[wav] {OUT/'paimon.wav'}")

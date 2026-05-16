"""
Speaker Verification server —— 桌宠（jarvis-pet）的「这是不是以琳本人」识别。

为啥要这层：
- wake server 喊「玄女」会被任何人触发（同事 / 老婆孩子 / 视频里听到的人）。
- ASR 端 / wake 端各自调 /verify 拦一道——只有声纹匹配 owner 才放行。

模型：iic/speech_campplus_sv_zh-cn_16k-common（CAM++ 中文 SV，27MB，CPU/GPU 都跑得动；
same-speaker cos ≈ 0.6~0.8；不同说话人里「明显不同」≈0.0，但「同性同口音音色相近」
常落 0.3~0.5。默认阈值 0.5（见 sv_decision.py），把相近音色挡在外面。

接口（HTTP）：
- POST /enroll   body {"wav_b64": "..."}  → 提 192 维 embedding 存 owner.npy
- POST /verify   body {"wav_b64": "..."}  → {"match", "score", "threshold", "enrolled"}
- GET  /healthz  → 模型状态 + 是否已注册

鉴权：跟 sovits-proxy / asr_server 同款 HMAC token（`~/.fuxi/im_hmac.key`）。
fail-open：owner.npy 不存在时 /verify 永远 match=true——开机即用，注册后才严格。
"""
from __future__ import annotations
import asyncio
import base64
import hashlib
import hmac
import io
import json
import logging
import os
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional

import numpy as np
import soundfile as sf
import torch
import torch.nn.functional as F
from fastapi import FastAPI, Header, HTTPException, Request
from fastapi.responses import JSONResponse

# 判别逻辑（阈值 + 比对）抽到零依赖模块，便于在无 torch/funasr 的开发机上单测。
from sv_decision import DEFAULT_THRESHOLD, decide_match

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s")
log = logging.getLogger("sv_server")

HMAC_KEY_PATH = os.environ.get("SV_HMAC_KEY_PATH", os.path.expanduser("~/.fuxi/im_hmac.key"))
MODEL_ID = os.environ.get("SV_MODEL_ID", "iic/speech_campplus_sv_zh-cn_16k-common")
DEVICE = os.environ.get("SV_DEVICE", "cuda:0" if torch.cuda.is_available() else "cpu")
SAMPLE_RATE = 16000
# issue d22400a1：原 default 0.3 对同性同口音相近音色过松——CAM++ 对这类他人
# cos 常落 0.3~0.5（并非注释假设的 ≈0.0），全被放行。default 提到 0.5（见
# sv_decision.py docstring 的取值依据）；SV_THRESHOLD 仍可按部署机实测 FAR/FRR 覆盖。
THRESHOLD = float(os.environ.get("SV_THRESHOLD", str(DEFAULT_THRESHOLD)))
OWNER_PATH = Path(os.environ.get(
    "SV_OWNER_PATH",
    os.path.expanduser("~/.fuxi/voiceprint/owner.npy"),
))

with open(HMAC_KEY_PATH, "rb") as f:
    HMAC_SECRET = f.read().strip()

OWNER_PATH.parent.mkdir(parents=True, exist_ok=True)

app = FastAPI()

# 模型懒加载——首次请求时再加载（启动期 systemd 不超时）。CAM++ 27MB 在 cuda 上
# 加载 <1s，加上 modelscope 检查更新可能 5s——延后加载更稳。
_model = None
_model_lock = asyncio.Lock()


def _b64u_decode(s: str) -> bytes:
    pad = "=" * (-len(s) % 4)
    return base64.urlsafe_b64decode(s + pad)


def verify_token(token: str) -> dict:
    parts = token.split(".")
    if len(parts) != 2:
        raise ValueError("token 格式错（要 body.sig 两段）")
    body_b64, sig_b64 = parts
    body = _b64u_decode(body_b64)
    sig = _b64u_decode(sig_b64)
    expected = hmac.new(HMAC_SECRET, body, hashlib.sha256).digest()
    if not hmac.compare_digest(sig, expected):
        raise ValueError("签名不匹配")
    claims = json.loads(body)
    expires_at = claims.get("expires_at")
    if expires_at:
        try:
            exp = datetime.fromisoformat(expires_at.replace("Z", "+00:00"))
            if exp < datetime.now(timezone.utc):
                raise ValueError(f"token 已过期 ({expires_at})")
        except ValueError as e:
            raise ValueError(f"expires_at 解析失败: {e}")
    return claims


async def get_model():
    global _model
    async with _model_lock:
        if _model is None:
            log.info("loading SV model %s on %s ...", MODEL_ID, DEVICE)
            t0 = time.time()
            from funasr import AutoModel
            _model = AutoModel(model=MODEL_ID, device=DEVICE, disable_update=True)
            log.info("SV model ready in %.1fs", time.time() - t0)
    return _model


def _owner_embedding() -> Optional[np.ndarray]:
    """读 owner.npy → ndarray[192]，不存在返 None（fail-open 入口）。"""
    if not OWNER_PATH.exists():
        return None
    try:
        return np.load(OWNER_PATH)
    except Exception as e:
        log.warning("读 owner.npy 失败 %s（视为未注册）", e)
        return None


def _check_auth(authorization: Optional[str]) -> dict:
    if not authorization or not authorization.lower().startswith("bearer "):
        raise HTTPException(status_code=401, detail="missing Bearer token")
    token = authorization.split(None, 1)[1].strip()
    try:
        return verify_token(token)
    except ValueError as e:
        log.warning("token verify fail: %s", e)
        raise HTTPException(status_code=401, detail=f"unauthorized: {e}")


def _decode_wav(b64: str) -> tuple[np.ndarray, int]:
    """body wav_b64 → (float32 mono ndarray, sample_rate)。要求 16kHz，
    其他 sr 直接报 400（让客户端自己重采样，省服务端依赖）。"""
    if not b64:
        raise HTTPException(status_code=400, detail="wav_b64 required")
    try:
        # 直接 b64 / b64url 两可
        raw = base64.b64decode(b64, validate=False)
    except Exception as e:
        raise HTTPException(status_code=400, detail=f"wav_b64 解码失败: {e}")
    try:
        data, sr = sf.read(io.BytesIO(raw))
    except Exception as e:
        raise HTTPException(status_code=400, detail=f"wav 解码失败: {e}")
    if sr != SAMPLE_RATE:
        raise HTTPException(
            status_code=400,
            detail=f"sample_rate must be {SAMPLE_RATE}, got {sr}（客户端先 resample）",
        )
    if data.ndim > 1:
        data = data.mean(axis=1)  # to mono
    return data.astype(np.float32), sr


async def _extract_embedding(audio_f32: np.ndarray) -> torch.Tensor:
    """跑 CAM++ 抽 192 维 embedding。FunASR generate(output_emb=True) 在 cuda 上
    ~50ms 一次，足够 wake / asr 实时调用。"""
    model = await get_model()
    result = await asyncio.to_thread(
        model.generate,
        input=audio_f32,
        output_emb=True,
    )
    if not result or "spk_embedding" not in result[0]:
        raise HTTPException(status_code=500, detail="模型未返回 embedding（音频太短？）")
    emb = result[0]["spk_embedding"]  # torch.Tensor [1, 192] on cuda
    return emb.detach().cpu().squeeze(0)  # [192]


@app.get("/healthz")
async def healthz():
    enrolled = OWNER_PATH.exists()
    return JSONResponse({
        "ok": True,
        "model_loaded": _model is not None,
        "model_id": MODEL_ID,
        "device": DEVICE,
        "threshold": THRESHOLD,
        "enrolled": enrolled,
        "owner_path": str(OWNER_PATH),
    })


@app.post("/enroll")
async def enroll(req: Request, authorization: Optional[str] = Header(default=None)):
    claims = _check_auth(authorization)
    body = await req.json()
    audio, _ = _decode_wav(body.get("wav_b64") or "")
    if len(audio) / SAMPLE_RATE < 1.0:
        raise HTTPException(status_code=400, detail="音频太短，至少 1 秒（建议 5-30 秒说话）")
    emb = await _extract_embedding(audio)
    np.save(OWNER_PATH, emb.numpy())
    log.info("enroll OK by %s, dim=%d", claims.get("name", "?"), emb.shape[0])
    return {"enrolled": True, "dim": int(emb.shape[0]), "owner_path": str(OWNER_PATH)}


@app.post("/verify")
async def verify(req: Request, authorization: Optional[str] = Header(default=None)):
    claims = _check_auth(authorization)
    body = await req.json()
    audio, _ = _decode_wav(body.get("wav_b64") or "")
    if len(audio) / SAMPLE_RATE < 0.3:
        # 太短就跳过模型（CAM++ 在 <0.3s 不稳）；wake server 拿到的"玄女"瞬间
        # 通常 0.5-1s，足够。300ms 以下基本无效，直接判 false 但 fail-open 行为
        # 由 enrolled 控制：未注册时 match=true，已注册时 match=false（保守）。
        if _owner_embedding() is None:
            return {"match": True, "score": 0.0, "threshold": THRESHOLD,
                    "enrolled": False, "reason": "too_short_but_not_enrolled"}
        return {"match": False, "score": 0.0, "threshold": THRESHOLD,
                "enrolled": True, "reason": "audio_too_short"}

    owner = _owner_embedding()
    if owner is None:
        # fail-open：未注册时全放行——用户首次部署不用先 enroll 也能用
        return {"match": True, "score": 0.0, "threshold": THRESHOLD,
                "enrolled": False, "reason": "no_owner_enrolled"}

    emb = await _extract_embedding(audio)
    owner_t = torch.from_numpy(owner)
    score = F.cosine_similarity(owner_t.unsqueeze(0), emb.unsqueeze(0)).item()
    match = decide_match(score, THRESHOLD)
    log.info("verify by %s: score=%.3f match=%s", claims.get("name", "?"), score, match)
    return {"match": match, "score": score, "threshold": THRESHOLD, "enrolled": True}

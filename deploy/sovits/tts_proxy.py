"""
GPT-SoVITS thin proxy —— jarvis 客户端的简化入口。

为啥要这层：
- GPT-SoVITS api_v2.py 的 /tts 要传一堆参数（text_lang / ref_audio_path /
  prompt_lang / prompt_text / 各种 cut/seed/temperature 旋钮），客户端不
  应该知道 ref_audio 的 server-side 路径，也不该硬编 cut/seed 等模型
  细节。
- 我们这层只暴露 POST /tts {text} → wav，所有 ref + 模型旋钮 server 决定。
- 鉴权：复用 fuxi-im 同款 HMAC token（`~/.fuxi/im_hmac.key`），用户在 jarvis
  设置里填的 pair token 就能直接用，不需要为 TTS 单独再发一颗。

启动：
  uvicorn tts_proxy:app --host 127.0.0.1 --port 9881
环境变量：
  TTS_REF_PATH        ref audio 绝对路径（必填）
  TTS_REF_TEXT        ref audio 转写（必填）
  TTS_HMAC_KEY_PATH   fuxi-im HMAC key（默认 ~/.fuxi/im_hmac.key）
  SOVITS_BASE         sovits api base（默认 http://127.0.0.1:9880）
"""
from __future__ import annotations
import base64
import hashlib
import hmac
import json
import logging
import os
from datetime import datetime, timezone

import httpx
from fastapi import FastAPI, Header, HTTPException, Request
from fastapi.responses import Response

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s")
log = logging.getLogger("tts_proxy")

REF_PATH = os.environ["TTS_REF_PATH"]
REF_TEXT = os.environ["TTS_REF_TEXT"]
SOVITS_BASE = os.environ.get("SOVITS_BASE", "http://127.0.0.1:9880")
HMAC_KEY_PATH = os.environ.get("TTS_HMAC_KEY_PATH", os.path.expanduser("~/.fuxi/im_hmac.key"))

with open(HMAC_KEY_PATH, "rb") as f:
    HMAC_SECRET = f.read().strip()

app = FastAPI()


def _b64u_decode(s: str) -> bytes:
    pad = "=" * (-len(s) % 4)
    return base64.urlsafe_b64decode(s + pad)


def verify_token(token: str) -> dict:
    """跟 fuxi-im im-mint-token.py 同款 HMAC-SHA256 token 验签。"""
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


@app.get("/healthz")
async def healthz():
    return {"ok": True, "ref_path": REF_PATH, "sovits": SOVITS_BASE}


@app.post("/tts")
async def tts(req: Request, authorization: str | None = Header(default=None)):
    if not authorization or not authorization.lower().startswith("bearer "):
        raise HTTPException(status_code=401, detail="missing Bearer token")
    token = authorization.split(None, 1)[1].strip()
    try:
        claims = verify_token(token)
    except ValueError as e:
        log.warning("token verify fail: %s", e)
        raise HTTPException(status_code=401, detail=f"unauthorized: {e}")

    body = await req.json()
    text = (body.get("text") or "").strip()
    if not text:
        raise HTTPException(status_code=400, detail="text required")

    payload = {
        "text": text,
        "text_lang": "zh",
        "ref_audio_path": REF_PATH,
        "prompt_text": REF_TEXT,
        "prompt_lang": "zh",
        "media_type": "wav",
        "streaming_mode": False,
        "text_split_method": "cut5",
        "batch_size": 1,
        "speed_factor": 1.0,
        "top_k": 5,
        "top_p": 1.0,
        "temperature": 1.0,
    }
    log.info("tts device=%s text_len=%d", claims.get("name", "?"), len(text))
    async with httpx.AsyncClient(timeout=30.0) as client:
        try:
            resp = await client.post(f"{SOVITS_BASE}/tts", json=payload)
        except httpx.RequestError as e:
            log.error("sovits unreachable: %s", e)
            raise HTTPException(status_code=502, detail=f"sovits unreachable: {e}")

    if resp.status_code != 200:
        log.error("sovits %d body=%r", resp.status_code, resp.content[:200])
        raise HTTPException(status_code=resp.status_code, detail=resp.text)

    return Response(content=resp.content, media_type="audio/wav")

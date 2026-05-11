"""
FunASR Paraformer-zh ASR server —— 桌宠（jarvis-pet）的语音转写出口。

为啥要这层（vs. 客户端 WhisperKit）：
- 药丸 v0.2 在 macOS 跑 WhisperKit on-device 没问题；桌宠走 Tauri webview
  Vue/Rust，没法 bind Swift 框架，Web 侧又没有同级中文 ASR 模型。
- home 已经有 RTX 5090 + sovits 邻居住，加一个 GPU 推理服务最划算。
- 桌宠 webview 录 16kHz PCM 流上来，这边一次 transcribe 返 final 文本。

接口（WebSocket 协议）：
1. 客户端连 `ws://...:9882/asr`
2. 客户端发文本帧 `{"type":"start","token":"<bearer>","sample_rate":16000}`
3. 服务端验签 → 文本帧 `{"type":"ready"}`  或 close(4401)
4. 客户端连续发二进制帧：PCM 16-bit LE mono 16kHz，chunk 任意（建议 100-400ms）
5. 客户端发文本帧 `{"type":"end"}`
6. 服务端跑 batch transcribe（SenseVoiceSmall 不支持 streaming，整段一次出）→
   文本帧 `{"type":"final","text":"...","duration_ms":N}` → close

为啥不做 partial：SenseVoiceSmall 非 streaming；Phase 2 桌宠是 push-to-talk
不需要 partial。后续要 partial 换 `paraformer-large-streaming` 即可。

鉴权：跟 sovits-proxy 同款 HMAC token（`~/.fuxi/im_hmac.key`）。
"""
from __future__ import annotations
import asyncio
import base64
import hashlib
import hmac
import json
import logging
import os
import time
from datetime import datetime, timezone
from io import BytesIO

import numpy as np
import soundfile as sf
import torch
from fastapi import FastAPI, WebSocket, WebSocketDisconnect
from fastapi.responses import JSONResponse

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(name)s: %(message)s")
log = logging.getLogger("asr_server")

HMAC_KEY_PATH = os.environ.get("ASR_HMAC_KEY_PATH", os.path.expanduser("~/.fuxi/im_hmac.key"))
MODEL_ID = os.environ.get("ASR_MODEL_ID", "iic/SenseVoiceSmall")
DEVICE = os.environ.get("ASR_DEVICE", "cuda:0" if torch.cuda.is_available() else "cpu")
SAMPLE_RATE = 16000
MAX_AUDIO_SECONDS = int(os.environ.get("ASR_MAX_SECONDS", "60"))

with open(HMAC_KEY_PATH, "rb") as f:
    HMAC_SECRET = f.read().strip()

app = FastAPI()

# 模型懒加载——首次 WS 请求时加载，避免 systemd 启动超时（模型首次下载 ~400MB）
_model = None
_model_lock = asyncio.Lock()


def _b64u_decode(s: str) -> bytes:
    pad = "=" * (-len(s) % 4)
    return base64.urlsafe_b64decode(s + pad)


def verify_token(token: str) -> dict:
    """跟 fuxi-im im-mint-token.py / sovits-proxy 同款 HMAC-SHA256 token 验签。"""
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
            log.info("loading model %s on %s ...", MODEL_ID, DEVICE)
            t0 = time.time()
            # 在 loop 里 await 同步导入，避免阻塞——FunASR 加载有点慢
            from funasr import AutoModel
            _model = AutoModel(
                model=MODEL_ID,
                device=DEVICE,
                disable_update=True,
                # SenseVoiceSmall 内置 VAD/标点；不需要额外配 punc_model
            )
            log.info("model ready in %.1fs", time.time() - t0)
    return _model


@app.get("/healthz")
async def healthz():
    return JSONResponse({
        "ok": True,
        "model_loaded": _model is not None,
        "model_id": MODEL_ID,
        "device": DEVICE,
    })


@app.websocket("/asr")
async def asr_ws(ws: WebSocket):
    await ws.accept()
    log.info("ws %s connected", ws.client)
    try:
        # 第一帧必须是 start JSON
        first = await ws.receive_text()
        try:
            payload = json.loads(first)
        except json.JSONDecodeError:
            await ws.close(code=4400, reason="first frame must be json start")
            return
        if payload.get("type") != "start":
            await ws.close(code=4400, reason="expected type=start")
            return
        token = payload.get("token") or ""
        try:
            claims = verify_token(token)
        except ValueError as e:
            log.warning("auth fail: %s", e)
            await ws.close(code=4401, reason=f"auth: {e}")
            return
        sample_rate = int(payload.get("sample_rate") or SAMPLE_RATE)
        if sample_rate != SAMPLE_RATE:
            # 简化：要求客户端先重采样到 16kHz。后续可加 server-side resample。
            await ws.close(code=4400, reason=f"sample_rate must be {SAMPLE_RATE}")
            return

        await ws.send_text(json.dumps({"type": "ready"}))
        log.info("ws %s authed user=%s", ws.client, claims.get("name", "?"))

        # 收 PCM chunks 直到 end / disconnect
        pcm_buf = bytearray()
        max_bytes = SAMPLE_RATE * 2 * MAX_AUDIO_SECONDS  # int16 mono
        while True:
            msg = await ws.receive()
            if "bytes" in msg and msg["bytes"] is not None:
                pcm_buf.extend(msg["bytes"])
                if len(pcm_buf) > max_bytes:
                    await ws.send_text(json.dumps({"type": "error", "error": "audio too long"}))
                    await ws.close(code=4413, reason="audio too long")
                    return
            elif "text" in msg and msg["text"] is not None:
                try:
                    ctl = json.loads(msg["text"])
                except json.JSONDecodeError:
                    continue
                if ctl.get("type") == "end":
                    break
                if ctl.get("type") == "abort":
                    log.info("ws %s aborted by client", ws.client)
                    await ws.close(code=1000)
                    return
            else:
                # WebSocket close frame
                break

        if not pcm_buf:
            await ws.send_text(json.dumps({"type": "error", "error": "no audio received"}))
            await ws.close(code=4400, reason="no audio")
            return

        # int16 LE → float32 [-1, 1]
        audio_i16 = np.frombuffer(bytes(pcm_buf), dtype=np.int16)
        audio_f32 = audio_i16.astype(np.float32) / 32768.0
        duration_ms = int(len(audio_f32) / SAMPLE_RATE * 1000)
        log.info("ws %s transcribing %d ms", ws.client, duration_ms)

        model = await get_model()
        t0 = time.time()
        # FunASR API：generate(input=ndarray, **kwargs)
        result = await asyncio.to_thread(
            model.generate,
            input=audio_f32,
            cache={},
            language="auto",  # auto / zh / en / ja / ko / yue
            use_itn=True,     # 数字归一化
            batch_size_s=60,
        )
        elapsed_ms = int((time.time() - t0) * 1000)

        # result 形如 [{'key': '...', 'text': '<|zh|><|NEUTRAL|>...<|/zh|>正文', ...}]
        text = ""
        if isinstance(result, list) and result:
            raw = result[0].get("text", "") or ""
            text = _strip_sensevoice_tags(raw)
        log.info("ws %s done %d ms text_len=%d", ws.client, elapsed_ms, len(text))
        await ws.send_text(json.dumps({
            "type": "final",
            "text": text,
            "duration_ms": duration_ms,
            "elapsed_ms": elapsed_ms,
        }))
        await ws.close(code=1000)

    except WebSocketDisconnect:
        log.info("ws %s disconnected", ws.client)
    except Exception as e:
        log.exception("ws %s error: %s", ws.client, e)
        try:
            await ws.send_text(json.dumps({"type": "error", "error": str(e)}))
            await ws.close(code=1011)
        except Exception:
            pass


def _strip_sensevoice_tags(s: str) -> str:
    """SenseVoiceSmall 输出形如 `<|zh|><|NEUTRAL|><|Speech|><|woitn|>正文` —— 去 tags。"""
    import re
    return re.sub(r"<\|[^|]*\|>", "", s).strip()

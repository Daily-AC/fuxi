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
import re
import time
from datetime import datetime, timezone
from io import BytesIO
from pathlib import Path
from typing import Optional

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
# Phase 5 SV：transcribe 后 in-process 调声纹服务，不通过则返空 text → 桌宠端
# 看到空 transcript 不发 intervene（陌生人喊话被静默丢弃）。
# fail-open：sv_server 不通 / SV 服务挂掉时走原文（不因为旁路故障让桌宠瘫痪）。
SV_ENABLED = os.environ.get("ASR_SV_ENABLED", "1") not in ("0", "false", "")
SV_URL = os.environ.get("ASR_SV_URL", "http://127.0.0.1:9883")
# 热词文件——`fuxi xuannv hotword add/rm/list` CLI 操作的目标。每次 transcribe
# 前 mtime check 自动 reload，不需要 systemctl restart。SenseVoiceSmall 不支持
# 模型级 hotword，靠后处理正则替换；详见 _Hotwords.apply()。
HOTWORDS_PATH = Path(os.environ.get(
    "ASR_HOTWORDS_PATH",
    os.path.expanduser("~/.fuxi/asr-hotwords.json"),
))

with open(HMAC_KEY_PATH, "rb") as f:
    HMAC_SECRET = f.read().strip()

app = FastAPI()

# 模型懒加载——首次 WS 请求时加载，避免 systemd 启动超时（模型首次下载 ~400MB）
_model = None
_model_lock = asyncio.Lock()


class _Hotwords:
    """ASR 后处理热词替换。

    JSON 形态（用户/玄女通过 `fuxi xuannv hotword add` 写入；也可手写）：
        {
          "rules": [
            {"match": "克劳德[寇口扣][德的]?", "replace": "claude code",
             "comment": "..."},
            {"match": "麦克(?![布风斯尔])", "replace": "mac",
             "comment": "..."}
          ]
        }

    `match` 一律按 Python `re` 正则解释；字面字符串不含特殊字符时也安全。
    规则按数组顺序匹配；首个匹配的替换后**不再回头**——后续规则继续在新文本上跑。
    这避免 A 替换的结果触发 B 的二次替换（链式串改风险）。

    mtime check 让 transcribe 路径自动 reload，无需 systemctl restart。
    """

    def __init__(self, path: Path) -> None:
        self.path = path
        self._mtime_ns: int = -1
        self._compiled: list[tuple[re.Pattern[str], str, str]] = []
        self._lock = asyncio.Lock()

    async def _reload_if_changed(self) -> None:
        try:
            mtime = self.path.stat().st_mtime_ns
        except FileNotFoundError:
            if self._mtime_ns != 0:
                self._compiled = []
                self._mtime_ns = 0
                log.info("hotwords: 配置文件不存在 %s（无替换规则）", self.path)
            return
        if mtime == self._mtime_ns:
            return
        async with self._lock:
            # double-check inside lock：两条 transcribe 路径并发 reload 时只跑一次
            try:
                mtime = self.path.stat().st_mtime_ns
            except FileNotFoundError:
                self._compiled = []
                self._mtime_ns = 0
                return
            if mtime == self._mtime_ns:
                return
            try:
                with open(self.path, encoding="utf-8") as f:
                    data = json.load(f)
                rules = data.get("rules", [])
                compiled: list[tuple[re.Pattern[str], str, str]] = []
                for i, r in enumerate(rules):
                    pat = r.get("match")
                    repl = r.get("replace")
                    if not isinstance(pat, str) or not isinstance(repl, str):
                        log.warning("hotwords: rule #%d 缺 match/replace，跳过", i)
                        continue
                    try:
                        compiled.append((
                            re.compile(pat),
                            repl,
                            (r.get("comment") or "").strip(),
                        ))
                    except re.error as e:
                        log.warning("hotwords: rule #%d 正则无效 %r: %s", i, pat, e)
                        continue
                self._compiled = compiled
                self._mtime_ns = mtime
                log.info("hotwords: reloaded %d rules from %s", len(compiled), self.path)
            except (OSError, json.JSONDecodeError) as e:
                log.warning("hotwords: 读 %s 失败 %s（保留旧规则）", self.path, e)

    async def apply(self, text: str) -> str:
        await self._reload_if_changed()
        if not self._compiled or not text:
            return text
        out = text
        for pat, repl, comment in self._compiled:
            new = pat.sub(repl, out)
            if new != out:
                log.debug("hotword hit %r → %r%s",
                          pat.pattern, repl, f" ({comment})" if comment else "")
                out = new
        return out


_hotwords = _Hotwords(HOTWORDS_PATH)


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
    await _hotwords._reload_if_changed()
    return JSONResponse({
        "ok": True,
        "model_loaded": _model is not None,
        "model_id": MODEL_ID,
        "device": DEVICE,
        "hotwords_path": str(HOTWORDS_PATH),
        "hotwords_count": len(_hotwords._compiled),
        "sv_enabled": SV_ENABLED,
        "sv_url": SV_URL,
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
        # 热词后处理：模型出文本 → 正则替换。失败兜底走原文，不要因为 hotword
        # 配置写错就让整条 transcribe 报错。
        try:
            text = await _hotwords.apply(text)
        except Exception as e:
            log.warning("hotword apply 失败 %s（用原文）", e)

        # Phase 5 SV：transcribe 完后调 sv_server 比对声纹。non-match → 返空 text
        # 让桌宠端不发 intervene。fail-open：SV 服务不通 → 返原 text + warning。
        sv_score: float | None = None
        sv_enrolled = False
        if SV_ENABLED and text:  # 空 text 没必要再 verify
            try:
                sv = await _sv_verify(audio_i16, token)
                sv_score = sv.get("score")
                sv_enrolled = sv.get("enrolled", False)
                if sv_enrolled and not sv.get("match", True):
                    log.info(
                        "ws %s SV reject text='%s' score=%.3f → 返空 text",
                        ws.client, text[:30], sv_score or 0.0,
                    )
                    text = ""
            except Exception as e:
                log.warning("SV 调用失败 %s（fail-open 走原文）", e)

        log.info("ws %s done %d ms text_len=%d sv_score=%s enrolled=%s",
                 ws.client, elapsed_ms, len(text), sv_score, sv_enrolled)
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


async def _sv_verify(audio_i16: np.ndarray, token: str) -> dict:
    """把 PCM int16 转 wav bytes（in-memory）→ b64 → POST sv_server /verify。
    用 httpx async client 调 localhost；token 直接透传，sv_server 验同款 HMAC。
    返 sv_server 完整 response（含 match / score / threshold / enrolled）。
    """
    import httpx
    buf = io.BytesIO()
    sf.write(buf, audio_i16, SAMPLE_RATE, format="WAV", subtype="PCM_16")
    wav_b64 = base64.b64encode(buf.getvalue()).decode("ascii")
    async with httpx.AsyncClient(timeout=10.0) as client:
        resp = await client.post(
            f"{SV_URL}/verify",
            headers={"Authorization": f"Bearer {token}"},
            json={"wav_b64": wav_b64},
        )
    if resp.status_code != 200:
        raise RuntimeError(f"sv {resp.status_code}: {resp.text[:120]}")
    return resp.json()

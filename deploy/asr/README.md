# FunASR ASR 服务部署（home / RTX 5090）

桌宠（jarvis-pet v0.4 Phase 2）的语音转写出口。

## 为什么这层（vs. 客户端 STT）

- jarvis 药丸 v0.2 走客户端 WhisperKit on-device（Swift 框架）。
- 桌宠是 Tauri 2 webview，Vue/Rust 没法 bind WhisperKit。
- 服务端 ASR 用 GPU 推理更快、模型升级集中改服务端、客户端只 fetch 一个 endpoint。

## 接口

```
WSS https://im.qmledmq.cn:8443/api/asr
```

WebSocket 协议：

1. 客户端连上 → 发文本帧 `{"type":"start", "token":"<bearer>", "sample_rate":16000}`
2. 服务端验签（HMAC，复用 `~/.fuxi/im_hmac.key`）→ 文本帧 `{"type":"ready"}` 或 `close(4401)`
3. 客户端连续发 **二进制帧 PCM 16-bit LE mono 16kHz**（chunk 100–400ms 都行）
4. 客户端发文本帧 `{"type":"end"}`
5. 服务端跑模型 → 文本帧 `{"type":"final", "text":"...", "duration_ms":N, "elapsed_ms":M}` → close

中途 `{"type":"abort"}` 客户端主动取消。

> 不做 partial：SenseVoiceSmall 非 streaming 模型，整段一次出。
> Phase 2 桌宠场景是 push-to-talk，partial 用不上。要 partial 换
> `paraformer-large-streaming` 即可，asr_server.py 改 generate 模式。

## 模型

默认 `iic/SenseVoiceSmall`：
- 多语种（中/英/日/韩/粤）+ 情感识别 + 事件识别
- CER 中文测试集优于 Paraformer-large
- ~400MB，首次启动从 modelscope 拉到 `~/.cache/modelscope/hub/iic/SenseVoiceSmall`
- 显存 ~1.5GB（跟 sovits 5GB 共住 24GB RTX 5090 不挤）

切其它模型改 `ASR_MODEL_ID` env 即可。

## 安装

```bash
# 在 mac dev 机
scp -r deploy/asr home:~/fuxi-deploy-asr
env -u HTTPS_PROXY ssh home 'bash ~/fuxi-deploy-asr/install.sh'

# 合并 nginx snippet
env -u HTTPS_PROXY ssh home '
  sudo grep -q "/api/asr" /etc/nginx/sites-enabled/im \
    || sudo bash -c "cat ~/fuxi-deploy-asr/im-asr-snippet.conf >> /etc/nginx/sites-enabled/im"
  sudo nginx -t && sudo systemctl reload nginx
'

# smoke
ssh home 'curl -sf http://127.0.0.1:9882/healthz'
curl -sf https://im.qmledmq.cn:8443/api/asr  # WS endpoint，curl 会报 426 Upgrade Required 正常
```

## 维护

```bash
# 重启
sudo systemctl restart asr.service

# 日志
sudo journalctl -u asr.service -f --since "5 min ago"

# 模型升级（换 ASR_MODEL_ID）
echo 'ASR_MODEL_ID=iic/paraformer-large_asr_nat-zh-cn-16k-common' \
  | sudo tee /home/e0-7/.fuxi/asr-server.env
sudo systemctl restart asr.service
```

## 监控点

- `/healthz` 返 `model_loaded: true` 表示模型已加载（首次 WS 请求触发懒加载）
- `nvidia-smi` 看 funasr 进程占用应在 1-2GB
- journalctl 里 `done N ms text_len=...` 行是成功路径

## 鉴权 / 公网

跟 fuxi-im、sovits-proxy 同一颗 HMAC key（`~/.fuxi/im_hmac.key`，43 字节随机串）。
桌宠 webview 在 Tauri-side 通过 Tauri command 拿 token（避免暴露给 JS），WSS
start 帧带过来。token TTL 30 天，重发用 `ssh home python3 ~/.fuxi/im-mint-token.py`。

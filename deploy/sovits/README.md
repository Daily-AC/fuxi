# GPT-SoVITS · home 派蒙音色部署

把 home（RTX 5090 笔记本）变成 jarvis 的角色音色 TTS server。mac 客户端走
`https://im.qmledmq.cn:8443/api/tts` 拿 wav 播放，鉴权复用 fuxi-im pair token。

## 架构

```
mac jarvis
  └─ POST https://im.qmledmq.cn:8443/api/tts {text}
     + Authorization: Bearer <fuxi-im hmac token>
       │
       ▼ home nginx :8443 (location = /api/tts)
       │ proxy_pass http://127.0.0.1:9881/tts
       ▼ tts_proxy.py (FastAPI :9881)
         · HMAC 验签（同 fuxi-im im_hmac.key）
         · payload 转完整 sovits API params (ref_audio_path / prompt_text / cut5 / batch=1)
         ▼ POST http://127.0.0.1:9880/tts
           ▼ GPT-SoVITS api_v2.py (cuda RTX 5090，推理 ~0.3s)
             ▼ wav 252KB / 5s / 32kHz mono
       ◀ 流回 nginx
     ◀ 流回 mac
   ▼ AVAudioPlayer 播
```

## 关键路径

- **GPT-SoVITS 仓库**: `~/GPT-SoVITS/`
- **conda env**: `~/miniforge3/envs/GPTSoVITS/` (Python 3.11)
- **派蒙 ref audio**: `~/.fuxi/sovits-ref/paimon.wav` + `paimon.txt`
  来自 `hanamizuki-ai/genshin-voice-v3.5-mandarin` shard 1 npcName=派蒙 含语气词短句
  当前选中："我们再往前走一点嘛！在后面什么都看不清啊。" 4.01s
- **HMAC key**: `~/.fuxi/im_hmac.key` (复用 fuxi-im 同一颗签 token，**不**单独发)
- **sovits API**: 127.0.0.1:9880（systemd `sovits.service`）
- **tts proxy**: 127.0.0.1:9881（systemd `sovits-proxy.service`）
- **nginx 入口**: `https://im.qmledmq.cn:8443/api/tts`（`im` site `location = /api/tts`）

## 部署步骤（首次）

home 上：

```bash
# 1. 装 miniforge + conda env (python 3.11)
bash ~/miniforge3/bin/conda create -n GPTSoVITS python=3.11 -y

# 2. clone GPT-SoVITS + 装依赖
git clone --depth 1 https://github.com/RVC-Boss/GPT-SoVITS.git ~/GPT-SoVITS
cd ~/GPT-SoVITS
# install.sh 在 nohup 下 tput 会失败 → 用 WORKFLOW=true 跳过 tput
WORKFLOW=true bash install.sh --device CU128 --source HF-Mirror

# 3. 装 datasets + soundfile + httpx + torchcodec（fetch script 要用）
~/miniforge3/envs/GPTSoVITS/bin/python -m pip install datasets soundfile httpx torchcodec

# 4. 抓派蒙 ref audio（从 hanamizuki-ai/genshin-voice-v3.5-mandarin 第一 shard）
HF_ENDPOINT=https://hf-mirror.com python ~/fetch_paimon_v2.py
python ~/pick_paimon.py     # 二次精挑——含语气词的派蒙短句

# 5. 写 proxy env file
cat > ~/.fuxi/sovits-proxy.env << ENV
TTS_REF_PATH=/home/e0-7/.fuxi/sovits-ref/paimon.wav
TTS_REF_TEXT=$(cat ~/.fuxi/sovits-ref/paimon.txt)
TTS_HMAC_KEY_PATH=/home/e0-7/.fuxi/im_hmac.key
SOVITS_BASE=http://127.0.0.1:9880
ENV
chmod 600 ~/.fuxi/sovits-proxy.env

# 6. 装 systemd
sudo cp deploy/sovits/sovits.service /etc/systemd/system/
sudo cp deploy/sovits/sovits-proxy.service /etc/systemd/system/
cp deploy/sovits/tts_proxy.py ~/tts_proxy.py
sudo systemctl daemon-reload
sudo systemctl enable --now sovits.service
sleep 30      # sovits 加载 ckpt + cuda warmup ~25s
sudo systemctl enable --now sovits-proxy.service

# 7. nginx /api/tts location
# 把 im-tts-snippet.conf 内容插入 /etc/nginx/sites-enabled/im 的 location /wake/ 之前
sudo nginx -t && sudo systemctl reload nginx

# 8. mint 一颗 30 天 token 给 jarvis 用
python3 ~/.fuxi/im-mint-token.py
```

mac 上：jarvis 设置 → 连接 → Pair Token 粘贴上面 token；设置 → 语音 → 选「角色语音（远端）」。

## 换 ref audio（换不同句子 / 换不同角色）

1. 选新 wav：把任意干净 4-9s 角色音频丢到 `~/.fuxi/sovits-ref/<name>.wav`
2. 写转写到 `~/.fuxi/sovits-ref/<name>.txt`
3. 改 `~/.fuxi/sovits-proxy.env` 的 `TTS_REF_PATH` / `TTS_REF_TEXT`
4. `sudo systemctl restart sovits-proxy` 即生效（不用 restart sovits）

## 常用排障

- `sudo journalctl -u sovits -n 50 --no-pager` — sovits 日志（cuda OOM / 模型缺失）
- `sudo journalctl -u sovits-proxy -n 50 --no-pager` — proxy 日志（401/502 错误码）
- `curl http://127.0.0.1:9881/healthz` — proxy 在不在
- token 过期：`python3 ~/.fuxi/im-mint-token.py` 重 mint，jarvis 重新粘

### 已知坑

- **混中英文本走英文 POS tagger 时 sovits 返 400 `Resource 'averaged_perceptron_tagger_eng' not found`**：
  `install.sh` 下的是老 NLTK 命名 `averaged_perceptron_tagger`，NLTK 4+ 改名带
  `_eng` 后缀。运行时 lazy download 旧版仍然不命中。补装：
  ```bash
  source ~/miniforge3/etc/profile.d/conda.sh && conda activate GPTSoVITS
  python -m nltk.downloader averaged_perceptron_tagger_eng cmudict
  sudo systemctl restart sovits.service
  ```
  jarvis 客户端表现：第一次纯中文回话用派蒙音色 OK，后续含英文词的回话降级回
  系统 TTS（`remote tts http 400`）。

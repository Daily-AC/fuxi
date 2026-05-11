#!/usr/bin/env bash
# 在 home 机上跑这个脚本装 FunASR Paraformer-zh ASR 服务（端口 9882）。
# 用 conda env `funasr-asr`（跟 sovits 的 GPTSoVITS env 隔离，CUDA 11/12 兼容）。
#
# 用法：
#   scp -r deploy/asr home:~/fuxi-deploy-asr
#   ssh home 'bash ~/fuxi-deploy-asr/install.sh'
#
# 幂等：重跑会跳过已装的 pip 包 + 已存在的 service。

set -eEuo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_NAME="funasr-asr"
CONDA_BASE="/home/e0-7/miniforge3"
PY="$CONDA_BASE/envs/$ENV_NAME/bin/python"

echo "[1/6] 检查 conda env"
if [[ ! -d "$CONDA_BASE/envs/$ENV_NAME" ]]; then
    "$CONDA_BASE/bin/conda" create -y -n "$ENV_NAME" python=3.11
    # 没 pip？兜底
    "$PY" -m ensurepip --upgrade || true
fi

echo "[2/6] 装依赖（funasr / torch CUDA / fastapi / soundfile）"
# 国内 mirror + cuda 12.1 wheel（5090 必须 sm_120 CUDA 12.x+）
PIP="$PY -m pip"
$PIP install --upgrade pip setuptools wheel -i https://pypi.tuna.tsinghua.edu.cn/simple
# RTX 5090 (Blackwell) 是 sm_120，cu121/cu124 wheel 不支持 → 必须 cu130。
# cu130 wheel 在 pytorch 官方 index，国内直连慢但能跑。装完看 torch.cuda.get_device_capability
# 应该 (12, 0)。装错 cu 版会跑出 "no kernel image available" 错。
$PIP install \
    "torch>=2.11" "torchaudio>=2.11" \
    --index-url https://download.pytorch.org/whl/cu130
$PIP install \
    "funasr>=1.2" "fastapi>=0.115" "uvicorn[standard]>=0.30" \
    "soundfile" "numpy<2.0" "modelscope" \
    -i https://pypi.tuna.tsinghua.edu.cn/simple

echo "[3/6] 预下 SenseVoiceSmall 模型（~400MB，免首次冷启超时）"
# 首次跑会从 modelscope 下到 ~/.cache/modelscope/hub/iic/SenseVoiceSmall
"$PY" - <<'PY_EOF'
from funasr import AutoModel
m = AutoModel(model="iic/SenseVoiceSmall", device="cpu", disable_update=True)
print("model loaded, ready")
PY_EOF

echo "[4/6] 部署 asr_server.py 到 home 工作目录"
mkdir -p /home/e0-7/funasr-asr
cp "$HERE/asr_server.py" /home/e0-7/funasr-asr/
chmod 644 /home/e0-7/funasr-asr/asr_server.py
# systemd WorkingDirectory=/home/e0-7，asr_server 模块名按 `funasr-asr/asr_server` 显式
# 改 systemd 的 ExecStart 工作目录到具体子目录
sed -i 's|^WorkingDirectory=.*|WorkingDirectory=/home/e0-7/funasr-asr|' "$HERE/asr.service"

echo "[5/6] 装 systemd unit + 启动"
sudo cp "$HERE/asr.service" /etc/systemd/system/asr.service
sudo systemctl daemon-reload
sudo systemctl enable asr.service
sudo systemctl restart asr.service
sleep 3
sudo systemctl status --no-pager asr.service | head -15

echo "[6/6] 测健康"
curl -sf http://127.0.0.1:9882/healthz && echo
echo "DONE. 接下来要把 im-asr-snippet.conf 合并进 /etc/nginx/sites-enabled/im 再 reload nginx。"

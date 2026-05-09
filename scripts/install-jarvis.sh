#!/usr/bin/env bash
#
# install-jarvis.sh —— 一键装贾维斯到 mac /Applications/。
#
# 做的事：
#   1. xcodegen 生成 Xcode 工程 + xcodebuild Release
#   2. 把 Jarvis.app cp 到 /Applications/，ad-hoc codesign（按 fuxi 项目惯例）
#   3. ssh home 取 wake.token 写进 mac Keychain
#   4. 从 macOS 剪贴板读 fuxi-im pair token（你在 PWA 设置生成→复制即可）
#      也支持 --pair-token <tok> / 或 stdin 模式
#   5. open /Applications/Jarvis.app
#
# 跑完就用——首次启动答应三个系统权限（麦克风 / 语音识别 / 辅助功能）即可。
#
# 用法：
#   bash scripts/install-jarvis.sh                 # 从剪贴板取 pair token
#   bash scripts/install-jarvis.sh --pair-token TOK
#   bash scripts/install-jarvis.sh --no-pair       # 暂不配 fuxi-im pair（只装 wake）

set -euo pipefail

# ── 0. 入参解析 ─────────────────────────────
PAIR_TOKEN=""
SKIP_PAIR=0
while [[ $# -gt 0 ]]; do
    case "$1" in
        --pair-token) PAIR_TOKEN="$2"; shift 2 ;;
        --no-pair) SKIP_PAIR=1; shift ;;
        -h|--help) sed -n '1,30p' "$0"; exit 0 ;;
        *) echo "未知参数: $1" >&2; exit 2 ;;
    esac
done

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
JARVIS_DIR="$REPO_ROOT/apps/jarvis"

# ── 1. 工具检查 ─────────────────────────────
need() { command -v "$1" >/dev/null 2>&1 || { echo "缺工具: $1"; exit 3; }; }
need xcodebuild
need codesign
need security
need ssh

# xcodegen 不装就 brew 装一发；brew 没装就提示用户
if ! command -v xcodegen >/dev/null 2>&1; then
    if command -v brew >/dev/null 2>&1; then
        echo "==> 装 xcodegen"
        brew install xcodegen
    else
        cat <<'TIP' >&2
缺 xcodegen 且没 brew。装一个：
    /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
    brew install xcodegen
然后重跑本脚本。
TIP
        exit 3
    fi
fi

# ── 2. xcodegen + build ────────────────────
echo "==> [1/6] 生成 Xcode 工程"
cd "$JARVIS_DIR"
xcodegen generate

echo "==> [2/6] xcodebuild Release"
xcodebuild \
    -project Jarvis.xcodeproj \
    -scheme Jarvis \
    -configuration Release \
    -destination "platform=macOS" \
    -derivedDataPath build/derived \
    CODE_SIGN_IDENTITY="-" \
    CODE_SIGN_STYLE=Automatic \
    DEVELOPMENT_TEAM="" \
    -quiet \
    clean build

APP_PATH="$(find "$JARVIS_DIR/build/derived/Build/Products/Release" -maxdepth 2 -name 'Jarvis.app' -type d | head -1)"
if [[ -z "$APP_PATH" ]]; then
    echo "找不到 build 出的 Jarvis.app" >&2
    exit 4
fi

# ── 3. 安装到 /Applications/ ────────────────
echo "==> [3/6] 装到 /Applications/Jarvis.app"
sudo rm -rf /Applications/Jarvis.app
sudo cp -R "$APP_PATH" /Applications/Jarvis.app
# 重新 ad-hoc 签——cp 后签名会被破坏（按 fuxi feedback_macos_gatekeeper_codesign）
sudo codesign --force --deep --sign - /Applications/Jarvis.app

# ── 4. 取 home wake.token 写 Keychain ───────
echo "==> [4/6] 取 home wake.token 写 mac Keychain"
WAKE_TOKEN="$(ssh home 'cat ~/.fuxi/wake.token' 2>/dev/null | tr -d '\n')"
if [[ -z "$WAKE_TOKEN" ]]; then
    echo "ssh home 取 wake.token 失败——home 还没装 fuxi-wake-server？" >&2
    exit 5
fi
SERVICE="cn.qmledmq.fuxi.jarvis"
security delete-generic-password -s "$SERVICE" -a wakeToken 2>/dev/null || true
security add-generic-password -s "$SERVICE" -a wakeToken -w "$WAKE_TOKEN" -U
echo "    wakeToken 长度 ${#WAKE_TOKEN} → Keychain"

# ── 5. fuxi-im pair token ───────────────────
if [[ "$SKIP_PAIR" -eq 1 ]]; then
    echo "==> [5/6] 跳过 fuxi-im pair（--no-pair）"
else
    if [[ -z "$PAIR_TOKEN" ]]; then
        # 尝试剪贴板
        if command -v pbpaste >/dev/null 2>&1; then
            CLIP="$(pbpaste 2>/dev/null | tr -d '[:space:]')"
            # token 一般是 32+ 字符 hex/base64ish；剪贴板里如果是这种就用
            if [[ "$CLIP" =~ ^[A-Za-z0-9_+/=-]{20,}$ ]]; then
                PAIR_TOKEN="$CLIP"
                echo "    从剪贴板拿到 pair token (长度 ${#PAIR_TOKEN})"
            fi
        fi
    fi
    if [[ -z "$PAIR_TOKEN" ]]; then
        cat <<'TIP'
==> [5/6] 需要 fuxi-im pair token
    去 PWA (https://im.qmledmq.cn:8443) → 设置 → 配对设备 → 生成 token → 复制
    然后在这里粘贴（粘完按回车）：
TIP
        read -r PAIR_TOKEN
    fi
    if [[ -z "$PAIR_TOKEN" ]]; then
        echo "pair token 空——后续设置面板里手动填也行" >&2
    else
        security delete-generic-password -s "$SERVICE" -a pairToken 2>/dev/null || true
        security add-generic-password -s "$SERVICE" -a pairToken -w "$PAIR_TOKEN" -U
        echo "    pairToken 长度 ${#PAIR_TOKEN} → Keychain"
    fi
fi

# ── 6. 启动 ─────────────────────────────────
echo "==> [6/6] open /Applications/Jarvis.app"
open /Applications/Jarvis.app

cat <<'EOF'

──────────────────────────────────────────────
  装好了 ✓
  首次启动会弹三次权限请求——都点允许：
    1) 麦克风（必给）
    2) 语音识别（必给）
    3) 辅助功能（用全局热键时给——系统设置→隐私与安全性→辅助功能→Jarvis）
  之后菜单栏 mic 图标点开，或说「玄女」唤醒。
──────────────────────────────────────────────
EOF

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
# CommandLineTools 自带 swift / codesign / security——不强制装 Xcode。
need() { command -v "$1" >/dev/null 2>&1 || { echo "缺工具: $1"; exit 3; }; }
need swift
need codesign
need security
need ssh
need plutil

# ── 2. swift build ─────────────────────────
echo "==> [1/6] swift build -c release"
cd "$JARVIS_DIR"
swift build -c release 2>&1 | tail -3

BIN="$JARVIS_DIR/.build/release/Xuannv"
[[ -x "$BIN" ]] || { echo "找不到 build 出的 Xuannv binary"; exit 4; }

# ── 3. 组 .app bundle + 装 ~/Applications/ ─
# .app 目录名保留 ASCII (Xuannv.app) 避免 LS/Finder 路径中文歧义；用户可见名「玄女」
# 通过 CFBundleDisplayName 控制。
echo "==> [2/6] 组 .app bundle"
STAGE="$JARVIS_DIR/.build/release/Xuannv.app"
rm -rf "$STAGE"
mkdir -p "$STAGE/Contents/MacOS" "$STAGE/Contents/Resources"
cp "$BIN" "$STAGE/Contents/MacOS/Xuannv"
chmod +x "$STAGE/Contents/MacOS/Xuannv"

# .icns 注入——install.sh 跑前 Resources/AppIcon.icns 必须就位（gpt-image-2 生成 +
# iconutil 打包，详见 commit 历史）。没有就算了不挂，App 会用系统默认。
if [[ -f "$JARVIS_DIR/Resources/AppIcon.icns" ]]; then
    cp "$JARVIS_DIR/Resources/AppIcon.icns" "$STAGE/Contents/Resources/AppIcon.icns"
    ICON_KV='<key>CFBundleIconFile</key><string>AppIcon</string>'
else
    ICON_KV=''
fi

# Info.plist——SwiftPM 不注入，手写一份完整的（值都硬编，不留 $(VAR) 占位）。
# CFBundleDisplayName=玄女（用户可见名），CFBundleName=Jarvis（系统内部名/搜索词保留英文）。
cat > "$STAGE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>zh-Hans</string>
    <key>CFBundleDisplayName</key><string>玄女</string>
    <key>CFBundleExecutable</key><string>Xuannv</string>
    <key>CFBundleIdentifier</key><string>cn.qmledmq.fuxi.xuannv</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>玄女</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>LSMinimumSystemVersion</key><string>14.0</string>
    <key>LSUIElement</key><true/>
    ${ICON_KV}
    <key>NSAppleEventsUsageDescription</key><string>全局热键触发需要 AppleEvents 权限。</string>
    <key>NSMicrophoneUsageDescription</key><string>玄女需要麦克风来听你的呼唤和指令。</string>
    <key>NSSpeechRecognitionUsageDescription</key><string>玄女把你的语音转写成文字派给伏羲后端——全程本机处理，不上云。</string>
</dict>
</plist>
PLIST
plutil -lint "$STAGE/Contents/Info.plist" >/dev/null

# 装到 ~/Applications/——用户级目录无需 sudo，Launchpad / Spotlight 同样可达。
# 想全局 /Applications/ 自己 sudo cp 一份即可。
INSTALL_DIR="$HOME/Applications"
INSTALL_PATH="$INSTALL_DIR/Xuannv.app"
echo "==> [3/6] 装到 $INSTALL_PATH"
mkdir -p "$INSTALL_DIR"
# 顺手清掉历史 Jarvis.app（同 bundle id 但 v0.1 用过，名字残留在系统索引里）
rm -rf "$INSTALL_DIR/Jarvis.app" "/Applications/Jarvis.app" 2>/dev/null || true
rm -rf "$INSTALL_PATH"
cp -R "$STAGE" "$INSTALL_PATH"
codesign --force --deep \
    --entitlements "$JARVIS_DIR/Resources/Jarvis.entitlements" \
    --sign - "$INSTALL_PATH"

# 清 macOS TCC 数据库里旧 bundle id 的权限残留——不清的话用户在系统设置里看到老名字
# "Jarvis"，且新 binary 用同名权限项可能不被认。tccutil reset 让用户首次启动时
# 重新弹权限对话框（接受即写新条目）。
tccutil reset Microphone cn.qmledmq.fuxi.jarvis 2>/dev/null || true
tccutil reset SpeechRecognition cn.qmledmq.fuxi.jarvis 2>/dev/null || true
tccutil reset Accessibility cn.qmledmq.fuxi.jarvis 2>/dev/null || true
# Launch Services 重注册——强刷 Display Name 缓存
LSREG=/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister
"$LSREG" -u "$INSTALL_DIR/Jarvis.app" 2>/dev/null || true
"$LSREG" -f "$INSTALL_PATH"

# ── 4. 取 home wake.token 写 UserDefaults ───
echo "==> [4/6] 取 home wake.token 写 UserDefaults"
WAKE_TOKEN="$(ssh home 'cat ~/.fuxi/wake.token' 2>/dev/null | tr -d '\n')"
if [[ -z "$WAKE_TOKEN" ]]; then
    echo "ssh home 取 wake.token 失败——home 还没装 fuxi-wake-server？" >&2
    exit 5
fi
DOMAIN="cn.qmledmq.fuxi.xuannv"
defaults write "$DOMAIN" wakeToken -string "$WAKE_TOKEN"
echo "    wakeToken 长度 ${#WAKE_TOKEN} → UserDefaults($DOMAIN)"

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
        defaults write "$DOMAIN" pairToken -string "$PAIR_TOKEN"
        echo "    pairToken 长度 ${#PAIR_TOKEN} → UserDefaults($DOMAIN)"
    fi
fi

# ── 6. 启动 ─────────────────────────────────
echo "==> [6/6] open $INSTALL_PATH"
open "$INSTALL_PATH"

cat <<'EOF'

──────────────────────────────────────────────
  装好了 ✓
  首次启动会弹三次权限请求——都点允许：
    1) 麦克风（必给）
    2) 语音识别（必给）
    3) 辅助功能（用全局热键时给——系统设置→隐私与安全性→辅助功能→玄女）
  - 菜单栏：圆形 waveform 图标（深色波形）→ 点开看状态/设置
  - 默认热键：⌃⌥M（Control+Option+M，避开 Spotlight ⌘Space 冲突）
  - 唤醒词：「玄女」（远端讯飞 SDK + 本地 SFSpeech 兜底）
  - 唤醒后屏幕底部弹 Siri 风格悬浮窗 + 实时波形动画
──────────────────────────────────────────────
EOF

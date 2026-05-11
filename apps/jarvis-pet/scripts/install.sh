#!/usr/bin/env bash
# 一键 build release + 装 ~/Applications/XuannvPet.app + 注入 mic 权限 + 重签
#
# Tauri 2 bundler 不直接支持自定义 Info.plist keys（schema 没暴露 infoPlist
# 字段），用 plutil 后处理塞 NSMicrophoneUsageDescription——必须在 codesign
# 前改，否则签名校验会因 Info.plist 改动失败。
#
# 用法：bash apps/jarvis-pet/scripts/install.sh

set -euo pipefail

cd "$(dirname "$0")/.."  # 跳到 apps/jarvis-pet/

TARGET="$HOME/Applications/XuannvPet.app"
SOURCE="src-tauri/target/release/bundle/macos/Xuannv.app"

echo "[1/5] build release"
npm run tauri build 2>&1 | tail -3

echo "[2/5] kill running instance"
pkill -f "Applications/XuannvPet|MacOS/jarvis-pet" 2>/dev/null || true
sleep 1

echo "[3/5] cp 到 $TARGET"
rm -rf "$TARGET"
cp -R "$SOURCE" "$TARGET"

echo "[4/5] 注入 macOS 权限 keys（mic + LSUIElement）"
plutil -insert NSMicrophoneUsageDescription \
    -string "玄女桌宠需要麦克风权限以听你说话——语音通过 home 服务转写后发给玄女门客。" \
    "$TARGET/Contents/Info.plist"
plutil -replace LSUIElement -bool true "$TARGET/Contents/Info.plist" 2>/dev/null \
  || plutil -insert LSUIElement -bool true "$TARGET/Contents/Info.plist"

echo "[5/5] codesign（plutil 改完必重签，否则 macOS 拒）"
codesign --force --deep --sign - "$TARGET"
codesign --verify --verbose=2 "$TARGET" 2>&1 | tail -2

echo "DONE: $TARGET"
echo "open ~/Applications/XuannvPet.app"

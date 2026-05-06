#!/usr/bin/env bash
# scripts/deploy-home.sh —— 一键把 fuxi 部署到 home（systemd 跑 fuxi-im）。
#
# 在**本机** mac 上跑：
#   ./scripts/deploy-home.sh           # 全量：rsync + cargo + pnpm + 重启 + smoke
#   ./scripts/deploy-home.sh --no-pwa  # 只重 build 后端（PWA 没改时省 30s）
#   ./scripts/deploy-home.sh --no-rsync# 跳过同步代码（home 已最新时）
#
# 流程：
#   1. rsync 源码 → home:/home/e0-7/fuxi
#   2. ssh home: cargo build --release -p fuxi-cli
#   3. ssh home: pnpm install + build PWA + rsync dist → ~/.local/share/fuxi/im-web
#   4. ssh home: systemctl stop fuxi-im
#   5. ssh home: cp 二进制到 ~/.local/bin/fuxi + ~/.cargo/bin/fuxi
#   6. ssh home: fuxi xuannv refresh        ← 关键！清旧 session record，
#                                              否则下次 spawn 用 --resume <stale> 立即 cc exit
#   7. ssh home: systemctl start fuxi-im     → 玄女 fresh spawn 加载新 ROLE.md
#   8. ssh home: ./scripts/smoke-p0.sh       → 健康检查
#
# 错在第几步会 exit；smoke fail 退出码非零。
#
# 前置：~/.ssh/config 有 `Host home`；home 装好 fuxi + corepack + 主密码已设。

set -euo pipefail

DO_RSYNC=1
DO_PWA=1
LOCAL_ROOT="${LOCAL_ROOT:-/Users/e0_7/fuxi}"
REMOTE_ROOT="/home/e0-7/fuxi"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-rsync) DO_RSYNC=0; shift ;;
    --no-pwa)   DO_PWA=0; shift ;;
    -h|--help)  sed -n '2,30p' "$0"; exit 0 ;;
    *) echo "未知参数: $1"; exit 2 ;;
  esac
done

echo "== 1. rsync src → home =="
if [[ "$DO_RSYNC" == "1" ]]; then
  # bug #77 · 把 git sha 写到 .fuxi-build-sha 一起 rsync，让 home 端 build.rs
  # 拿到 deploy 时刻的 sha（home 不是 git repo，build.rs 否则 fallback "unknown"）
  git -C "$LOCAL_ROOT" rev-parse --short HEAD > "$LOCAL_ROOT/.fuxi-build-sha"
  rsync -az --delete \
    --exclude=target --exclude=node_modules --exclude=.git \
    --exclude=.playwright-mcp --exclude=.claude \
    "$LOCAL_ROOT/" "home:$REMOTE_ROOT/"
else
  echo "  (跳过)"
fi

echo
echo "== 2. cargo build (release, fuxi-cli) =="
ssh home "\$HOME/.cargo/bin/cargo build --release -p fuxi-cli --manifest-path $REMOTE_ROOT/Cargo.toml 2>&1 | tail -3"

if [[ "$DO_PWA" == "1" ]]; then
  echo
  echo "== 3. pnpm install + build PWA =="
  cat <<'PWA' | ssh home 'bash -s'
set -eo pipefail
cd /home/e0-7/fuxi/crates/fuxi-im/web
# CI=true 让 pnpm 跳过 modules 删除确认（无 TTY 下默认会 abort）
export CI=true
corepack pnpm install --prefer-offline 2>&1 | tail -3
corepack pnpm build 2>&1 | tail -5
mkdir -p $HOME/.local/share/fuxi/im-web
rsync -a --delete dist/ $HOME/.local/share/fuxi/im-web/
PWA
fi

echo
echo "== 4-7. stop / cp binary / refresh xuannv / start =="
cat <<'DEPLOY' | ssh home 'bash -s'
set -eo pipefail
sudo systemctl stop fuxi-im
cp /home/e0-7/fuxi/target/release/fuxi $HOME/.cargo/bin/fuxi
cp /home/e0-7/fuxi/target/release/fuxi $HOME/.local/bin/fuxi
echo "  binary updated:"
ls -la $HOME/.cargo/bin/fuxi $HOME/.local/bin/fuxi | awk '{print "    " $0}'
echo
echo "  ── fuxi xuannv refresh（关键：清旧 session_id 防 cc resume 失败）──"
$HOME/.cargo/bin/fuxi xuannv refresh 2>&1 | head -3
echo
echo "  ── systemctl start ──"
sudo systemctl start fuxi-im
sleep 4
sudo systemctl is-active fuxi-im
DEPLOY

echo
echo "== 8. smoke =="
ssh home "bash $REMOTE_ROOT/scripts/smoke-p0.sh"

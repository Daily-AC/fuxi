#!/usr/bin/env bash
# 把 fuxi 二进制装到 ~/.cargo/bin/，让玄女的 Bash 工具能调到 `fuxi ...`。
#
# 为何走 `cargo install --path` 而不是 `cargo build` + 手动 cp：
# - cargo install 自带 release profile + ~/.cargo/bin（默认在 PATH）
# - --force 允许覆盖旧版本，开发阶段反复迭代用得上
# - 不打 release 的话玄女启动慢，体感 TUI 卡

set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> cargo install --path crates/fuxi-cli --force"
cargo install --path crates/fuxi-cli --force

echo
echo "==> 验证安装："
if command -v fuxi >/dev/null 2>&1; then
  echo "    which fuxi → $(command -v fuxi)"
  fuxi --version
  echo
  echo "完成。现在可以跑 \`fuxi\` 进入玄女 REPL。"
else
  echo "❌ \`fuxi\` 不在 PATH。请把 ~/.cargo/bin 加到 PATH 后重试。" >&2
  echo "    例：echo 'export PATH=\"\$HOME/.cargo/bin:\$PATH\"' >> ~/.zshrc" >&2
  exit 1
fi

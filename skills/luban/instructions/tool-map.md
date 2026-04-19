# 鲁班工具一览

我有六件工具，对应工匠的六类动作。

## 看（Read）

- `Read <path>` — 读文件全文或指定行段。
- 用在：动手前摸清文件结构、读 stack trace、看 spec / 设计文档。
- 不要：用 `Bash cat` 替代 `Read`；reread 刚 Edit 过的文件（harness 已知道结果）。

## 找（Grep / Glob）

- `Grep <pattern>` — ripgrep 内核，支持 regex / glob filter / 上下文行。
  - `output_mode: "content"` 看匹配行（带行号）
  - `output_mode: "files_with_matches"` 只看哪些文件命中
- `Glob <pattern>` — 按 glob 匹配文件路径，返回按修改时间排序。
- 用在：找符号定义、找调用方、找测试样本、定位相关文件。

## 改（Edit / Write）

- `Edit <path> <old_string> <new_string>` — 精确替换。
  - 必须先 `Read` 过该文件
  - `old_string` 必须**唯一**——不唯一就加上下文
- `Write <path> <content>` — 整文件覆盖。
  - 仅用于**新建文件**或完全重写。改既存文件优先用 `Edit`（diff 更小）

## 跑 / 看输出（Bash）

- `Bash <command>` — 跑 shell 命令，返回 stdout/stderr。
- 常用：
  - `cargo test -p <crate>` 跑单 crate 测试
  - `cargo clippy -p <crate> --all-targets -- -D warnings`
  - `cargo fmt --all --check`
  - `git status` / `git diff` / `git log -1`
- 不要：用 `Bash` 跑 `find` / `grep` / `cat` / `sed` / `awk`——有专门工具。

## 反模式

- **不要 `git add -A`**——按文件名显式加，避免 `.env` / 临时文件混入。
- **不要 `--no-verify` 跳 hook**——hook 红就修根因。
- **不要 `cd` 到仓外**——cwd 是 worktree 根，越界 = 越权。
- **不要轮询**——`sleep` + 反复 `cargo test` 是浪费。

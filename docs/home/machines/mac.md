# mac

> 开发主力 + agent 控制端。Claude Code 在这里跑。

## 硬件 / OS
- M-series MacBook Pro
- macOS Darwin 25.0.0

## 路径
- fuxi repo：`/Users/e0_7/fuxi`（symlink `/Users/e0_7/xihe` → 同位置，老路径兼容）
- ssh config：`~/.ssh/config`（含 `winhome` + `winhome-pub` 别名）
- 公钥：`~/.ssh/id_ed25519.pub`（已加到 winhome `administrators_authorized_keys`）
- Claude job tmp：`/Users/e0_7/.claude/jobs/<job-id>/tmp/`

## 网络
- ISP 出口 IP 动态（FlClash/Clash TUN 走代理时 source IP 不同）
- VPN：Clash/Surge TUN（与 winhome 上的 FlClash 同源问题域，但 mac 这边 bypass 配置由用户管）
- mixed proxy：`127.0.0.1:7897`（与 winhome 上 :7890 不同）

## 装的工具（开发链）
- Rust toolchain (rustup) + `x86_64-pc-windows-gnu` target + mingw-w64（mac 交叉编译 Windows binary 用）
- gh CLI（自装，已 auth）
- glab CLI（公司 GitLab）
- brew 一堆
- OpenSSH 10.0.0 client
- python3 / node / 等

## 与 winhome 的关系
- 是 winhome 的**远控 client**（ssh winhome-pub）
- 是 wanctl 的 client end（兜底通道）
- 编译跨平台 binary（home-qm Windows release）
- Claude Code agent 控制 home 资产基本走 ssh winhome-pub 而非 wanctl（wanctl 仅作 fallback，痛点见 [wanctl-agent.md](../services/wanctl-agent.md)）

## quirks
- ⚠️ shell 设 `HTTPS_PROXY` 时干扰 ssh ProxyCommand DOH 查询。任何 ssh 命令包 `env -u HTTPS_PROXY -u https_proxy ssh ...`
- ⚠️ Bash tool 子 shell `unset HTTPS_PROXY` 不持久（每次新 shell 从父 env 重新注入）
- 详 [../../home-runbook.md](../../home-runbook.md) #9/#16

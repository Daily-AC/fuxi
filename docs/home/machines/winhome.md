# winhome (Legion Y9000P)

> 家里公网入口部署机。Win11 native + WSL2 Ubuntu 24.04 混合架构。

## 硬件 / OS
- Legion Y9000P 笔电
- Windows 11 Pro 25H2
- 内存：用户 .wslconfig 给 WSL 配 48GB
- WSL2 Ubuntu 24.04（mirrored networking + firewall + hostAddressLoopback 已开）

## 网络
- LAN IP：`192.168.1.19`（家里光猫 NAT 后）
- 公网 IP：`124.126.5.223`（家宽动态，[ddns-go](../services/ddns-go.md) 维护，几天/几个月不变）
- 入站：
  - `:8443`（HTTPS）→ LAN `:8443`（Caddy）
  - `:20777`（SSH）→ LAN `:22`（sshd）
  - `:3389`（RDP）当前公网开放（救命用，安全风险待决策，见 quirk #12）

## 跑的服务
全部见 [../services/INDEX.md](../services/INDEX.md)。winhome 这边：
- [caddy](../services/caddy.md) — 公网 HTTPS 反代
- [sshd](../services/sshd.md) — 公网 SSH
- [ddns-go](../services/ddns-go.md) — IP 同步
- [caddy-cert-sync](../services/caddy-cert-sync.md) — cert 同步任务
- [wanctl-agent](../services/wanctl-agent.md) — 远控兜底
- [flclash](../services/flclash.md) — 用户日常 VPN
- [home-qm](../services/home-qm.md) — 子域名 CLI（按需）

WSL Ubuntu 24.04 里：
- [acme-sh](../services/acme-sh.md) — cert 续签

## 用户 / 凭据
- 用户名：`yilin zhang`（含空格，OpenSSH 10.0+ 支持 `User "yilin zhang"`）
- Administrators 组成员
- 默认 shell：pwsh 7.5

## SSH 别名（mac `~/.ssh/config`）
```ssh
Host winhome           # LAN 直连（mac 在家时）
    HostName 192.168.1.19
    User "yilin zhang"
    Port 22
    IdentityFile ~/.ssh/id_ed25519

Host winhome-pub       # 公网（mac 出门时）
    HostName home.qmledmq.cn
    User "yilin zhang"
    Port 20777
    IdentityFile ~/.ssh/id_ed25519
```

## 关键路径
| 路径 | 用途 |
|---|---|
| `C:\Caddy\` | Caddy + ddns-go + qm + cert-sync 脚本聚集地 |
| `C:\ProgramData\ssh\` | sshd_config + administrators_authorized_keys |
| `C:\ProgramData\qm\` | qm domains.yaml registry |
| `C:\Users\Yilin Zhang\AppData\Roaming\com.follow\clash\` | FlClash config + profiles |
| `\\wsl$\Ubuntu-24.04\etc\nginx\ssl\` | acme.sh cert 落地（source） |
| `\\wsl$\Ubuntu-24.04\root\.acme.sh\` | acme.sh state |

## .wslconfig
```ini
[wsl2]
memory=48GB
swap=8GB
nestedVirtualization=true
networkingMode=mirrored      # WSL 网卡和 Windows host mirror，localhost 互通
firewall=true                # WSL 走 Windows Firewall
dnsTunneling=true
autoProxy=true

[experimental]
hostAddressLoopback=true     # localhost loopback 跨 WSL/Windows
sparseVhd=true
```

## quirks
全部见 [../../home-runbook.md](../../home-runbook.md) §「已知 limits / quirks」16 条。winhome 相关重灾区：sshd / Caddy / FlClash / WSL networking。

## 变更历史
- 2026-06-29：从老 Linux 重装为 Win11 + WSL2
- 2026-06-30：装齐 Caddy / sshd / ddns-go / acme.sh / cert-sync / qm 等
- 详细 timeline：[../../home-runbook.md](../../home-runbook.md)

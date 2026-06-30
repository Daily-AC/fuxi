# 端口分配

> 避免新服务监听冲突时查这里。winhome 当前 listen 一览。

## winhome native

| 端口 | 进程 | 用途 |
|---|---|---|
| 22 | sshd | OpenSSH Server LAN |
| 80 | caddy | （listen 但 Caddyfile 无 site，备而不用） |
| 135 | svchost | RPC，Windows 默认 |
| 139 | System | NetBIOS |
| 445 | System | SMB |
| 2019 | caddy | admin API（127.0.0.1 only） |
| 5040 | svchost | Windows Update / WSD |
| 5357 | System | WSDAPI HTTP |
| 7890 | FlClashCore | mixed proxy（HTTP + SOCKS） |
| 8443 | caddy | **公网 HTTPS 入口** |
| 9876 | ddns-go | web UI（127.0.0.1 only） |
| 27901 | GameViewerServer | 网易游戏，localhost only |
| 47890 | FlClashHelperService | localhost only |
| 42050 | OneDrive.Sync.Service | localhost only |
| 49664-49688, 50418 | lsass / wininit / svchost / spoolsv / services | Windows RPC ephemeral |

## WSL Ubuntu 24.04

| 端口 | 进程 | 用途 |
|---|---|---|
| _动态_ | _按需起_ | mirrored mode 下 listen `0.0.0.0:<port>` 等同 Windows localhost listen |

## 外部入站（路由器 NAT 转发到 winhome）

| 公网 | 内 | 用途 |
|---|---|---|
| 8443/tcp | 192.168.1.19:8443 | Caddy |
| 20777/tcp | 192.168.1.19:22 | sshd |
| 3389/tcp | 192.168.1.19:3389 | RDP（救命） |

## 预留约定

- **不要占用** Windows ephemeral 49152-65535（除非显式 listen）
- 新服务挑端口推荐范围：**10000-19999**（避免与系统服务、FlClash :7890 / Caddy :8443 / Caddy admin :2019 / ddns-go :9876 / GameViewer :27901 等冲突）

## 引用
- [services/INDEX.md](../services/INDEX.md)
- [refs/network-topology.md](network-topology.md)

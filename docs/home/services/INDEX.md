# services 索引

| 服务 | 机器 | 用途 |
|---|---|---|
| [caddy](caddy.md) | winhome | 公网 HTTPS 入口反代（:8443） |
| [sshd](sshd.md) | winhome | SSH 远程入口（公网 :20777 → LAN :22） |
| [ddns-go](ddns-go.md) | winhome | 动态公网 IP 同步到 CF DNS |
| [caddy-cert-sync](caddy-cert-sync.md) | winhome | scheduled task，每 6h 从 WSL 拉 acme cert 同步给 Caddy |
| [wanctl-agent](wanctl-agent.md) | winhome | wanctl relay agent（agent 远控兜底） |
| [home-qm](home-qm.md) | winhome | 子域名管理 CLI（写 Caddyfile + reload） |
| [acme-sh](acme-sh.md) | WSL Ubuntu 24.04 | wildcard cert 续签（cron） |
| [flclash](flclash.md) | winhome | VPN/代理（用户日常） |

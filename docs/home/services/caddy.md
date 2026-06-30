# caddy

> winhome 上的反向代理，公网 :8443 入口，路由到 Windows / WSL 各服务。

## 部署位置
- 机器：winhome
- Binary：`C:\Caddy\caddy.exe`（v2.11.4，含 `caddy-dns/cloudflare` 插件，53MB）
- Service manager：NSSM Windows Service `Caddy`（AutoStart + 失败自动重启 5s）
- Working dir：`C:\Caddy\`

## 端口 / 地址
- listen：`:80`（HTTP，ISP/中间盒拦截不可用，仍 listen）、`:8443`（HTTPS 主入口）、`127.0.0.1:2019`（admin API）
- 外部访问：`https://<sub>.qmledmq.cn:8443`（路由器 NAT 公网 :8443 → 192.168.1.19:8443）

## 配置
- 主：`C:\Caddy\Caddyfile`
- 子（auto-gen by [home-qm](home-qm.md) `qm sync`）：`C:\Caddy\Caddyfile.qm`
- 主 Caddyfile 末尾 `import C:/Caddy/Caddyfile.qm` 接入 qm 生成片段
- 证书：`C:\Caddy\certs\qmledmq.cn.{crt,key}`（acme.sh 生成，cert-sync 任务从 WSL 拉过来）
- Caddy state：`C:\Caddy\data\`（含 ACME account fallback，留着）
- 全局 directives：`auto_https disable_certs`（强制不走 ACME，用静态 cert，详 quirk #1）

## 依赖
- 上游：[caddy-cert-sync](caddy-cert-sync.md) 提供 cert，[acme-sh](acme-sh.md) 续签
- 凭据：见 [refs/secrets-locations.md](../refs/secrets-locations.md)（`CLOUDFLARE_API_TOKEN` 通过 NSSM `AppEnvironmentExtra` 注入，DNS-01 challenge 用）
- 系统依赖：Windows Service / NSSM / .NET（NSSM 用）

## 启动 / 停止 / 重启
```pwsh
# 状态
Get-Service Caddy

# 重启（NSSM service）
Restart-Service Caddy

# 热加载配置（不重启 service）
& "C:\Caddy\caddy.exe" reload --config C:\Caddy\Caddyfile --address localhost:2019

# 校验配置语法
& "C:\Caddy\caddy.exe" validate --config C:\Caddy\Caddyfile
```

## 健康检查
```bash
# 公网验
curl -sI https://home.qmledmq.cn:8443/ --resolve home.qmledmq.cn:8443:124.126.5.223 -k
# 期待：HTTP/2 200, Server: Caddy
```

## 日志
- stderr：`C:\Caddy\caddy.service.stderr.log`（10MB rotate）
- 关键 grep：`tls handshake|certificate|reverse_proxy|error`

## 变更历史
- 2026-06-30: ship NSSM service + acme wildcard cert 路径 + caddy-cert-sync 任务（commit `cb20def`）
- 2026-06-30: 主 Caddyfile 加 `import C:/Caddy/Caddyfile.qm` 接入 home-qm 生成片段（commit `0fe2817`）

## 已知问题 / 坑
- ⚠️ **Caddy auto-ACME 在 winhome 跑不通**：FlClash TUN + CF anycast NS + 1.1.1.1 DNS cache stale 导致 lego polling 失败。**走静态 cert + WSL acme.sh sync 路径**（详 [home-runbook.md#quirk-1](../../home-runbook.md)）。
- ⚠️ Caddyfile import 子文件后 `caddy reload` 输出会带 `Caddyfile input is not formatted; run 'caddy fmt --overwrite'` warning。不致命，是 indentation 风格警告。
- ⚠️ Windows TCP stack 看不到 :8443 listening（mirrored mode 跨 WSL/Windows）。`Get-NetTCPConnection -LocalPort 8443` 可能为空，但实际能命中。

## 引用
- [home-qm](home-qm.md) 写 Caddyfile 片段 + 触发 reload
- [caddy-cert-sync](caddy-cert-sync.md) 提供 cert
- [machines/winhome.md](../machines/winhome.md)
- quirk 列表：[home-runbook.md](../../home-runbook.md)

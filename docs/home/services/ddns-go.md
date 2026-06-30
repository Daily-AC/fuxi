# ddns-go

> 探测公网 IP 变化 + 自动 PUT 到 Cloudflare DNS（`home.qmledmq.cn` + `qmledmq.cn` 两条 A）。

## 部署位置
- 机器：winhome
- Binary：`C:\Caddy\ddns-go\ddns-go.exe`
- Service manager：Windows Service `ddns-go`（AutoStart）
- Working dir：`C:\Caddy\ddns-go\`

## 端口 / 地址
- listen：`127.0.0.1:9876`（web UI，`NotAllowWanAccess: true` 仅本机）
- 外部访问：_n/a_（仅 localhost）

## 配置
- 主：`C:\Caddy\ddns-go\config.yaml`
- 关键字段：
  - `Ipv4.URL`：`http://members.3322.org/dyndns/getip,https://myip.ipip.net`（**国内端点**，绕开 FlClash 拦截境外端点）
  - `Ipv4.Domains`：`home.qmledmq.cn` + `qmledmq.cn`
  - `DNS.Name`：`cloudflare`
  - `DNS.Secret`：CF API token（**当前明文**，见 [refs/secrets-locations.md](../refs/secrets-locations.md)）

## 依赖
- 出网 → `members.3322.org` + `myip.ipip.net` 拿真 ISP IP
- CF API（`Zone:DNS:Edit` for qmledmq.cn）
- 凭据：见 [refs/secrets-locations.md](../refs/secrets-locations.md)

## 启动 / 停止 / 重启
```pwsh
Get-Service ddns-go
Restart-Service ddns-go

# install/uninstall
& "C:\Caddy\ddns-go\ddns-go.exe" -s install -f 300 -c "C:\Caddy\ddns-go\config.yaml"
& "C:\Caddy\ddns-go\ddns-go.exe" -s uninstall
```

## 健康检查
```pwsh
# IP 实际更新到 CF（modified_on 应该是最近一次 IP 变化时间）
curl -H "Authorization: Bearer <TOKEN>" "https://api.cloudflare.com/client/v4/zones/<ZONE>/dns_records?name=home.qmledmq.cn" | jq '.result[0] | {content, modified_on}'

# IP 没变时 ddns-go 不写 → modified_on 不变 = 正常
```

## 日志
- `C:\Caddy\ddns-go\console.log` / `daemon.stderr.log` / `daemon.stdout.log`（IP 未变时空）

## 变更历史
- 2026-06-29：第一次装但 ipify/icanhazip/ipinfo 走 FlClash 代理拿到 Azure 出口 IP（错值），service 卸掉避免乱写
- 2026-06-30 16:05：改 URL 为国内端点 + 重装 service。实测国内端点走 FlClash TUN 仍返真 ISP IP 124.126.5.223 ✓（commit `33ba31e`）

## 已知问题 / 坑
- ⚠️ **FlClash TUN 拦截境外 IP 检测端点**（ipify / cloudflare / ipinfo），返代理出口 IP。**用国内端点绕**。详 [home-runbook.md#quirk-2](../../home-runbook.md)。
- ⚠️ **CF API token 在 config.yaml 明文**：归 [project_secrets_cli_plan_2026_06_29](#) 整治范围，未来迁出。
- ❌ 死路：4.ipw.cn / cloudflare.com / api.ip.sb / ifconfig.co 都试过，要么 timeout 要么返代理 IP。

## 引用
- [refs/secrets-locations.md](../refs/secrets-locations.md)
- [machines/winhome.md](../machines/winhome.md)
- quirk #2：[home-runbook.md](../../home-runbook.md)

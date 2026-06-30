# flclash

> 用户日常 VPN/代理（Clash Meta 的 Flutter 版）。winhome 上 TUN 模式重度使用。

## 部署位置
- 机器：winhome
- Binary：`C:\Program Files\FlClash\FlClash.exe` + `FlClashCore.exe` + `FlClashHelperService.exe`
- Service manager：用户登录态启动（不是 Windows Service）
- Working dir：install dir

## 端口 / 地址
- listen：`:7890`（mixed proxy：HTTP + SOCKS）、`:47890`（FlClashHelperService）
- TUN：fake-IP 池 `198.18.0.0/15`（CGNAT 段）
- 默认 WinHTTP proxy 配为 `127.0.0.1:7890`

## 配置
- 主配置：`C:\Users\Yilin Zhang\AppData\Roaming\com.follow\clash\config.yaml`（528KB，含 rules）
- profile：`com.follow\clash\profiles\<profile_id>.yaml`（订阅同步）
- rules 走 group 路由（如 `cloudflare.com` → `节点 Cloudflare` proxy group，不直连）

## 依赖
- 远程订阅源（用户私有）
- TUN driver（Windows Filtering Platform）

## 启动 / 停止 / 重启
- 用户托盘图标右键
- 或重启 winhome 后用户登录时自启

## 健康检查
```pwsh
# 进程在
Get-Process | Where ProcessName -Match "lash"

# 代理端口能用
curl -x http://127.0.0.1:7890 https://www.google.com -o NUL -w "%{http_code}\n"
```

## 日志
- 通过 GUI 内置 logs panel
- 或 `%LOCALAPPDATA%\com.follow\` 下日志文件

## 变更历史
- 2026-06-30：winhome 重装后默认 config 丢失 ipify DIRECT bypass rules → ddns-go 拿不到真 IP（详 quirk #2）。解决方案：改 ddns-go 用国内端点（不动 FlClash）。

## 已知问题 / 坑
- ⚠️ **重度用户必开**，不能整个停（用户硬约束）
- ⚠️ TUN 模式拦截**境外 HTTPS IP 检测端点**（ipify / cloudflare / icanhazip / ipinfo）→ 返代理出口 IP，不是真 ISP IP。详 quirk #2。
- ⚠️ fake-IP 错误信息会**掩盖远端真实 reset**（看到 198.18.0.x 端口报错未必 TUN 的锅，可能远端 reset 被 Clash 包装）。详 quirk #10。
- ⚠️ 改 FlClash rules 添加 DIRECT bypass 是可行的，但用户更倾向**不动 FlClash 配置**，下游服务自适应（如 ddns-go 用国内端点）。

## 引用
- quirks #2/#10：[home-runbook.md](../../home-runbook.md)
- [ddns-go](ddns-go.md)（典型受影响下游）

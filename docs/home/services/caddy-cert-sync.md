# caddy-cert-sync

> Scheduled Task：从 WSL Ubuntu 24.04 拉 acme.sh 续签后的 cert，同步到 `C:\Caddy\certs\`，触发 Caddy reload。

## 部署位置
- 机器：winhome
- Binary：`C:\Caddy\sync-acme-cert.ps1`（PowerShell 脚本）
- Service manager：Windows Scheduled Task `caddy-cert-sync`
- Working dir：`C:\Caddy\`

## 端口 / 地址
- _n/a_

## 配置
- 触发：每 6h + 系统启动时
- LogonType：**S4U as `yilin zhang`**（Windows SYSTEM 用户不能跑 WSL，详 quirks）
- 注册脚本：`C:\Caddy\register-cert-sync-task.ps1`

## 依赖
- [acme-sh](acme-sh.md)（WSL 里跑，cron 自动续签）
- [caddy](caddy.md)（拉到新 cert 后 `caddy reload`）
- 源路径：WSL `/etc/nginx/ssl/qmledmq.cn.{crt,key}`
- 目标路径：`C:\Caddy\certs\qmledmq.cn.{crt,key}`

## 启动 / 停止 / 重启
```pwsh
# 手动触发立即跑
schtasks /Run /TN caddy-cert-sync

# 看 task 状态 / 上次跑结果
schtasks /Query /TN caddy-cert-sync /V /FO LIST

# 注销 / 重新注册
schtasks /Delete /TN caddy-cert-sync /F
& C:\Caddy\register-cert-sync-task.ps1
```

## 健康检查
```pwsh
# 比 WSL 端 mtime 和 Windows 端是否一致（同步成功）
$wsl = (wsl -d Ubuntu-24.04 -u root -- stat -c '%Y' /etc/nginx/ssl/qmledmq.cn.crt)
$win = (Get-Item C:\Caddy\certs\qmledmq.cn.crt).LastWriteTimeUtc.ToString('o')
"WSL=$wsl  WIN=$win"

# 看 cert SAN（实测）
openssl x509 -in C:\Caddy\certs\qmledmq.cn.crt -noout -subject -dates -ext subjectAltName
```

## 日志
- `C:\Caddy\cert-sync.log`

## 变更历史
- 2026-06-30 凌晨：ship Scheduled Task + sync-acme-cert.ps1（commit ssh winhome 凌晨那批）

## 已知问题 / 坑
- ⚠️ **SYSTEM 用户不能跑 WSL**（`WSL_E_LOCAL_SYSTEM_NOT_SUPPORTED`）。必须 LogonType=S4U + UserId="yilin zhang"。
- ⚠️ pwsh 7 不在 SYSTEM PATH（user-scope 装）。Task action 用 `powershell.exe`（PS 5.1）或 pwsh 7 绝对路径。

## 引用
- [acme-sh](acme-sh.md)
- [caddy](caddy.md)
- quirks #4/#5：[home-runbook.md](../../home-runbook.md)

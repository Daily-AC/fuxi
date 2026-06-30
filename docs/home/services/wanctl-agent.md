# wanctl-agent

> wanctl relay agent。agent ssh 不通时的**远控兜底通道**（agent 通过 wanctl_exec 跑命令 / push/pull 文件）。

## 部署位置
- 机器：winhome
- Binary：`C:\Users\Yilin Zhang\AppData\Local\wanctl\`（user-scope 装）
- Service manager：Windows Scheduled Task `WanctlAgent` + `WanctlKeeper`（user login 跑）
- Working dir：user profile

## 端口 / 地址
- _n/a_（出网到 wanctl-relay.tbox.ktvsky.com，用户主动建立连接）

## 配置
- 主 config 路径：user profile（具体看 wanctl docs）
- 策略：default-deny + bypass 持久化（2026-06-30 用户改）—— bypass 状态写到本地配置文件

## 依赖
- relay：`wanctl-relay.tbox.ktvsky.com`
- 团队 SSO（飞书）：通过 `wanctl-portal` 拿 enroll code
- agent identity / trust：本地 wanctl trust store

## 启动 / 停止 / 重启
```pwsh
Get-ScheduledTask WanctlAgent, WanctlKeeper | Select TaskName, State

# 重启
Stop-ScheduledTask WanctlAgent
Start-ScheduledTask WanctlAgent
```

## 健康检查
```bash
# 从 mac (有 wanctl MCP 装)
wanctl_peers   # 期待返：zyl（home）+ zyldeMacBook-Pro.local
wanctl_exec target=zyl 'powershell -Command "whoami"'   # 期待 zyl\yilin zhang
```

## 日志
- agent 本地 log（具体路径 wanctl docs）
- relay 侧记录可通过 wanctl_logs 拉

## 变更历史
- 2026-06-30：装好 + 配 admin context service + 持久化 bypass 状态（用户实装）

## 已知问题 / 坑（mac 侧使用 wanctl 的痛点，issue#1 在跟）
- ⚠️ exec 超时 2 分钟，命令仍在远端跑（state desync）
- ⚠️ session token 频繁过期，要走飞书 OAuth 重新 enroll
- ⚠️ MCP tool schema 没暴露 `rebind` 参数，relay 重连时无法自动恢复（user 被反复打扰）
- ⚠️ exec 长命令撞 timeout 后 agent 自己挂掉（issue#1 #18）
- ⚠️ 目标进程等 stdin 时 wanctl 完全无提示（issue#1 #19）
- ⚠️ ssh-able 时优先用 ssh，wanctl 仅作 ssh 挂时兜底入口

## 引用
- 痛点合集：[memory `reference_wanctl_issues_2026_06_30`](#)
- issue：`https://g.ktvsky.com/zhangyilin/wanctl/-/issues/1`
- [machines/winhome.md](../machines/winhome.md)

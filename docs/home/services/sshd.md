# sshd

> Windows OpenSSH Server，公网 SSH 入口（:20777 → 内 :22）。pubkey-only。

## 部署位置
- 机器：winhome
- Binary：`C:\Program Files\OpenSSH\sshd.exe`（GitHub MSI v10.0.0.0，自装版；非 Windows 内置）
- Service manager：Windows Service `sshd`（AutoStart）
- Working dir：_n/a_

## 端口 / 地址
- listen：`:22`（LAN + 公网 NAT 转发自 :20777）
- 外部访问：`ssh -p 20777 "yilin zhang"@home.qmledmq.cn` 或 mac `~/.ssh/config` 别名 `winhome-pub`
- 路由器：公网 :20777 → LAN 192.168.1.19:22（治本，规避 :2222 brute force）

## 配置
- 主：`C:\ProgramData\ssh\sshd_config`
- 授权公钥（admin 组）：`C:\ProgramData\ssh\administrators_authorized_keys`
- 关键配置：`PubkeyAuthentication yes` + `PasswordAuthentication no` + `PermitRootLogin no` + `AuthenticationMethods publickey`（强制只公钥）+ `MaxStartups 200:30:500`（抗 brute force） + `LoginGraceTime 30`
- `Subsystem sftp` 用 **绝对路径**：`"C:/Program Files/OpenSSH/sftp-server.exe"`

## 依赖
- Windows OpenSSH Server feature（安装时 enable）
- Windows Defender Firewall inbound rule（默认装好时有）
- 凭据：mac/agent 公钥写入 `administrators_authorized_keys`，文件 ACL = `SYSTEM:F + Administrators:F`

## 启动 / 停止 / 重启
```pwsh
Get-Service sshd
Restart-Service sshd

# 校验 sshd_config 语法（重要：改 sshd_config 后必跑）
& "C:\Program Files\OpenSSH\sshd.exe" -t
```

## 健康检查
```bash
# 从 mac
ssh -o ConnectTimeout=5 winhome-pub 'whoami'   # 期待返 zyl\yilin zhang

# winhome 本地
Get-Service sshd | Select Status   # 期待 Running
```

## 日志
- Event log：`OpenSSH/Operational` channel（admin 才能读）
- 看登录成功：`Get-WinEvent -LogName 'OpenSSH/Operational' | Where Message -Match 'Accepted'`
- 看 brute force / failures：`Where Message -Match 'Invalid user|Maxstartups|preauth'`

## 变更历史
- 2026-06-30 凌晨：装 OpenSSH MSI v10.0.0.0、配 administrators_authorized_keys、加固 sshd_config
- 2026-06-30 14:00：sftp `Subsystem` 绝对路径修正（scp -O 兜底）
- 2026-06-30 15:30：撞 brute force MaxStartups 池满 → 改 `MaxStartups 200:30:500` + 加 `LoginGraceTime 30` + 删 `UsePAM no`（Win32-OpenSSH 不支持） + 路由器换 :20777 + Firewall block 5.231.242.0/24（commit `117f109`）
- 2026-06-30 15:30：加 `AuthenticationMethods publickey` 强制只公钥（commit `117f109`）

## 已知问题 / 坑
- ⚠️ **Win32-OpenSSH sftp subsystem garbage bug**：sftp 协议 `Received message too long`。**永远 `scp -O`**（legacy 协议）兜底。
- ⚠️ **`UsePAM no` 不识别**：Win32-OpenSSH 不支持 PAM，每连接 warn 一次。从 sshd_config 删该行。
- ⚠️ **`Match` block 之后所有 directive 都属于该 block**：全局 directive（`MaxStartups` 等）必须放第一个 `Match` 之前，否则 `sshd -t` 报 `not allowed within a Match block`。详 [home-runbook.md#quirk-13](../../home-runbook.md)。
- ⚠️ **Brute force banner = "Not allowed at this time"**：Windows OpenSSH 把 MaxStartups drop 映射成这串 banner（Linux 版不一样）。诊断时不要被字面误导，看 Event log `Maxstartups` 关键词。

## 引用
- [machines/winhome.md](../machines/winhome.md)
- quirks #11/#13/#14：[home-runbook.md](../../home-runbook.md)

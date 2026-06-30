# home runbook · 家里基础设施现状

> 本机/远程机器、服务、端口、依赖、known limits/quirks 的单一真相源。任何远程 debug / 新装服务前先翻这里，避免「以前 work 现在不 work」类问题靠记忆撞墙。
>
> 维护原则：现状变了立刻改这里。每个 quirk 都标记发现日期 + 撞过的原因，免得后人（含未来自己）再踩。

## 机器拓扑

```
+--------------------+   LAN 192.168.1.0/24    +--------------------+
|  mac (开发主力)    |  <-------------------> |  home Legion 笔电  |
|  M-series macOS    |                         |  Win11 Pro 25H2    |
|  zsh + Claude Code |                         |  + WSL2 Ubuntu     |
+--------------------+                         +--------------------+
         ^                                              ^
         |        公网 ISP 124.126.5.223                |
         +---- (CF DNS qmledmq.cn:8443/:2222) ---------+
                          (家宽，动态)
```

## 每台机器

### home Legion 笔电（zyl, Win11 Pro 25H2 native）

**物理位置**：家里
**LAN IP**：192.168.1.19（DHCP 当前租约稳定）
**入站**：路由器 NAT 把公网 :8443 → 192.168.1.19:8443（Caddy），公网 :2222 → 192.168.1.19:22（OpenSSH）
**用户**：`yilin zhang`（含空格，OpenSSH 10.0+ 支持 `User "yilin zhang"` quoted）

#### 跑的服务（Windows native）

| 服务 | 端口 | 由谁起 | 备注 |
|---|---|---|---|
| OpenSSH Server | 22 | Windows Service | GitHub MSI v10.0.0.0；sshd_config 已 `Subsystem sftp` 用绝对路径，但 sftp 协议仍有 Win32-OpenSSH bug → **永远 `scp -O`** |
| Caddy v2.11.4 | 8443 | NSSM service `Caddy` | `C:\Caddy\caddy.exe`，含 caddy-dns/cloudflare 插件；AutoStart + 失败自动重启 5s |
| `caddy-cert-sync` 任务 | — | Scheduled Task | 每 6h + 系统启动，从 WSL 拉 acme.sh wildcard cert 同步到 `C:\Caddy\certs\` + `caddy reload localhost:2019` |
| FlClash | TUN | 用户登录态 | 重度用户必开，bypass rules 见 quirks |
| pwsh 7.5 | — | 默认 shell | NSSM/Caddy/sshd 都跑在它上面 |

**重要资产路径（C:\Caddy\）**：
- `caddy.exe` `Caddyfile` `nssm.exe`
- `certs\qmledmq.cn.{crt,key}`（acme.sh wildcard，cp from WSL）
- `data\`（Caddy state，含旧 ACME account fallback）
- `sync-acme-cert.ps1` + `register-cert-sync-task.ps1` + `install-caddy-service.ps1`
- `caddy.service.stderr.log` + `cert-sync.log`
- `ddns-go\` + `C:\ddns\cf-ddns.ps1`（binary 留备查，service 已卸，见 quirks）

**SSH alias（mac `~/.ssh/config`）**：
- `winhome` → 192.168.1.19:22（仅 LAN，mac 在家时用）
- `winhome-pub` → home.qmledmq.cn:2222（公网，mac 出门用，端口转发已验）

**Caddyfile site 块**（当前 3 个，都是 placeholder respond 没接真后端）：
- `home.qmledmq.cn:8443`
- `*.lab.qmledmq.cn:8443`
- `*.qmledmq.cn:8443`（wildcard 兜底）
- 配置头：`auto_https disable_certs`，每 site 显式 `tls cert.crt key.key`（不走 ACME，详 quirk #1）

#### 跑的服务（WSL2 Ubuntu 24.04）

| 服务 | 用途 | 备注 |
|---|---|---|
| acme.sh | wildcard cert 续签 | cron 自动续；cert 落地 `/etc/nginx/ssl/qmledmq.cn.{crt,key}` + `/root/.acme.sh/qmledmq.cn_ecc/` |
| fuxi 编译环境 | rust toolchain | `~/.cargo/bin/cargo`（rustup, 1.95+）|
| ~~nginx~~ | ~~反代~~ | 历史遗留，**已不再是公网入口**（现 Caddy 接管）；`/etc/nginx/ssl/` 仍是 cert 落地点供 acme.sh 写 |

**WSL ↔ Windows 同步**：scheduled task `caddy-cert-sync` 跑 `C:\Caddy\sync-acme-cert.ps1`，比 mtime 拉新 cert。LogonType **S4U as `yilin zhang`**（SYSTEM 不能跑 WSL，详 quirk #4）。

#### 当前 cert SAN

```
qmledmq.cn
*.qmledmq.cn           # 三级 wildcard，覆盖 fuxi.qmledmq.cn / im.qmledmq.cn / ...
*.lab.qmledmq.cn       # 四级 wildcard，覆盖 lab 下的实验子站
```

有效期 → 2026-09-27（acme.sh cron 自动续）。

**给 cert 加新 tier-2 wildcard（如 `*.foo.qmledmq.cn`）的完整步骤** 见下面「常见运维操作」。

---

### mac（开发主力）

- macOS Darwin 25.0.0 / M-series
- 路径：`/Users/e0_7/fuxi`（symlink `/Users/e0_7/xihe` 指过来，兼容老路径）
- fuxi 主开发机；Claude Code 也跑这；ssh 各家机器
- **VPN**：Clash/Surge TUN 模式（与 winhome 上的 FlClash 同源问题，详 quirk #2）

---

### 公网入口

- **域名**：qmledmq.cn（用户拥有，注册商 = 阿里云万网 / wanwang.aliyun.com）
- **DNS**：Cloudflare（NS 已迁离 hichina，详「子域名管理」）
- **真 ISP IP**：124.126.5.223（家宽动态，几天/几个月不变；ddns 自动同步当前阻塞，详 quirk #2）
- **入站端口**：8443（HTTPS, Caddy）+ 2222（SSH）
- 80/443 因 ICP 未备案 + ISP 限制不可用，**永远走 8443**

---

## 常见运维操作

### 1. 改 fuxi 代码后部署到 home

**注意：此流程指 Linux 历史路径**。home 重装 Win11 后，fuxi 服务还没迁到 winhome native（Phase 2 工作）。当前仍按 [[reference_home_deploy]] 走（rsync 到 home，cargo build，cp 两份，restart fuxi-im.service）。**待迁完成后此节重写**。

### 2. 给 cert 加新 tier-2 wildcard

例：要 `*.foo.qmledmq.cn` 走 HTTPS。

```bash
# 1. WSL 内 acme.sh issue 新 SAN（已有 cert 就 --force）
ssh winhome 'wsl -d Ubuntu-24.04 -- bash -c "\
  cd /root/.acme.sh && \
  ./acme.sh --issue --force --dns dns_cf \
    -d qmledmq.cn \
    -d \"*.qmledmq.cn\" \
    -d \"*.lab.qmledmq.cn\" \
    -d \"*.foo.qmledmq.cn\" \
    --keylength ec-256"

# 2. install hook 已配，acme.sh 自动写 /etc/nginx/ssl/qmledmq.cn.{crt,key}
#    （如果没配，手动 cp from /root/.acme.sh/qmledmq.cn_ecc/）

# 3. 触发 winhome scheduled task 立即同步（不等 6h）
ssh winhome 'schtasks /Run /TN caddy-cert-sync'

# 4. 验 cert SAN
ssh winhome 'wsl -d Ubuntu-24.04 -- openssl x509 -in /etc/nginx/ssl/qmledmq.cn.crt -noout -text | grep DNS'

# 5. Caddyfile 加 site 块（如果要单独路由）
#    *.qmledmq.cn:8443 已能 wildcard 兜底，*.foo 走它就行；
#    只在需要不同 reverse_proxy 路径时才加显式 site 块。
```

**CF API token 要求**：`Zone:DNS:Edit` for qmledmq.cn。acme.sh 用的 `CF_Token` 已配，env 变量在 WSL `~/.acme.sh/account.conf`。

### 3. 加新子域名 CNAME 到 CF

```bash
# zone_id 见下面「子域名管理」
ZONE_ID=791621b616f83a44a34e4796adbe0920
TOKEN=<CF API token>

curl -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  "https://api.cloudflare.com/client/v4/zones/$ZONE_ID/dns_records" \
  --data '{"type":"CNAME","name":"newsub","content":"home.qmledmq.cn","ttl":1,"proxied":false}'
```

但其实 `*.qmledmq.cn` wildcard 已覆盖任意新子域名 → **不加 CNAME 也能用**，除非需要走 CF proxy（橙云）或指向 cfargotunnel。

### 4. Hyper-V Ubuntu Server VM 配置（待做）

winhome 上跑一个 Ubuntu Server VM 的步骤。**网络是关键决策点**。

#### 准备

```pwsh
# winhome pwsh 7（管理员）
# 1. 启 Hyper-V feature（重启一次）
Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All
Restart-Computer -Force

# 2. 拉 Ubuntu Server 24.04 LTS ISO（约 2.7GB）
$iso = "$env:USERPROFILE\Downloads\ubuntu-24.04.2-live-server-amd64.iso"
Invoke-WebRequest -Uri "https://releases.ubuntu.com/24.04/ubuntu-24.04.2-live-server-amd64.iso" -OutFile $iso
```

#### 网络选 External Switch（关键，别用默认 NAT）

```pwsh
# 找物理网卡（通常是有线 Ethernet，无线 Wi-Fi 也行但不稳）
Get-NetAdapter | Where-Object Status -eq 'Up'

# 创建 External vSwitch 绑物理网卡，VM 直接落 LAN 192.168.1.x
# AllowManagementOS=$true 让 winhome 自己也能用这块物理网卡
New-VMSwitch -Name "ExternalLAN" -NetAdapterName "<物理网卡名>" -AllowManagementOS $true
```

**为什么 External 不 Default Switch / Internal**：
- Default Switch = NAT，VM 拿到 172.x.x.x；mac/winhome 之外的设备**访问不到 VM**；VM 也不能被路由器 DHCP 看到
- External = 桥接到物理网卡，VM 直接拿 192.168.1.x 的 DHCP 租约，跟 winhome 平级；mac 可以 `ssh vm@192.168.1.xxx`；路由器可以做端口转发到 VM
- Internal = 仅 winhome ↔ VM 互通，外面看不到

#### 建 VM

```pwsh
$vmName = "ubuntu-server-1"
$vmPath = "D:\Hyper-V"
$vhd    = "$vmPath\$vmName\$vmName.vhdx"

New-VM -Name $vmName -MemoryStartupBytes 4GB -Path $vmPath `
       -NewVHDPath $vhd -NewVHDSizeBytes 50GB -SwitchName "ExternalLAN" -Generation 2

# Gen2 默认开 Secure Boot，Ubuntu 装好后再关；或者装时就关
Set-VMFirmware -VMName $vmName -EnableSecureBoot Off

# 装 ISO
Add-VMDvdDrive -VMName $vmName -Path $iso
Set-VMFirmware -VMName $vmName -FirstBootDevice (Get-VMDvdDrive -VMName $vmName)

# vCPU 4 核（按机器调）
Set-VMProcessor -VMName $vmName -Count 4

Start-VM $vmName
vmconnect localhost $vmName  # 弹出 console 装系统
```

#### 装完后

1. Ubuntu installer 里选 OpenSSH server（必勾）
2. 装完 `ip a` 看 DHCP 拿到的 IP（应是 192.168.1.xxx）
3. 在路由器后台给这个 MAC 做 **DHCP reservation**（绑定 IP），免得重启变 IP
4. mac `~/.ssh/config` 加 alias
5. Hyper-V Manager → VM → 设置 → Automatic Start = Always start，Automatic Stop = Save state（winhome 关机时 VM 自动保存状态，不丢数据）

#### 可选：Hyper-V Enhanced Session

只有跑 GUI 桌面才需要。Server ISO 不需要。

---

## 已知 limits / quirks

### #1. Caddy auto-ACME 在 winhome 跑不通（2026-06-30 发现）

**症状**：Caddy 自动走 DNS-01 challenge 永远 polling 不到 expected TXT，CF API 401/81058。
**根因**：FlClash TUN + CF anycast NS + 1.1.1.1 DNS cache stale，lego（caddy 内的 ACME 库）查 TXT 的 resolver 看到的不是 CF 写入的最新 record。
**解**：走静态 cert + WSL acme.sh sync 路径（已 ship）。Caddyfile `auto_https disable_certs` + 每 site 显式 `tls`。
**不要再排**：撞过 1 次，绕了一晚。

### #2. FlClash TUN 拦截公网 IP 检测端点（2026-06-30 发现）

**症状**：winhome 上 ddns-go / cf-ddns 拿到的「公网 IP」是代理出口 IP（Azure 段，103.172.x / 4.193.x），不是真 ISP IP 124.126.5.223。
**根因**：FlClash TUN 默认配置把所有 HTTPS 流量代理出去，包括 ipify.org / cloudflare.com/cdn-cgi/trace / icanhazip.com / ipinfo.io/ip。
**解（待用户决策）**：
- (a) **FlClash 加 DIRECT bypass rules** 让 ipify / cloudflare / api.cloudflare.com 走直连。用户之前 ddns-go 在 win 跑得顺正是因为旧 FlClash 配了这些 bypass，重装 winhome = 默认无 bypass。
- (b) mac 跑 cf-ddns，mac 上 Clash 加同样 bypass。
- (c) 手动维护，IP 变了登 CF 改一次（IP 多数几天/几个月不变可接受）。

**当前状态**：CF DNS 正确（mac PUT 回 124.126.5.223），ddns-go service 已卸。

### #3. Win32-OpenSSH sftp subsystem 有 garbage bug

**症状**：`sftp` 协议传文件报 `Received message too long`。
**根因**：Win32-OpenSSH 的 sftp 子系统已知 stderr 污染问题，即使 sshd_config 路径修对仍报。
**解**：永远 `scp -O`（legacy SCP 协议，绕 sftp 子系统）。**任何上传 winhome 的脚本都要用 `-O`**。

### #4. Windows SYSTEM 用户不能跑 WSL

**症状**：`WSL_E_LOCAL_SYSTEM_NOT_SUPPORTED`。
**根因**：WSL2 设计上拒 SYSTEM 上下文。
**解**：scheduled task 跑 WSL 必须 `LogonType S4U + UserId="yilin zhang"`。

### #5. pwsh 7 不在 SYSTEM PATH

**症状**：scheduled task action `pwsh -File ...` 报找不到。
**根因**：pwsh 7 是 user-scope 装（非 system MSI）。
**解**：用 `powershell.exe`（内置 PS 5.1）或 pwsh 7 绝对路径。

### #6. NSSM 装来源

`nssm.cc` 站点 503 已久。**用 `winget install NSSM.NSSM`**。

### #7. NSSM 跑 caddy 时设 AppDirectory

caddy 即使绝对路径也可能受 cwd 影响（如 Caddyfile 内 import 用相对路径）。`AppDirectory = C:\Caddy` 防万一。

### #8. 华为光猫一体机 UPnP 没 IGD

**症状**：UPnP 端口转发自动化失败，光猫只暴露 WPS device。
**根因**：华为光猫一体机出厂常态，`manufacturer=huaweitec, modelName=WAP` 仅 WPS UPnP。
**解**：手动登光猫管理面板做端口转发，别在 UPnP IGD 上浪费时间。

### #9. mac shell `HTTPS_PROXY` 干扰 ssh ProxyCommand

**症状**：`ssh home` / `ssh winhome-pub` 偶发 SSL 35 失败 / DNS 解不到。
**根因**：mac shell 设了 `HTTPS_PROXY`（Clash/Surge HTTP proxy）时，ssh config 的 `ProxyCommand` 走 DOH（cloudflare-dns 查 A 记录）受影响。
**解**：长跑 ssh 必须 `env -u HTTPS_PROXY -u https_proxy ssh ...` 包一层。

### #10. FlClash fake-IP 错误信息会掩盖远端真实错误（2026-06-30 学到，**坑王**）

**症状**：`ssh winhome-pub` 报 `Connection closed by 198.18.0.80 port 2222`。198.18.0.0/15 是 Clash fake-IP 池，看起来全是 TUN 锅。
**真相**：远端 sshd / 服务进程 reset 连接时，**Clash TUN 在 error 路径会把回包 source 替换成 fake IP** → 客户端看到的「远程地址」是假的，但 reset 是真的远端发的。
**诊断**：怀疑是远端服务而非网络时：
- 关 TUN 跑同一条命令，看真错（如 `Connection reset by peer` + 真 IP）
- 并行 `curl :8443` / 其他公网端口验「机器/路由器整体活着」
- `ssh -v` 看 banner 阶段输出，banner 内容（如 `Not allowed at this time`）是 sshd 给的，不是 Clash 编的
**何时撞过**：本次诊断 sshd reset 时绕了 30 分钟以为是 TUN。

### #11. sshd `Not allowed at this time` 真因 = **公网 brute force 打满 MaxStartups 池**（2026-06-30 诊断闭环）

**症状**：`ssh -v winhome-pub` 看到 `banner line 0: Not allowed at this time` 紧接 `Connection timed out during banner exchange`。但 `Caddy:8443 HTTP 200` + `RDP:3389 listen` = 机器/路由器都活，**仅 sshd auth 阶段拒**。
**最初 3 个假设全部**否定（沉淀诊断顺序，下次省时间）：
1. ❌ `nologin` 文件存在 — 不存在
2. ❌ `sshd_config` 有 `Match Time/Deny/Allow` 拒绝 — 只有标准 `Match Group administrators` + AuthorizedKeysFile path
3. ❌ Windows 账户 `Logon Hours / Lockout` 限制 — `net user "yilin zhang"` 显示 `Logon hours allowed: All` + `Account active: Yes`
**真因（OpenSSH Operational event log 给的）**：
```
sshd: drop connection #46 from [5.231.242.53] on [192.168.1.19]:22 Maxstartups
sshd-session: Invalid user root from 5.231.242.53
```
- 公网 :2222 → 路由器 → 内 :22，**全球 SSH 扫描机器人疯狂打 root**（伊朗 5.231.242.0/24 一秒几十个连接）
- OpenSSH 默认 `MaxStartups 10:30:100` = 10 个未认证并发后 30% 概率 drop，100 个全 drop
- mac 合法 ssh 进来时**也被 Maxstartups drop**，sshd 在 Windows 版本下关闭 banner = "Not allowed at this time"（与 Linux 版的 "Too many authentication failures" 映射不同）
**修复（已 ship 2026-06-30 15:30）**：
```pwsh
# sshd_config: 把 MaxStartups + LoginGraceTime 加在 Match block 之前（否则 sshd -t 报「not allowed in Match block」）
MaxStartups 200:30:500
LoginGraceTime 30
AuthenticationMethods publickey   # 强制只公钥（已加）
# 删 'UsePAM no' — Win32-OpenSSH 不支持
```
+ 路由器公网端口换 20777（删 2222 转发，机器人扫 2222 远多于 20777）
+ Windows Firewall: `New-NetFirewallRule -DisplayName "Block-IR-5.231.242.0_24" -Direction Inbound -Action Block -RemoteAddress 5.231.242.0/24 -LocalPort 22`
+ mac `~/.ssh/config` winhome-pub Port 20777

### #12. RDP 3389 当前公网开放（2026-06-30 发现，**安全风险待决策**）

### #12. RDP 3389 当前公网开放（2026-06-30 发现，**安全风险待决策**）

**症状**：`nc -zv 124.126.5.223 3389` succeeded。意味着路由器把公网 :3389 转给了 winhome。
**风险**：RDP 暴露公网 = 被全网扫 + 字典爆破。Windows RDP 历史漏洞链多（BlueKeep / etc）。
**为何还在**：可能是早期排查时打开的端口转发没关；或用户主动留作救命通道。
**当前救命用途**：sshd 挂了时，mac 可以用「Microsoft Remote Desktop」app（App Store 免费）连 `124.126.5.223:3389` 进 winhome 桌面修服务。
**长期建议（用户决策）**：
- (a) 保留 RDP 公网，但改非标端口（如 3389 → 13389）+ 限源 IP 白名单（mac 出门常用 4G + 公司）
- (b) 关 RDP 公网转发，依赖 ssh，sshd 挂了就只能等回家
- (c) 用 Cloudflare Tunnel 代理 RDP（Zero Trust 鉴权），公网零暴露
**短期**：先用 RDP 修 sshd，回头再决策。

### #13. sshd_config `Match` block 之后所有 directive 都属于该 Match（2026-06-30 踩）

**症状**：往 sshd_config 末尾 append 全局 directive（`MaxStartups`/`LoginGraceTime`），`sshd -t` 报 `Directive 'MaxStartups' is not allowed within a Match block`。
**根因**：OpenSSH `Match` block 没有显式结束符——一旦出现，后续 directive 全部归属该 block 直到下一个 `Match` 或 EOF。
**正确写法**：全局 directive **必须放在第一个 `Match` 之前**。修法 — 解析 sshd_config + 在 Match 行前插入新 directive。

### #14. Win32-OpenSSH 不识别 `UsePAM` directive（2026-06-30）

**症状**：sshd Operational log 反复出现 `sshd-session: user: (null): rexec line 12: Unsupported option UsePAM [preauth]`，每个新连接打一条 warn。
**根因**：Win32-OpenSSH 没有 PAM 子系统，对 `UsePAM no/yes` 直接报 unsupported（不致命，但 log 噪音）。
**修法**：从 sshd_config 删 `UsePAM` 行。Linux OpenSSH 才需要这条。

### #15. Defender ML 把 `Start-Job + TCP listener + 立刻自连` 模式误报为 `Trojan:Win32/ClickFix.CCJ!MTB`（2026-06-30）

**症状**：`Get-MpThreat` 显示 `Trojan:Win32/ClickFix.CCJ!MTB`，但 `DidThreatExecute=False` `IsActive=False` `ThreatStatusID=4 (Cleaned)`，`Resources` CmdLine 是自己跑过的 PS 诊断脚本（停 WSL nginx + `Start-Job { New TcpListener :8443 → AcceptTcpClient → WriteLine HELLO_FROM_WINDOWS_8443 }`）。
**根因**：ClickFix 家族（社工 trojan）确实常用 PowerShell + 启 listener + IEX 风格代码，Defender ML heuristic 误判。
**应对**：本人写的合法诊断脚本可直接忽略此报警；如要避免误报，把 listener 启动改成 `[System.Net.Sockets.TcpListener]::new($endpoint)` 之外加点 noise（comment / 配置文件 driven）让特征不那么集中。

### #16. Bash tool 子 shell `unset HTTPS_PROXY` 不持久

**症状**：在 Claude Code 里 `unset HTTPS_PROXY && ssh ...` 第一次行，第二次 Bash tool 调用又不行。
**根因**：Bash tool 每次启新 shell 从父 env 再注入 `HTTPS_PROXY`。
**解**：每条 ssh 命令都用 `env -u HTTPS_PROXY` 显式清。

---

## 子域名管理 / CF DNS

### Zone 状态

| 项 | 值 |
|---|---|
| Zone ID | `791621b616f83a44a34e4796adbe0920` |
| 当前 NS | `jermaine.ns.cloudflare.com`, `perla.ns.cloudflare.com` |
| 原 NS | `dns27.hichina.com`, `dns28.hichina.com`（阿里云万网） |
| 注册商 | 阿里云万网（wanwang.aliyun.com） |
| 状态 | active, full DNS |
| Plan | Free |
| 迁入 CF 时间 | 2026-03-01 |

**hichina 残留检查**：CF zone 已迁入 active，hichina NS 不再被任何 TLD 服务器返回。**但用户在阿里云万网域名后台的「域名解析（DNS 解析 DNS）」里可能还留着老 records**——这些不生效但属于历史垃圾。**清理需要用户自己登 wanwang.aliyun.com**（Claude 没账号）：

1. 登 https://wanwang.aliyun.com
2. 域名列表 → qmledmq.cn → 解析设置
3. 备份当前 records 截图
4. 全删（或全暂停）

`dnspod` 没出现过，用户记忆有误。

### 当前 records 速览（2026-06-30 快照）

详见 `docs/cf-dns-cleanup-2026-06-30.md`（user 勾选清单）。

---

## 待办（推进顺序由用户拍）

0. ✅ **修 winhome sshd `Not allowed at this time`**（quirk #11 闭环）。真因 = 公网 brute force 把 MaxStartups 打满；修法 = MaxStartups 200:30:500 + 路由器换 20777 + AuthenticationMethods publickey + Firewall block 伊朗段。**全审查无入侵**（见 quirk #15）。
1. **fail2ban-style 自动 ban brute force IP**（nice-to-have，未做）。当前只手动 ban 了 5.231.242.0/24。可写 PS scheduled task 每分钟扫 sshd Operational log 把 N 次失败的 IP 加到 Firewall block list。
1. **fuxi 服务从老 home Linux 迁到 winhome native / Hyper-V Ubuntu VM**
   - 当前 fuxi-im.service 仍跑在老 Ubuntu Linux（参 [[reference_home_deploy]]），与 winhome Win11 native 是两个分支
   - 决策点：直接 winhome WSL Ubuntu 跑？还是 Hyper-V Ubuntu Server VM 跑？前者轻后者重，前者 systemd 限制后者完整
2. **FlClash bypass rules**（决策 (a)/(b)/(c) 见 quirk #2）
3. **公网 SSH 出门 4G 验证**（验路由器 2222→22 + ISP 不封）
4. **Caddyfile reverse_proxy 接真后端**（当前都是 placeholder respond，没接 fuxi-im / jarvis backend）
5. **WSL acme.sh 续签 hook 通知 winhome 立即同步**（当前 6h 粒度足够，nice-to-have）

---

## 引用

- [[reference_home_deploy]] — 老 home Linux 部署流程（rsync + cargo + restart）
- [[reference_wanctl_issues_2026_06_30]] — wanctl 痛点（已 issue#1，agent 默认 ssh winhome）
- `docs/cf-dns-cleanup-2026-06-30.md` — CF records 清理勾选清单
- CLAUDE.md 「常见陷阱」节 — fuxi 工程级 quirk（与 home 基础设施 quirks 互补）

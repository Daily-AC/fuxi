# home-qm

> qmledmq.cn 子域名管理 CLI。维护 `domains.yaml` 注册表 → 生成 Caddyfile 片段 → reload Caddy。

## 部署位置
- 机器：winhome
- Binary：`C:\Caddy\qm.exe`（1.3MB，Rust release build）
- Service manager：_n/a_（CLI，按需调用）
- Working dir：调用时当前目录
- PATH：`C:\Caddy` 已加 Machine PATH，新 ssh session 直接 `qm` 命中

## 端口 / 地址
- _n/a_（CLI）

## 配置
- 注册表：`C:\ProgramData\qm\domains.yaml`（YAML，tier1/2/3 分层）
- 输出片段：`C:\Caddy\Caddyfile.qm`
- 主 Caddyfile：`C:\Caddy\Caddyfile`（要含 `import C:/Caddy/Caddyfile.qm` 指令）
- 默认 caddy admin：`localhost:2019`

## 依赖
- [caddy](caddy.md) 跑着 + admin API 监听 `localhost:2019`
- 主 Caddyfile 已 `import C:/Caddy/Caddyfile.qm`

## 启动 / 停止 / 重启
_n/a_（按需调）

```pwsh
# 列已注册
qm list

# 加（tier1=canonical / 2=project / 3=lab，tier3 默认 30 天 expire）
qm add im --backend localhost:18080 --tier 1 --purpose "fuxi IM PWA"
qm add foo --backend localhost:8888 --tier 3 --purpose "lab experiment"

# 摘除
qm retire foo

# 重新生成 Caddyfile.qm + reload Caddy
qm sync

# 状态
qm status
```

backend 可写 `localhost:<port>`（同主机 Windows native 服务）/ `localhost:<wsl_port>`（mirrored mode 透明反 WSL）/ `192.168.1.x:<port>`（LAN）。

## 健康检查
```pwsh
qm status   # 期待返 registry 路径 + 各 tier 计数
qm list     # 期待返已注册条目
```

## 日志
- _n/a_（CLI 直接 stdout）

## 变更历史
- 2026-06-30 17:00：Rust 重写 ship，从老 Python qm（写 nginx config）迁过来；mac 交叉编译 x86_64-pc-windows-gnu。端到端实测：`qm add ddns --backend localhost:9876 --tier 3` → mac curl → HTTP/2 + TLS + 307（commit `0fe2817`）

## 已知问题 / 坑
- ⚠️ `caddy reload` 输出会带 `Caddyfile input is not formatted; run 'caddy fmt --overwrite'` warning。不致命。
- ⚠️ tier3 默认 30 天 expire 字段写入 yaml，但**当前没自动 GC 过期条目**。需要手动 retire 或者后续加 `qm prune-expired` 子命令。

## 引用
- [caddy](caddy.md)
- 源码：`crates/home-qm/`（fuxi workspace 内）
- 同名工具旧版（已废弃）：WSL `/usr/local/bin/qm`（Python，写 nginx config）—— 现在 nginx 不在公网链路上，**已无效**，不要再调

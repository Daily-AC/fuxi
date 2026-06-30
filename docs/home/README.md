# home 文档

> home 基础设施 / 服务 / 项目的单一真相源。**只记长期稳态**，动态状态用 CLI 现查（`qm list` / `Get-Service` / `systemctl status` / 等），避免文档 stale 误导 agent。

## 设计原则

1. **每条信息只在一处**：一个 service / project / machine 一个 .md 文件；改一处不需要跨文件同步。
2. **每个目录有 INDEX.md**：一行一指针，agent 用 INDEX 跳到具体文件。
3. **统一 schema**：services 和 projects 各有 `SCHEMA.md`，加新条目 copy schema、填空即可。
4. **稳态 vs 瞬时**：稳态（端口/路径/依赖/启动方式）写文档；瞬时（process 是否活、当前 IP、当前 token）用工具现查。
5. **可索引**：grep `docs/home/` 能定位任何 keyword。

## 索引

| 类别 | 索引 | 内容 |
|---|---|---|
| 机器 | [machines/INDEX.md](machines/INDEX.md) | home / mac / 待加 VM 等 |
| 服务 | [services/INDEX.md](services/INDEX.md) | Caddy / sshd / ddns-go / acme.sh / wanctl-agent / ... |
| 项目 | [projects/INDEX.md](projects/INDEX.md) | fuxi / sia / 桌宠 / ERP / ... |
| 参考 | [refs/](refs/) | 网络拓扑 / 端口分配 / 凭据位置 |

## 运维与 quirks

[../home-runbook.md](../home-runbook.md) 仍是「机器拓扑 / 常见运维 / 已知 quirks」的总集，文档分层后**逐步迁移**：
- quirks → `docs/home/refs/quirks.md`（待迁）
- 常见运维 → 各 service .md 的「运维」节
- 机器拓扑 → `docs/home/machines/*.md`

短期保留两份共存，避免一次性大重构。新加内容**优先进 docs/home/ 新结构**。

## 加新条目流程

```bash
# 加一个 service
cp docs/home/services/SCHEMA.md docs/home/services/<name>.md
# 编辑 <name>.md 填空，按 SCHEMA 章节顺序
# 更新 docs/home/services/INDEX.md 一行
git add docs/home/services/<name>.md docs/home/services/INDEX.md
git commit -m "docs(home/services): 加 <name>"
```

同样适用 projects / machines。

# services/SCHEMA.md

> 每个 service .md 必须遵循的章节顺序。**章节顺序不可变**（保证 grep + 视觉定位）；某节没内容写 `_n/a_` 而不是省略。

```markdown
# <service-name>

> 一句话用途。

## 部署位置
- 机器：winhome / mac / wsl-ubuntu-24.04
- Binary：`/绝对/路径/`
- Service manager：systemd / NSSM Windows Service / Scheduled Task / 用户登录态 / 手动
- Working dir：...

## 端口 / 地址
- listen：...
- 外部访问：...

## 配置
- 主配置：`/path/to/config`
- 其他：...

## 依赖
- 上游服务：[[other-service]]
- 系统依赖：...
- 凭据：见 [refs/secrets-locations.md](../refs/secrets-locations.md) #<key>

## 启动 / 停止 / 重启
```bash
启停命令
```

## 健康检查
```bash
检查命令 + 期待返回
```

## 日志
- 路径：...
- 关键 grep 模式：...

## 变更历史
- YYYY-MM-DD: 描述（commit ref）

## 已知问题 / 坑
- 标 ⚠️ 或 ✓（解决）

## 引用
- 相关：[[other]]
- 上层 quirk：[home-runbook.md#quirk-N](../../home-runbook.md)
```

## 章节备注

- **稳态**才进文档。当前 IP / process PID / 实时连接数 → 不写，agent 现查。
- 改完任何 service 配置，**同步改这个 file 的对应节**；commit 一起。
- 「变更历史」每次值得记的改动加一行，便于 diff 上下文。

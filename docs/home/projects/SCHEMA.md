# projects/SCHEMA.md

> 每个 project .md 必须遵循的章节顺序。

```markdown
# <project-name>

> 一句话项目愿景。

## 状态
- 当前阶段：design / building / shipped / maintained / paused / archived
- 主仓库：<repo URL>
- 最近大事件：YYYY-MM-DD

## 部署
- 跑在哪台机：[winhome](../machines/winhome.md) / [mac](../machines/mac.md) / cloud
- 用到哪些 service：[[caddy]] / [[fuxi-im]] / ...
- 子域名（如适用）：<sub>.qmledmq.cn

## 入口
- 用户访问：URL / app 路径
- 开发：`cd ...; <run cmd>`

## 关键路径
- 配置 / 数据：...
- 凭据：见 [refs/secrets-locations.md](../refs/secrets-locations.md) #<key>

## 依赖
- 上游服务：...
- 外部 API：...

## 已知 issue / 待办
- ...

## 引用
- 设计文档：...
- 决策记录：...
- 相关 service：[[other]]
```

每个 project 一个 .md。状态变化重要节点写「最近大事件」更新日期。

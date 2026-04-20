# Decision 02 · Skill 是角色包，不止 SKILL.md；soul 优先

**日期**：2026-04-19  
**状态**：已采纳，所有 role 强制执行

## 背景

最初实装 `fuxi-skills` 时把每个 role 写成单一 `SKILL.md` 平面文件。用户点醒：

> skill 并不是只有 skill.md 这么窄，所以一个角色包可以非常丰富，还可以在 skill.md 分层到更多的 md，懂我意思吧？**一个角色重要的是 soul 其次才是别的 —— soul 是角色的核心愿景以及使命和价值观**。

## 决策

每个 role 是一个**目录**（skill bundle），包含：

```
skills/<role>/
├── SKILL.md              # 入口：frontmatter + body（body 顶部就是 soul）
├── instructions/         # 具体流程 / 工具指引 / 协议
│   ├── tool-map.md
│   ├── <protocol>.md
│   └── ...
├── resources/            # 参考资料（公理 / 项目 context / 规约）
└── examples/             # 历史范例（可从河图洛书晋升）
```

`SKILL.md` 的 body 前 3-5 段是 **soul**，回答三个问题：
1. **我是谁**（身份 / 文化定位）
2. **我为何存在**（使命 / 价值主张）
3. **我的价值观**（原则 / 底线 / 行事风格）

之后才写"工具清单" / "常见流程"等。

## 理由

### 技术上
- agentskills.io 是开放规范，Anthropic/Microsoft/Cursor/OpenCode/Goose 都支持完整 bundle 结构
- 跨 CLI 复用：同一个 `luban/` 目录 cc / codex / opencode 都能 load
- **招贤机制的技术前置**：新 role 诞生 = 写新 skill bundle；非代码改

### 产品上
- soul-first 不是美学选择，是让用户**能手改角色行为不用改代码**
- 门客越多 soul 越重要——它是玄女区分 "派谁做什么" 的决策根据
- 招贤（铸牒司）要写出"活的新角色"，模板必须带 soul 槽位

## 影响面

- C1 重写 `xuannv/` + `luban/` 按 soul-first（+ instructions/ + resources/ + examples/）
- C4 铸牒司的 3 份 archetype（`dev/pm/research.archetype.md`）都留 soul 槽
- 2026-04-20 Fix-A 把玄女工具表扩充到 memory/skill/cron 时 follow 这个模式（新加 `§8 系统事件响应` + `§9 记忆主动积累` 到 dispatch-protocol.md 而非塞回 SKILL.md）

## 反例禁止

- 一份 SKILL.md 写 200 行什么都塞—— body 膨胀意味着 soul 被淹没
- 跳过 soul 直接写工具列表—— role 没灵魂就只是 hardcoded prompt

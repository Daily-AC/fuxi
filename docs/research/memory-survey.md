# 伏羲记忆系统调研与方案

> 写给主线 Claude：读完即开工。重判断，轻综述。

## 1. 方案一句话定性

1. **Claude Code `--resume` + CLAUDE.md + Skills**（cc 原生）：会话 JSONL 自动落盘，顶层 md 每次启动注入，skill 按需加载。**适合伏羲门客层**（cc 门客白嫖即可），**不适合玄女跨会话长期记忆**——JSONL 仅给单会话续写。
2. **Letta / MemGPT 三层**（core / recall / archival）：core 是可编辑的固定上下文块，recall 存对话原文，archival 是向量知识库，agent 自己调函数搬运。**概念可借，落地不抄**——core block = CLAUDE.md 思路已有，archival 需向量，伏羲不要。
3. **Mem0 1.2 token-efficient**（一次 LLM 调用只 ADD）：每条事实独立存储，冲突不覆盖而是并存，BM25 + 语义 + 实体融合检索。**伏羲核心参考**——anya loid 已是 Mem0 1.0 风格（两阶段 extract + consolidate），升级到 1.2 也就是把 consolidator 改成"默认保留、只在明显冲突时 DELETE"。
4. **Zep / Graphiti 时序知识图谱**：bi-temporal 边（事件发生 + 入库时间），事实过期不删只标失效。**概念强大但过重**——伏羲单机不上 Neo4j，且玄女的决策/偏好更新不到需要时间窗查询的粒度。借"soft delete + 时间戳"即可。
5. **Anya loid**（TS，最近亲）：SQLite `memories` 表 + `ChatMemoryExtractor`（importance≥7 + BM25 去重）+ `PersonMemoryConsolidator`（每日 ADD/UPDATE/DELETE）+ `MemorySettler`（任务完成抽 pitfall/decision/collaborator）+ `memoryBriefV2` 装配对话上下文。**直接移植到 Rust，改掉耦合飞书的部分即可**。
6. **Goose `.goosehints` + Memory extension**：用户可编辑 hints + agent 自动 key-value 存储，local/global 两层。**太薄**，伏羲场景需要按 person/project/task 分 scope。
7. **Cursor Project Rules + Memory Bank**：`.cursor/rules/*.md` 按 glob 触发，memory bank 是社区约定的几个 md 文件结构。**伏羲的 skill files 层直接对应**，不需要独立方案。
8. **纯 append-only 事件流 + 按需 SQL 聚合**（不做抽取）：把每次交互原封存 `events` 表，玄女每次启动时 SQL 跑几条聚合（最近 7 天 + 高 importance 标记）。**兜底方案**，对大多数场景够用但"跨会话记得用户叫工具玄女不叫 orchestrator"这种行话学习要手写规则才行。

## 2. 对比表

| 方案 | 持久化 | 检索 | 写入策略 | 成本 | Rust 落地 |
|---|---|---|---|---|---|
| cc `--resume` | JSONL 文件 | 全量重放 | 被动落盘 | 零 | 门客已有，玄女不够 |
| Letta 三层 | PG + 向量 | 函数调用 + 向量 | agent 自管 | 高 | 向量拒用 |
| Mem0 1.2 | 向量/关系 DB | BM25+语义+实体 | 一次 LLM 只 ADD | 中 | **概念可抄** |
| Zep Graphiti | Neo4j | 图遍历+BM25+embedding | 实体/关系抽取 | 很高 | 拒 |
| Anya loid | SQLite | BM25 (FTS5) | 阈值+去重+日整理 | 低 | **直接移植** |
| Goose hints | ~/.config md | 全量注入 | 手写或 agent 追加 | 零 | 太薄 |
| Cursor rules | `.cursor/rules/*.md` | glob/manual | 人写 | 零 | 对应 skills |
| 纯事件流 | SQLite events | SQL | 无抽取 | 低 | 兜底 |

## 3. 推荐方案

**主方案：anya loid 架构 Rust 化 + Mem0 1.2 思想 + cc 原生 --resume 叠加。**

三层分工：

- **门客层（cc/codex/gemini）**：用各家原生 session resume。`fuxi-agent-cc` 已走 headless，按门客实例持久化 `cc_session_id` 到 SQLite（类似 `backend-models` 表），下次 dispatch 同一门客时 `--resume`。codex 不支持就每次 fresh（CLAUDE.md 已记这条铁律）。这一层不需要伏羲写记忆。
- **玄女长期记忆**：SQLite `memories` 表，字段直接复刻 anya：`(id, scope, scope_id, what, why, visibility, source_type, importance, hit_count, last_hit_at, created_at, deleted_at)`。scope = `person | project | org`（比 anya 多一个 `project`，匹配伏羲按仓库划分的场景）。写入走**两条路径**：
  1. **被动**：玄女每次对话结束后，由 orchestrator 拉起一次轻量 extractor（等价 `ChatMemoryExtractor`），importance≥0.7 + BM25 去重才入库。
  2. **主动**：玄女显式调 `memory_remember` 工具（source_type=`explicit`）。
- **门客经验沉淀**：门客任务 DONE 时，orchestrator 从 EventBus 里扫该 task 的事件流（`TaskCreated`→`AgentOutput`→`TaskCompleted`），用 `MemorySettler` 风格抽 pitfalls/decisions/collaborators 入 `memories`（scope=`project` 或 `org`）。伏羲比 anya 优势：**事件流已经在 EventBus 持久化了**，不用像 anya 去解析 `execution-log.jsonl`。

**不做**：向量、图数据库、Mem0 的实体抽取（单机太重）；Letta 的 archival（没必要把"门客干过的活"再做向量）；Zep 的 bi-temporal（soft delete + `created_at/updated_at` 的单时间轴够用）。

**做但延后**：每日 `PersonMemoryConsolidator` 一样的离线合并（Mem0 1.2 风格——默认只 ADD，冲突才 DELETE）。v0.1 可以先 append-only，靠 hit_count 和 importance 排序兜底；积累到 500+ 条再加整理器。

## 4. 命名映射

按已定命名池：

| 中文 | 英文表/模块 | 角色 |
|---|---|---|
| **策府** | `cefu` crate（`fuxi-memory`）| 记忆总库，对外接口 |
| **甲骨** | `oracle_facts` 表（sqlite）| 长期事实：用户偏好/性格/行话/项目架构/历史决策 |
| **简册** | `scroll_events`（复用现有 EventBus `events` 表）| 事件日志，append-only，`AgentOutput` / `TaskCompleted` 等都在这儿 |
| **河图洛书** | `hetu_patterns` 表 | 门客沉淀的"学到的模式/技能"，对应 anya pitfall/decision——规则型知识，可升级为 SKILL.md |

实际 crate/表命名建议走英文（按 CLAUDE.md 第 2 条）：crate 叫 `fuxi-memory`（注释里标"策府"），表叫 `oracle_facts` / `hetu_patterns`。

## 5. 与伏羲现有基建的衔接

- **EventBus**：简册就是它，零新增。长期记忆抽取器订阅 `TaskCompleted` 和 `ConversationTurnClosed`（如还没有就加一个）事件类型，做 extract。按 CLAUDE.md「加新 EventKind 变体必须更新 Firehose 渲染和 EventStore 持久化测试」这条铁律走。
- **SKILL.md / skills dir**：河图洛书的"高置信、可复用"条目到了一定成熟度，主动转写成 `skills/<topic>/SKILL.md`（progressive disclosure 两级，frontmatter + 正文 <500 行）。这是"从 SQLite 提升到可被 cc/codex 原生加载"的晋升路径——甲骨/河图洛书是玄女私藏，skills 是发给门客的指令。
- **orchestrator**：两个勾子：
  - Pre-turn：玄女收到用户消息，orchestrator 调 `cefu::brief(person_id, project_id)` 拼装 memoryBrief，和 `recentMessages`（从 EventBus replay）一起注入 cc/codex prompt。等价 anya 的 `LoidContextBuilder::buildMessageContext`。
  - Post-task：`TaskCompleted` 触发 settler，从 EventBus 扫该 task 的 span 抽记忆。
- **daemon (Unix socket)**：玄女 CLI 里加子命令 `fuxi mem remember/recall/search/forget`，走 daemon 转发到 cefu crate。对应 anya 的 MCP 工具 `memory_remember/recall/search`。
- **fuxi-core 新 trait**：`MemoryStore`（append/find_active/soft_delete/merge/bump_hit），实现放 `fuxi-memory`。遵循"library 不 unwrap"铁律，全返回 `Result<_, MemoryError>`。

## 6. 落地薄片拆分（建议）

- **薄片 M1**：`oracle_facts` 表 + migration + `MemoryStore` trait + append/find/soft_delete 三个 SQL 方法 + 单测。
- **薄片 M2**：`cefu::brief(person_id, project_id) -> String` 装配函数 + `fuxi mem` CLI 子命令。
- **薄片 M3**：订阅 `ConversationTurnClosed` 的 extractor（LLM 调用走玄女同一个 runtime，importance 阈值 0.7，BM25 dedup 用 SQLite FTS5）。
- **薄片 M4**：订阅 `TaskCompleted` 的 settler（从 EventBus span 扫，抽 pitfall/decision/collaborator 入 `hetu_patterns`）。
- **薄片 M5**（延后）：每日 consolidator（Mem0 1.2 风格，只 ADD，冲突才 DELETE）。
- **薄片 M6**（延后）：hetu → SKILL.md 晋升器，自动把高 hit_count 条目生成 skill 文件。

## 7. 关键判断摘要

- 伏羲 ≠ anya：anya 是飞书聊天场景（person-centric），伏羲是本地编排（project-centric），所以 scope 要加 `project`。
- 不抄向量：单机 + 中文语料 + <10k 条记忆，BM25（SQLite FTS5）跑 p95<10ms，够用且零依赖。
- 事件流复用：伏羲已有 append-only events 表，anya 的 `execution-log.jsonl` 解析这一层伏羲不用做。
- cc `--resume` 解决的是"门客记得上次干到哪"，不是"玄女记得用户"。两个层要分清楚，不要混用一个机制。
- SKILL.md 是晋升终点：SQLite 里高频高价值的条目最终要变成 skill 文件，这样 cc 门客原生加载，省掉玄女每次注入的开销。

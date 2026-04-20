# Handoff · v1 · Session 3 开工指引

> 上一 session（2026-04-20，v1-session2 续作）把 M2 地基批彻底收完 + γ 架构审查
> 交付 + 一堆体感 bug 修完。本 session 开头，context 高，让下个 session 接手。
>
> 上份 handoff: `docs/handoff/v1-session2.md`（留着）。

---

## 1 · 10 分钟必读（按顺序）

1. `CLAUDE.md` · 公理 + 常见陷阱（5 min）
2. **`docs/architecture-audit-v1.md`** · 架构现状盘点 + Gap + Debt 分级（5 min）
3. **`docs/architecture-v1.1-roadmap.md`** · M2-M5 路线图（5 min）
4. 本文 §3 "下一动作" + §4 "P2 召回设计已定"（5 min）
5. `docs/v1.1-agenda.md` · N1-N13 + G1-G8 的完整反馈清单
6. `~/.claude/projects/-Users-e0-7-fuxi/memory/feedback_*.md` · 协作范式（自动加载）

**跳过即踩坑**：
- 跳过 §3 → 不知道 P2 已拍板，会重新向用户问设计
- 跳过 §4 → 实装 P2 时忽略"session 跨多 task" corner，会被 user 再纠一次

---

## 2 · 当前仓库状态

**分支**：`feat/fuxi-v0.1`（下次 ship 合 `main`）

**本 session 全部 commit**（按时间）：

| commit | 内容 |
|---|---|
| `e0c57ae` | γ 架构审查 · 4 份文档（cratewise-inventory / event-flow / audit / roadmap） |
| `5304eab` | M2.1 消息队列 pending + M2.2 codex 接入 + M2.3 玄女订阅心法 |
| `860c377` | drain 漏洞修 + 僵尸 user-turn task 修（TUI Submit::Xuannv 改走 intervene） |
| `08358fa` | codex 默认模型改空串（不再硬编码 gpt-5.1-mini） |
| `074ab2e` | parser 双发 bug 修（AssistantText/ResultSuccess 去重） |
| `df4179a` | M2.4 GC/TTL + M2.5 Extractor 实装（并行 2 agent + 主线整合） |
| `fbba2ec` | 撤回 spawn_worker 去重（违反"起 N 个就真起 N 个"直觉） |
| `1e6db4e` | shutdown_agent 豁免玄女（防 GC 10min 把她杀了） |

**门禁**：fmt ✓ / clippy -D ✓ / **371 tests 全绿**

**M2 完成度**：5/5 ✅（D1-D5）

---

## 3 · 下一动作（**已拍板，直接开工**）

用户上一句："选1。你先别急开。上下文不健康了。"

"选 1" = **P2 召回机制 · task_id 主入口 + role shortcut 双入口**。设计讨论完毕，实装未动。

### 3.1 P2 召回 · 实装清单

估时 **一 session（可能跨半 session 跑 agent，半 session 主线整合）**。

**a. `fuxi_core::Agent` trait 扩展**
- 加 `fn session_id(&self) -> Option<String> { None }` 默认返 None
- `CcAgent` override 返 `cli_session_id().await`（注意 async 转 sync 难办——要么改成 async fn，要么 inner 里缓存 sync 可读字段）
- `CodexAgent` / `StubAgent` 继承默认 None（codex spawn-per-dispatch 无持久 session）

**b. dispatch pump terminal 时入库 session_id**
- 位置：`Fuxi::dispatch` 的 pump，看到 `TaskStateChanged::Done` 时拿 agent.session_id() + ev.meta.task（task_id），入 oracle
- 记录格式：
  ```
  subject   = task-<task_id>
  predicate = session_id
  object    = <cli_session_id>
  source    = "auto:dispatch-pump"
  ```
- 一个 session 可能对应多条 fact（多个 task 指向同 session）—— 这**是**设计不是 bug，见 §4

**c. 新 CLI 命令 · 两个 flag**
- `fuxi spawn --recall-task <task_id> --role <role>` → 查 `task-<id>` 的 session_id fact，构造 `CcLaunchConfig { resume_session_id: Some(x), .. }`
- `fuxi spawn --recall-role <role>` → 查该 role 最近活动的 session（按 oracle 的 updated_at DESC 取 LIMIT 1）
- 两 flag 互斥 + 和 `--role` 组合；不指定就走普通 spawn
- 实装位置：`crates/fuxi-cli/src/subcommands.rs::SpawnArgs`

**d. 玄女 skill 教用法**
- 改 `skills/xuannv/instructions/tool-map.md` 和 `dispatch-protocol.md`
- 加一段"召回"：用户说"重做刚才那个任务" / "召回鲁班" → 用 `--recall-task` 或 `--recall-role`
- 解释：session 是"那次对话线"，非"单 task 切片"；resume 后会看到之前所有 history

**e. TDD 先写测试**
- `cc_agent_exposes_session_id_after_init`
- `dispatch_pump_writes_task_to_session_mapping_on_done`
- `spawn_with_recall_task_flag_sets_resume_session_id`
- `spawn_with_recall_role_picks_latest_session_for_role`
- 召回 gated E2E 建议加：真跑两轮 cc 验证 session 跨 agent 实例 resume

### 3.2 用户反驳点提前排雷

用户明确说："我只是提议，你别无脑迎合我"。设计已经走过一次**反驳 + 修正**：

- 他提议 task_id 召回 → 我指出 session 跨多 task 的 corner → 两个都做（task 主 + role shortcut）
- 不要再把 task_id 吹成"只恢复这一个 task 的 context"——cc `--resume` 是 session 粒度

### 3.3 开工前跑一次门禁基线

```bash
cd /Users/e0_7/fuxi
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast 2>&1 | grep -E "^test result:" | \
  awk '{p+=$4; f+=$6} END {printf "passed: %d  failed: %d\n", p, f}'
# 期望：passed: 371  failed: 0
```

---

## 4 · P2 召回 · 已约定的设计点（不要再向用户重问）

**核心约定**：

1. **task_id 是入口，session_id 是值**。玄女/用户用 task_id 指定召回对象，底层转 session_id 给 cc `--resume`。
2. **session 跨多 task**：同一 agent 做 A / B 两 task 共享一个 session。召回任何一个，拿到的 context 含两个 task 的 history。这不是 bug 是 cc 本身设计。
3. **策府 fact schema**：`oracle_facts` 加约定 subject pattern `task-<id>`，predicate `session_id`。查询 `query_one("task-<uuid>", "session_id")`。
4. **多 task 指向同 session** 是正常的 —— oracle insert 不去重（append-only 本来就允许），用 updated_at 取最新的即可。
5. **`--recall-task` vs `--recall-role`** 两个互斥 flag。不合并成 `--recall <x>` 猜类型。
6. **codex 不支持召回**：返 None 走普通 spawn（codex 本来就是 spawn-per-dispatch 无持久 session）。
7. **玄女 skill** 加"召回"场景教学，不增新公理。

---

## 5 · P2 之后的 roadmap 剩余

按优先级（`docs/architecture-v1.1-roadmap.md` 权威）：

### M3 · 命名规整（🟡，1 session）

破坏性 rename 一次打包：
- D7 `fuxi resume` → `fuxi task unblock`
- D8 `skills/<role>/SKILL.md` → `roles/<role>/ROLE.md`（概念和 Claude Code 的 skill 撞名）
- D6 CLI 参数 charter 统一（`--id`/`--to`/`--role` 一致化）
- D9 `fuxi-a2a::AgentCard` 改 `wire::AgentCard`
- D10 fuxi-cli 的 `unwrap()` 清理
- D11 孤儿事件清理（6 publisher-orphan + 9 subscriber-orphan，见 `docs/audit/event-flow.md`）
- D12 `fuxi kill --id` / `fuxi events` 补洞

**警告**：M3.2 (skill→roles) 会破坏 `~/.fuxi/skills/` 用户数据，要加 migration：启动时 mv + 备份。

### M4 · 体验升级（🟡🟢，1 session）

- **U1 视觉方案 A** · 竖条只在首行 + 时间戳锚点 + 降饱和度（teal→sapphire / mauve→lavender）+ 消息间空行。治"清爽但费眼"
- D13 intervention task 视觉差异化（dim + 不同 icon）
- D14 让贤（ConversationHandoff）激活还是拆 · 用户拍板（推荐激活）
- D16 slash `/help` + `@agent` 命令面板（C5 research 里列过，没做）

### M5 · v1.2 大改（🟢，2-3 session）

- D15 单栏 TUI + 事件嵌入对话（cc 风格 transcript）—— 对应用户"TUI 太繁复，学 cc/opencode"大需求
- D17 启动 ASCII art（`fuxi` 回车后来个 banner）
- D18 Resume 真回放 dialogue history（持久化 `dialogues` 到 SQLite）

---

## 6 · 本 session 踩过的坑（加到 CLAUDE.md 常见陷阱）

### a. parser 双发（`074ab2e`）
cc 的 `assistant` 流 message 和 `result` 末尾会给**同一段文本**两次。parser 两处都发 `AgentResponded` → TUI 显示两遍。fix：TranslateState 加 `responded_this_turn` 标位。**下次加 EventKind 或 parser 改动时留意"同一信息被翻译两次"**。

### b. TUI Submit::Xuannv 必须走 intervene 不是 dispatch（`860c377`）
用户每按 Enter 发消息直接 `Fuxi::dispatch(xuannv, Task::new("user-turn"))` 会堆僵尸 task。**正解**：走 `Fuxi::intervene`，idle 自动 degrade 单 dispatch，busy 入 pending queue（M2.1）。Decision 04 的 degrade title 也改成 "user-turn"。

### c. dispatch pump terminal 不能立即 break（`860c377`）
M2.1 drain 在 terminal 后把 pending queue 塞 cc 起新 turn；旧 pump 早 break 让 rx drop → 新响应丢。fix：terminal 后 500ms timeout 等；新事件来重置 saw_terminal；超时才 break。

### d. codex `DEFAULT_MODEL_FALLBACK` 必须空串（`08358fa`）
硬编 `gpt-5.1-mini` 对 ChatGPT 账号用户失败。默认空串让 codex 自选；API key 用户 `export FUXI_CODEX_MODEL=<model>`。**已在 CLAUDE.md 有提示，但默认值遗漏了，这次修了**。

### e. shutdown_agent 必须豁免玄女（`1e6db4e`）
`IdleGcTask` 10 分钟 idle 会杀玄女（她 role=xuannv 对 GC 无差别）。治本：`Fuxi::shutdown_agent` 开头比对 `xuannv_id()`，命中返 Ok 静默 noop，warn 日志。**新增任何 shutdown 路径（如 `fuxi kill --id`）都吃这个豁免，不要走旁路**。

### f. spawn_worker 去重是反直觉的（`fbba2ec`）
一度在 `spawn_worker` 里塞 `find_idle_by_role` 复用，用户质疑"不能起三个鲁班？"。**spawn = 新建**，复用职责在 `dispatch_to_any`。GC 负责回收。

### g. Extractor 依赖方向（`df4179a`）
fuxi-memory 不能依赖 fuxi-orchestrator（否则循环）。`FactExtractorSpawner` trait 定义在 memory，impl 放 **fuxi-cli**（顶层依赖全部 crate）作为 adapter `FuxiExtractorSpawner`。未来类似 "memory 需要调 orchestrator" 都按这 pattern。

---

## 7 · 用户协作范式 · 必记

（已在 memory 里，但强调一次）

1. **用户会反问 + 质疑实装**。例："spawn 三次只得一个？"" shutdown_agent 会杀玄女吗？""session 对得上吗？" → 一律**先认真质疑自己设计**，有理有据就改，不无脑改也不无脑顶。
2. **用户叫停 context** 时真停 —— 不要"最后做一件事"。他让做 handoff 就做 handoff，不偷偷加 P2 第一步。
3. **文档驱动**：他看 `docs/`，看 test-reports，看 handoff。写好文档下次接手快。
4. **TDD 硬要求**：新功能必须先写失败测试。地基批全部遵循。
5. **并行 agent**：独立 crate 边界 + 清晰 scope 分派。本 session α (M2.4) + β (M2.5) 并行成功模板可复用。
6. **别无脑迎合**：用户原话。他希望协作者不是 yes-man。

---

## 8 · 工作单元 · 开工建议

开 P2 有两种拆法：

### 方案 X · 主线串做（约 1 session）

Step 1：Agent trait 加 `session_id` + CcAgent impl（一个 PR）
Step 2：dispatch pump 入 oracle fact（一个 PR）
Step 3：CLI flag `--recall-task` / `--recall-role`（一个 PR）
Step 4：玄女 skill 更新（一个 PR）

好处：每 step 独立验证，不 big bang。

### 方案 Y · 并行 agent（半 session）

- agent α：Agent trait + CcAgent session_id 暴露 + dispatch pump 入库
- agent β：CLI `--recall-task` / `--recall-role` + clap args + 查 oracle 逻辑
- 主线：玄女 skill 更新 + 整合

**推荐 Y**，本 session α+β 并行的成功模板可复用。

---

## 9 · 快速判断指标

下个 session 开工前：

- [ ] `git log --oneline -3` 看到 `1e6db4e`（本 session 最新）
- [ ] `cargo test --workspace` 得 371 passed / 0 failed
- [ ] `docs/architecture-v1.1-roadmap.md` M2 全 ✅（我未必在 markdown 里真打 ✅，但文本上 M2.1-5 都有 commit 落地记录）
- [ ] 用户若贴新 bug，先核对是否已在 §6 常见陷阱列过

ship 判据（v1.1，摘 roadmap）：M2+M3+M4 全绿 + 用户手测 10 条全过。当前 M2 完、M3/M4/M5 未开。

---

**给下个 session 一句话**：上来别犹豫，P2 召回已设计好，按 §3.1 清单 TDD 开工；踩坑前看 §6。

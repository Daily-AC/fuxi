# Handoff · v1 · Session 11 开工指引

> 上一 session（2026-05-05）核心是**记忆系统重构 v2**——按 ICML 2026
> arXiv:2604.14004 *Memory Transfer Learning* 论文重做：三表严格分流 +
> 仓颉自动 insight 提取 + spawn 注入桥。开了 3-worker agent team 干完，
> 后续修了多个 bug 才稳定。
>
> 上一份 handoff：`docs/handoff/v1-session10.md`（保留）。

---

## 1 · 5 分钟必读

1. `CLAUDE.md` · 七公理 + 「常见陷阱」
2. 本文 §2「memory-v2 ship 了什么」§3「v2.1 待办」§4「答辩 narrative 待对齐」
3. （选读）`docs/decisions/`、`crates/fuxi-orchestrator/src/insight_extractor.rs`、
   `roles/cangjie/instructions/extraction.md`

---

## 2 · 上 session ship 了什么

### 当前 main HEAD = `6dd9f23` ✅ 全绿，已部署 home

按 commit 顺序：

| commit | 内容 |
|---|---|
| `fa16e93` | feat(memory-v2): 三表分流 + 仓颉 insight 提取 · 论文 arXiv:2604.14004（21 files / 3235 insertions / 主 ship） |
| `07241f6` | fix(memory-v2): 仓颉死循环 · sentinel 黑 cangjie + insight_extractor 黑 xuannv |
| `706349d` | fix(memory-v2)+ux: cangjie sentinel role gate · 仓颉默认 [] · 系统消息折叠 · tool 调用分组 |
| `4f4e7ed` | fix(pwa): TaskThreadPage Thread sub-component 拿不到 ctx · history fold 抛错任务详情空白 |
| `d09daf6` | fix(pwa): tool_group head 加 who + 时间戳 · 跟 worker bubble 视觉对齐 |
| `6dd9f23` | fix(pwa): tool_group 用 lookupMember · agent id 前缀差异不再丢 role_display |

### A · 三表严格分流（safety boundary）

| 表 | 抽象度 | 谁写 | 谁读 | spawn 注入 |
|---|---|---|---|---|
| `oracle_facts` (沿用) | Trajectory | 平台 + 玄女 | 玄女 only | ❌ 不注入 |
| `user_profile` (新) | Summary | 玄女 / 用户显式 | 玄女 + 门客 | ✅ 身份卡 ≤200 字 |
| `hetu_patterns` (扩展) | Insight | 仓颉自动 + 玄女手动 | 玄女 + 门客 | ✅ 同 role 最近 5 条 |

**论文核心**：低层 trace 误注入门客 prompt 是 **negative transfer**（不只浪费，
是有害）—— 三表分流不只清洁度问题，是 safety。`render_memory_addendum` 测
`does_not_leak_oracle_keywords` 防字段名泄漏。

### B · 仓颉（cangjie）· 史官 / insight 提取者

- `roles/cangjie/{ROLE,extraction,judge}.md` 完整 prompt + 反模式清单
- `InsightExtractorTask` (orchestrator/src/insight_extractor.rs) 订阅
  `TaskStateChanged{Done}` → spawn cangjie 提取 → LLM-as-judge 单条打分
  → ≥0.6 入 hetu
- `FUXI_INSIGHT_EXTRACTOR_ENABLED` default true（论文支持开）
- 单测 16 条 ✓
- 接通点：`crates/fuxi-cli/src/im.rs` IdleGcTask spawn 后紧邻

### C · spawn 注入桥（safety boundary）

`sentinel_addendum::inject_role_memory_cc / inject_role_memory_codex`：
- 黑名单 xuannv（接收方）/ extractor（幕后）/ cangjie（自吞循环）严格 noop
- 注入文案：「## 用户身份卡（必读）」+「## 你这个角色的历史心法（论文：抽象度决定可迁移性）」
- 严格不注入 oracle_facts 任一字段
- recent_for_role 按 abstraction_score DESC + created DESC 排序

### D · 死循环修复（双重保险，2026-05-05 home 实测撞过）

cangjie 死循环 root cause 链：cangjie cc 即便不读 sentinel 教学，也会从
trajectory 里 luban 的 sentinel JSON 样本**自学复读** → cc parser 检测到
`_fuxi:request_review` 字串 emit AgentRequestReview event → bridge 转给
玄女 → 玄女 user-turn task done → InsightExtractorTask 又起 cangjie。

**三层防御**（任一层失效另一层兜底）：
1. **sentinel addendum** 黑名单加 cangjie（`should_inject_for_role`）—— cangjie
   不读 sentinel 教学，没看到例子不会自学
2. **insight_extractor** 内部 role filter 加 xuannv —— 玄女 task done 不再
   触发抽取
3. **cc parser TranslateState** 加 role 字段 —— cangjie/extractor role 时
   sentinel 路径整段禁用，即便它输出 sentinel JSON 也不 emit
   AgentRequestReview，走 AgentResponded 透传

测试：parser 39/39 ✓ · sentinel 23/23 ✓ · insight 16/16 ✓

### E · PWA UX 改进

| 改 | 实装 |
|---|---|
| 系统消息折叠 marker | `SystemMessageRow` 默认 `─ 📋 待审 ▸ ─` 横线，点击展开大卡 |
| tool_call group 折叠 | `ToolGroupCard` + `groupConsecutiveToolCalls` 渲染层 helper；连续同 agent ≥2 条折成 group；head `鲁班 17:16 🔧 N 个工具调用 ›` |
| 仓颉 prompt 不凑数 | `roles/cangjie/instructions/extraction.md` 改"默认 []"；多数任务 0 条；翻车 1 条；2 条复合；>2 罕见 |

PWA 单测 326/326 ✓

---

## 3 · v2.1 待办（玄女自诊断 + 用户反馈）

按性价比排（玄女自己提出大多数）：

### 🟡 必做（影响 cc 钱 / 体验）

1. **batch judge**：当前 1 task done 起 1+N 只 cangjie（1 extract + N judge）。
   合并 judge 进一只 cc 单 prompt，调用降至 1+1。论文严格度损失最小。
2. **task_type=validation/review 类豁免**：玄女自指 review 也被抽。当前
   xuannv role 已豁免（解决了），但 task_type 维度过滤未做——非玄女角色
   做 review 类 task 仍会被抽。

### 🟢 优化（不阻塞）

3. **judge 阈值 calibrate**：当前 ≥0.6 放行，但实测"复用优先原则可迁移
   但偏泛"也给 0.7。等积累 50+ 打分样本后调阈值或加 prompt hint。
4. **throttle 短窗口**：同 (role, task_id) 30s 内重复抽取压制，作为 #1 失效
   的兜底安全网。
5. **`fuxi agent dump-prompt <id>` 调试命令**：让"prompt addendum 含某段"
   变成 grep 断言（玄女测试方法建议）。
6. **跨会话验证**：关玄女重启 → 新玄女问"我叫什么" → 看是否从 user_profile
   注入读到。论文核心闭环验证。

### 🟢 用户主动写记忆的教学

7. user_profile 现在是**手动**写（玄女主动 record 用户身份）—— 实测
   home 上的 `user_profile` 还是 0 条（用户测过"记一下我叫以琳..."
   但玄女应该写了几条 fuxi profile set，待用户自己验）。
8. 可加自动从 trajectory 推断 user_profile 候选（类似 cangjie 但抽身份事实
   不是 insight），LLM-as-judge 把关后入 user_profile。

---

## 4 · 答辩 narrative 对齐（**重要 · 仍未对齐**）

上一 session 用户邀请"一起对齐"，我给出取证版"伏羲是什么"+ 4 个待校准
问题。**用户没回答这 4 个**，会话被 memory-v2 重构占满了。

下一 session 应该主动问回这 4 条：

1. **核心使用场景**：日常最常用哪 2-3 个 tab？答辩演示主线 demo 是哪条？
2. **谁是用户**：只你一人 / 你 + 朋友 / 给陌生人用？决定 PWA 抽象层级
   （现在很多假设是"用户 = 你"，比如硬编 home 域名）
3. **下一里程碑**：把已有 7 件事**做透**（深度），还是**加新场景**（广度）？
4. **答辩 30min**：核心创新点 / 技术挑战 / 已 ship 能力 demo —— 哪个权重最高？

memory-v2 是真·论文素材：可讲"借鉴最新研究 + 接受反直觉结论 + safety
boundary 真实落地（实测撞过死循环 + 双重防御）"。

---

## 5 · 部署快照（home 现状 2026-05-05）

```
binary:    /home/e0-7/.local/bin/fuxi  (含 6dd9f23，三表 + 仓颉 + safety boundary)
PWA:       /home/e0-7/.local/share/fuxi/im-web  (同上)
events.db:
  oracle_facts:    330+ (dispatch-pump 自动累积，不进门客 prompt)
  user_profile:    0    (等玄女主动 record；用户测过但具体 entries 未验)
  hetu_patterns:   ~10+ 条 (仓颉死循环修前累积的多 + 修后 1 个 task 抽 3 条)
玄女:       fresh spawn 后含 sentinel + memory + xuannv routing 三段 addendum
public URL: https://im.qmledmq.cn:8443
GitHub:     repo public，最新 release v0.1.5（自动 patch+1 已工作）
```

---

## 6 · 协作笔记（写给下个 session）

- 用户偏好已落 memory：feedback_full_bypass / feedback_keep_going /
  feedback_no_ceremonies / feedback_team_lead_batch_dispatch / feedback_tdd_required
- **用户 feedback_team_lead_batch_dispatch 关键**：team-lead 一次把 spec 和
  dep 全给 worker 让其自驱，不要 ack-派活 round-trip。本 session 验证
  对——alpha/beta/gamma 几乎完美自驱，只 alpha 一处需要催。
- **用户希望被反驳**：上 session 我推 ollama embedding，被论文打脸；用户对
  论文 3.7% 提升 + "高级检索不如朴素检索"反直觉结论敏感，**记忆架构选型不
  要轻易加新基础设施**（FTS5 关键字够用 + 写入侧 abstraction 才是关键）。
- **PWA 改 `Thread` 子组件时注意 closure scope**：Thread 是 module-level
  组件，TaskThreadPage 内部 createMemo 拿不到，必须 props 传 Accessor。
  bug `4f4e7ed` 撞过，注释也写过别再撞。

---

## 7 · 改 EventKind 清单（沿用，不变）

加新 EventKind 变体一定要同步多处，否则 clippy `-D warnings` 会一处处报：

1. `crates/fuxi-core/src/event.rs` —— 变体定义 + serde 字段
2. round-trip 测块 —— `tag_and_roundtrip` 加 case
3. `crates/fuxi-events/src/store.rs::kind_tag` —— 持久化标签
4. `crates/fuxi-firehose/src/{hub,tui}.rs` —— Hub 转发 + summarize/color
5. `crates/fuxi-cli/src/subcommands.rs::event_summary` —— CLI 文字
6. （若入对话视图）`crates/fuxi-im/src/handlers/{tasks,workers}.rs::*_visible`
7. （若 PWA 渲染）`crates/fuxi-im/web/src/messages.ts` 三 reducer + 渲染 switch

memory-v2 没加新 EventKind（hetu_patterns 改 schema 不动 EventKind 类型）。

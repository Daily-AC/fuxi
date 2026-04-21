# Handoff · v1 · Session 4 开工指引

> 上一 session（2026-04-20 ~ 04-21）把 P2 召回 L2 闭环、M3 命名规整批全做完、
> 修了 M2.5 extractor 的雪崩 + 实例堆积。406 tests 全绿。
>
> 上份 handoff: `docs/handoff/v1-session3.md`（保留）。

---

## 1 · 10 分钟必读（按顺序）

1. `CLAUDE.md` · 公理 + 常见陷阱（5 min）
2. **`docs/decisions/07-recall-scope.md`** · P2 召回 L2 的 scope + 边界 + worktree 持久化决定（3 min）
3. **`docs/cli-charter.md`** · M3.3 落档的 CLI 规约（命名 / flag / 弃用流程，2 min）
4. **`docs/architecture-v1.1-roadmap.md`** · 看 M4 / M5 还剩什么（3 min）
5. 本文 §3 "下一动作" + §6 "本 session 踩坑/修过"（5 min）
6. `~/.claude/projects/-Users-e0-7-fuxi/memory/` · 用户协作范式 feedback_*（自动加载）

**跳过即踩坑**：
- 跳过 §3 → 不知道 M4 路线，会重新向用户问优先级
- 跳过 §6 → 重蹈 extractor 雪崩 / spawn 去重拆掉的覆辙

---

## 2 · 当前仓库状态

**分支**：`feat/fuxi-v0.1`（v1.1 ship 时合 `main`）

**本 session 全部 commit**（按时间）：

| commit | 内容 |
|---|---|
| `8feb7eb` | P2 召回 L1 · wire 打通（task_id + role 双入口） |
| `d5a8e02` | P2 召回 L2 · trait 通用化 + worktree 复用 + e2e 闭环 |
| `ac9ad66` | M3 批 1 · kill/events/task unblock + 孤儿事件 + unwrap 清理（3 并行 agent） |
| `ae352de` | M3 批 2 · a2a wire 命名空间 + CLI charter 落档 |
| `8be9ed1` | M3.2 · skills/`<role>`/SKILL.md → roles/`<role>`/ROLE.md（大破坏 rename） |
| `48473bc` | fix · 桥过滤 INTERNAL_ROLES（修 extractor 雪崩抄送给玄女） |
| `ab5ef90` | fix · extractor_hook 用 dispatch_to_any 复用 idle（修堆 N 个 extractor） |
| `f2142c5` | chore · extractor 默认关，玄女按 prompt 手工 record |

**门禁**：fmt ✓ / clippy -D ✓ / **406 tests 全绿**

**M2 完成度**：5/5 ✅
**M3 完成度**：7/7 ✅（M3.1-M3.7）
**P2 召回**：L2 闭环（cc real-world e2e 跑通：第二轮 luban 答出第一轮 history 的文件路径）

---

## 3 · 下一动作（**已拍板，直接开工**）

用户最后一句："该收尾了。"

下个 session 推荐起手：

### 3.1 用户手测验收 v1.1（必做，0.5 session）

P2 召回 + M3 重构 + extractor 关掉，用户没全部跑过。建议第一件事：

```bash
# 1. 退当前 REPL 重启（玄女已被 supersede 走新 session）
fuxi
# 跟她说"我叫以琳，爱喝冰美式" → 看她是否调 fuxi memory record
# 派 luban 干件小活 → 验 P2 召回
fuxi list  # 干完后看：应该只有 luban + 玄女，没有堆 extractor
exit
fuxi  # 重启
# 跟她说"召回刚才那个 luban" → 看她是否走 fuxi spawn --role luban --recall-role luban
```

如果用户验收通过，标记 v1.1 ship-ready（缺 M4 但功能完整）。

### 3.2 M4 · 体验升级（1 session）

按 `architecture-v1.1-roadmap.md §M4`，4 个子项：

- **M4.1 · U1 视觉方案 A**（疲劳解药）—— 竖条只首行 + 时间戳锚点 + 降饱和度 + 消息间空行。立竿见影
- **M4.2 · D13 · intervention 视觉差**—— intervention task 用 dim + 不同 icon 区分正式 task
- **M4.3 · D14 · 让贤决策**—— 拍板「激活」（铸牒司临时让贤场景）vs「拆」。**用户须给意见**
- **M4.4 · D16 · slash/@ 命令面板**—— 输入框 `/` 弹 slash commands，`@` 弹门客切 active

**并行拆法**：U1+D13（UI 一路）/ D14（拍板后做）/ D16（独立面板）—— 三路 agent team，主线整合。

### 3.3 ship 判据复检

`architecture-v1.1-roadmap.md` 末尾 10 条用户验收清单，本 session 后状态：

- [x] busy 时连发 3 条消息全收到（M2.1）
- [ ] `fuxi spawn codex luban` 能起 + 派活（**未真测；β agent 实装但未 e2e**）
- [x] 玄女不再 poll fuxi status（M2.3 skill 改 + bridge 注入）
- [x] 连开 fuxi 5 次 spawn 3 次 → list 只 1 个；10 分钟不用自动回收（M2.4）
- [ ] 跟玄女聊喜好 → 关 fuxi → 开新会话她自然引用（**extractor 现在关了，靠她手工 record**）
- [x] `fuxi task unblock` 替代 `fuxi resume`（M3.1）
- [x] `fuxi kill --id` 单杀（M3.7）
- [ ] 对话视觉方案 A 应用后连看 30 秒不累（**M4.1 待做**）
- [ ] `/help` 有（**M4.4 待做**）
- [ ] `@agent` 切 active 有（**M4.4 待做**）

**结论**：M4 完成 + codex e2e 验过 + extractor 改完后玄女自我 record 真工作 → v1.1 可 ship。

---

## 4 · 本 session 关键决策（**不要再问用户重做**）

### 4.1 P2 召回 = 整个工作环境（Decision 07）

不只是 cc session uuid。worktree + cli session 一起记。

- `RecallContext { agent_id, task_id, role, worktree, cli_session_id }` 是通用契约
- codex 可走 worktree-only 召回（无 session）
- `Fuxi::shutdown` / `shutdown_agent` **不**销毁 worktree（保留召回 stash；物理清理留给 v1.2 `fuxi worktree clean`）
- `WorkspaceHandle.borrowed=true` 让召回 spawn 不被二次 destroy

### 4.2 skills/SKILL.md → roles/ROLE.md（M3.2）

**不**改 crate 名（`fuxi-skills` 留着，v1.2 再考虑）。文件路径变了；公共 API 名（`LoadedSkill`/`SkillFrontmatter`）暂留减少破坏面。

`load(role)` 双名兼容：先 ROLE.md 再 SKILL.md fallback + warn。`migrate_user_dir()` 启动期一次性 mv `~/.fuxi/skills` → `~/.fuxi/roles`。

### 4.3 自动 extractor 默认关（commit `f2142c5`）

每 task Done 都跑 cc 抽取太烧 + 噪音多。**机制改成 prompt 驱动**：玄女自己用 `fuxi memory record` 手工入。`FUXI_EXTRACTOR_ENABLED=1` 可恢复。

判断流程在 `roles/xuannv/instructions/tool-map.md`「什么时候主动 record」段——已重写。

### 4.4 桥过滤 INTERNAL_ROLES（commit `48473bc`）

`bridge.rs` 加 `INTERNAL_ROLES = &["extractor"]`——内部 role 的 TaskDone/AgentDead **不抄玄女**。否则 extractor 完活每次唤醒玄女 → cc transcript 被淹没。

**未来加新内部 role**（如 self-cc、watcher 等）必须把 role 标签加到这个数组。

### 4.5 extractor_hook 用 dispatch_to_any（commit `ab5ef90`）

`spawn_worker` 没有去重（commit `fbba2ec` 拆掉）。复用 idle 必须走 `Fuxi::dispatch_to_any(role, task, profile, kind)`——它原子地"找 idle 同 role / 否则 spawn 新的"。**任何"按 role 起短寿命门客"的代码都用这条路径**。

---

## 5 · 用户当下环境状态

- **`~/.fuxi/memory.db`**：xuannv session_id 已 supersede（valid_until 已设），下次 `fuxi` 启动玄女走新 session（无 resume banner）
- **`~/.claude/projects/-Users-e0-7-fuxi/d60d73a2-*.jsonl`**：旧玄女 cc session 文件还在（被 extractor 雪崩污染过的那条），用户没删——审计用
- **`.fuxi/worktrees/`**：之前 P2 e2e 留的 worktree 已 prune，状态干净
- **shelf**：用户上次进 REPL 时看到 N 个 extractor，那时进程内的；退 REPL 后 daemon 关、进程清空——**重启 fuxi 后会是新干净状态**
- **`/tmp/fuxi.log`**：TUI 模式 stderr 重定向位置，调试看这

---

## 6 · 本 session 踩过的坑（已加到 CLAUDE.md / 决策 doc）

### a. cc session 按 cwd 索引 → P2 召回门客必须复用 worktree

`~/.claude/projects/<mangled-cwd>/<sid>.jsonl`。fuxi 每次 spawn 新 worktree → cwd 不同 → cc resume 即死「No conversation found」。修法见 Decision 07 L2。

### b. shutdown 销毁 worktree 让召回失效

历史"agent 死亡 = worktree 回收"和召回 stash 冲突。改成 shutdown 只 stop process，worktree 留地上。物理清理留 v1.2 `fuxi worktree clean`。

### c. cc 默认 `--no-session-persistence` → resume 拿不到 session 文件

`CcLaunchConfig` 在 `resume_session_id` 和 `session_id` 都 None 时加这 flag → cc 不落 session 文件 → sink 记的 session_id 第二轮 resume 即死。
**daemon spawn_by_role 强塞 session_id = uuid::new_v4()** 强制 persist。

### d. M2.5 extractor 雪崩抄送 + 实例堆积（本 session 修）

两个独立 bug 同时出：
- bridge 没过滤 extractor TaskDone → 每次抄送给玄女 → cc transcript 噪音
- extractor_hook 用 spawn_worker 而非 dispatch_to_any → 每轮真起新 extractor

修法分别在 `48473bc` 和 `ab5ef90`。**任何新"自动后台跑的内部 role"必须**：
1. 添加到 `bridge.rs::INTERNAL_ROLES` 让玄女不被噪音淹
2. 用 `Fuxi::dispatch_to_any(role, ...)` 复用 idle 而非 `spawn_worker`

### e. 自动抽取太烧/噪音 → 默认关，prompt 驱动

每 task Done 都跑 cc 抽取的设计当时没真用过——一上线就发现成本/噪音比都不行。改让玄女按 prompt 判断时机手工 `fuxi memory record`。

**教训**：自动后台 cc 调用要默认 off + 玄女有手工 fallback；不要 default on。

### f. M3.2 rename 时记得 fuxi-skills crate 加 tracing dep

`migrate_user_dir` 用 `tracing::warn!` 但 fuxi-skills 之前没 tracing 依赖。同样的，**新加用 tracing 的 crate 都要在 Cargo.toml 加 `tracing.workspace = true`**——别等 cargo build 报 unresolved module。

---

## 7 · 用户协作范式 · 必记（沿用 session3）

1. **用户会反问 + 质疑实装**。本 session 例：「extractor 这个门客是啥」「每轮都抽么」——用户读 transcript 抓出 bug 比我自己跑测试还准。要尊重她的实测发现
2. **用户叫停 context** 时真停 —— "该收尾了"就写 handoff，不要再开新坑
3. **文档驱动**：他看 `docs/`、看 commit message、看 handoff。写好下次接手快
4. **TDD 硬要求**：新功能必须先写失败测试。本 session 全部遵循（每个 fix 都有回归测试）
5. **并行 agent**：独立 crate 边界 + 清晰 scope 分派。本 session γ/δ/ζ 三路并行成功（γ 中途 API overloaded 但落地了大部分代码，主线接尾）
6. **别无脑迎合**：用户原话。她希望反驳要"有理有据就改，不无脑改也不无脑顶"
7. **prompt > 代码**：能 prompt 解决的不加新命令（本 session extractor 默认关 + 玄女 skill 教学就是范例）

---

## 8 · 工作单元 · 开工建议

下个 session 开工建议顺序：

### Step A · 用户手测验收（30 min，主线做）

跑 §3.1 清单。问题贴回来再修。

### Step B · M4 体验升级（1 session）

并行 agent team：
- **agent ε**：M4.1 U1 视觉方案 A（改 firehose tui.rs / theme.rs）
- **agent ζ**：M4.2 D13 intervention 视觉差（细化 ε 的工作）
- **agent η**：M4.4 slash/@ 命令面板（改 repl.rs / 加 click_registry 路径）
- **主线**：M4.3 让贤决策（**先和用户拍板**激活 vs 拆，再做）+ 整合

### Step C · v1.1 ship（0.5 session）

- 跑完 10 条用户验收
- 写 v1.1 release note
- merge `feat/fuxi-v0.1` → `main`
- tag `v1.1.0`

### 留给 v1.2/M5

- D15 单栏 TUI（cc 风格 transcript）—— 大改，2-3 session
- D17 启动 ASCII art
- D18 Resume 真回放 dialogue history
- `fuxi worktree clean` 命令（清 P2 召回 stash）
- crate rename `fuxi-skills` → `fuxi-roles`（M3.2 留的债）
- 召回 fact 的 GC（哪些 worktree path 已不存在了应清掉相应 fact）

---

## 9 · 快速判断指标

下个 session 开工前：

- [ ] `git log --oneline -3` 看到 `f2142c5`（本 session 最新）
- [ ] `cargo test --workspace` 得 406 passed / 0 failed
- [ ] `~/.fuxi/memory.db` 里 `xuannv` subject 的 `session_id` fact 已 supersede（用户重启后会生新）
- [ ] `roles/xuannv/instructions/tool-map.md`「什么时候主动 record」已重写
- [ ] M3 全 ✅；M4/M5 未开
- [ ] 用户若贴新 bug，先核对是否已在 §6 列过

ship 判据（v1.1）：M2 ✓ + M3 ✓ + **M4 待做** + 用户验收 10 条（4 条已过、4 条待 M4、2 条待 e2e）。

---

**给下个 session 一句话**：M3 命名规整全干完，M2.5 extractor 默认关 + 玄女自管记忆。下一步推 M4 体验升级；用户手测先做。踩坑前看 §6。

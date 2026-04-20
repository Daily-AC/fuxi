# Session Review · 2026-04-20（v1 并行 + review + 用户实测）

> 给下个 session 接手人：这份写的是**过程性**（为什么这么做 / 当时选了什么 / 踩过什么坑），不是结论性（那些在 `docs/decisions/`）。
>
> **必读顺序**：
> 1. `docs/architecture-v1.md` · v1 蓝图总图（含命名总表）
> 2. `docs/decisions/` · 6 份关键决策（01-06）—— 每份独立文件，按编号看完
> 3. 本文 · 今天的 context 和决策过程
> 4. `docs/handoff-v1-session2.md` · 下个 session 开工 checklist

---

## 1 · 今天干了啥（按时间）

| 阶段 | 产出 | commit |
|---|---|---|
| R1-R5 并行研究（记忆/招贤/定时/TUI/multi-agent） | 5 份 `docs/research/*.md` | — |
| M1 聚合蓝图 | `docs/architecture-v1.md` | `5160e2f` |
| C1-C5 并行编码（独立 cc 进程 + worktree） | 5 个功能完整合并 | `8795ea2` / `499f54b` / `fcd185e` / `16b97cf` / `d6ed52b` / `2888799` / `98c653d` / `3c68af0` |
| T2 独立 reviewer | 发现 6 个 bug（BUG-1~6） | — |
| Fix-A 主线 + Fix-B/C 并行 | 修 BUG-3/5 + 加系统事件桥 BUG-1/2 | `dd1981d` / `3ad9c95` / `7434dc8` / `6c00a97` / `2524749` |
| 用户实测（凌晨） | 发现 12+ 个 UX bug，重新分类 A/B/C/D | — |
| Fix-D 并行（TUI 大改）+ 主线 3 个 C 类 bug | 进行中… | `ffd5dfa` |

---

## 2 · R → M → C → Fix 三层聚合怎么跑的

### 2.1 叶层（并行起 10+ 单元）

**研究 × 5**（Task subagent，context 隔离 + 便宜）：
```
R1 记忆 → docs/research/memory-survey.md
R2 招贤 → docs/research/skill-management-survey.md  
R3 定时 → docs/research/scheduler-survey.md
R4 三栏 TUI → docs/research/tui-3pane-design.md
R5 multi-agent 编排 → docs/research/multi-agent-survey.md
```

**编码 × 5**（独立 cc 进程 + git worktree，字面满足用户"直接调用多个 cc"要求）：
```
C1: feat/fuxi-install-soul   → cargo install + soul-first skill 重写
C2: feat/fuxi-tui-3pane       → 三栏 TUI + orchestrator 补课
C3: feat/fuxi-memory          → 策府 crate
C4: feat/fuxi-skills-zhaoxian → 点将台 crate
C5: feat/fuxi-scheduler       → 更漏 crate
```

### 2.2 中聚合（我主线 cherry-pick + 解冲突）

按**冲突度**排序合：C1 → C4 → C2（四步）→ C3 → C5。每合一个跑 fmt/clippy/test。

**冲突高发点**：
- `crates/fuxi-cli/src/main.rs` 的 `Command` enum（C1/C3/C4/C5 各加子命令）
- `crates/fuxi-cli/src/repl.rs`（C2 大改 + C5 要起 scheduler）
- `Cargo.toml` workspace members（C3/C4/C5 各加新 crate）
- `Cargo.lock` 自动合并常失败——直接 `rm Cargo.lock` 让 cargo 重生
- `EventKind` 新变体同步更 Firehose + EventStore（公理强约束）

### 2.3 顶聚合（独立 reviewer）

T2 用 `superpowers:code-reviewer` subagent 独立 review，产出 6 个 bug 分级。修法对策：BUG-1/2/3/5 立即修，BUG-4 延 v1.1（`docs/decisions/05`），BUG-6 延 v2。

---

## 3 · 并行起 cc 的关键坑（踩过记下）

### 3.1 CLAUDECODE 嵌套检测
父 cc 进程起子 cc 时若不清 `CLAUDECODE*` env，子 cc 会触发嵌套检测**静默卡死**。`spawn_claude` 已清（`agent-cc/src/spawn.rs`），但**主线 Bash 起 claude 也要清**：
```bash
env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT -u CLAUDE_CODE_NO_FLICKER \
    -u CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS -u CLAUDE_CODE_EXECPATH \
    NO_PROXY="127.0.0.1,localhost" \
    nohup claude -p "$PROMPT" --output-format stream-json ... &
```

### 3.2 Clash / Surge TUN 代理把 127.0.0.1 吞了
本机 VPN 的 TUN 模式把 localhost 连接也代理走，cc 反连 WS 时 SYN 被拦 → 30s timeout。**根治：`NO_PROXY=127.0.0.1,localhost`**。sia/src/core/cc-process.ts:666-667 也是这么做的——我之前误判为"claude 2.1.114 不支持 --sdk-url"走了 8 小时弯路（见 `docs/session-review-2026-04-19.md` 后半）。

### 3.3 teammate 漏 commit
Fix-C 干完没 `git commit`，只写了 `/tmp/fuxi-fc-done.txt` + rev-parse HEAD 返回的是 base commit。我识别到 hash 和 base 相同 → 查 diff 发现有未 commit 改动 → 帮它补 commit + rm assignment.md + amend。**assignment 要教 teammate 一定 commit 后再写 done 文件**。

### 3.4 Monitor 脚本的 exit 判据要宽
初版只监听 done 文件；遇到 teammate 崩溃就永远等。Fix 版加 `kill -0 $PID 2>/dev/null` 检进程存活，死了 + 没 done = 异常 exit。

---

## 4 · T2 reviewer 发的 6 个 bug 怎么拆

T2 的建议是"修 BUG-1/2/3/5 ≤ 4h ship"，我 accepted 但**反一条 BUG-4**（见 `docs/decisions/05-conversation-switch-keep-wire.md`）。

```
Fix-A (我主线) · 玄女 tool-map.md 扩充 memory/skill/cron → BUG-3
Fix-B (cc team) · SystemEventBridge + TriggerLookup trait → BUG-1 + BUG-2
Fix-C (cc team) · Keeper 双实例合并 → BUG-5
(保留) BUG-4 让贤 dead code → v1.1
(保留) BUG-6 Extractor stub → v2
```

---

## 5 · 2026-04-20 用户实测后的重新分类 A/B/C/D

T2 review 漏掉了用户体感的大量问题。用户测完真跑后列了 12+ 条，我分类成 A/B/C/D 四类：

**A · 基础设施（1 条，立即修）**：stderr 重定向时机 → `main()` 开头而不是 `drive_tui()` 内

**B · TUI 交互性（7 条，起 cc team 抄社区）**：
- 输入框多行 / 粘贴 / 光标 / 方向键编辑 / IME 聚焦
- 中栏会话区滚动（历史消息看不到）
- 事件流信息密度低 / 过滤
- 多行消息渲染（同 speaker 连续消息去前缀折叠）
- 左栏 roster → 任务树（见 `docs/decisions/03`）
- 右栏元信息从 agent 级 → task 级
- CJK 宽度（用 unicode-width）

**C · 后端 bug（4 条，主线修）**：
1. stderr 时机（同 A.1，算基础设施也算后端）
2. **intervene idle 自动退化 dispatch**（玄女自诊断，见 `docs/decisions/04`）
3. `--resume` wire（fuxi 启动应从 oracle 读 xuannv 上次 session_id）
4. `TaskCompleted` 桥（bridge.rs 加订阅）

**D · 产品连续性（3 条，后续迭代）**：
- 启动 greet 硬编码"问好" → 应看 oracle 和 pending trigger 决定开场
- 会话连续性（跨 fuxi 启动）
- 玄女 markdown 回复的多行渲染（本质 B 类 bug）

---

## 6 · 今天改的 docs 骨架

```
docs/
├── architecture-v1.md                  # M1 蓝图（略更新 §M1.4 任务树 override 注）
├── decisions/                          # ★ 新 · 决策独立文件
│   ├── 01-agent-team-parallel.md
│   ├── 02-soul-first-skill.md
│   ├── 03-tui-task-tree-override.md
│   ├── 04-intervene-idle-degrade.md
│   ├── 05-conversation-switch-keep-wire.md
│   └── 06-cultural-naming-scheme.md
├── handoff/
│   └── v1-session2.md                  # ★ 新 · 下个 session 开工指引
├── research/                           # 5 份 survey（昨天）
├── session-review-2026-04-19.md        # 昨天凌晨
├── session-review-2026-04-19-afternoon.md  # 昨天下午
└── session-review-2026-04-20.md        # ★ 本文 · 今天
```

为什么 `decisions/` 拆 6 份独立文件（不合 1 份）：
- AI 友好：grep 精确定位
- 每份 <100 行，单读负担低
- 加新 decision 不 touch 旧文件
- 跨 decision 引用清晰

---

## 7 · 给下个 session 的提醒

1. **我 context 已超长**。本 session 我写的 decision / review 文档请直接读，不要重新诊断。
2. **Fix-D 可能还在跑**。`ls /tmp/fuxi-fd-done.txt` 看是否完活；`cd /Users/e0_7/fuxi-fd && git log --oneline` 看进度。
3. **Bug 3（--resume wire）是我留给下个 session 的**——等 Fix-D 合完再改（避免和 TUI 重构冲突）
4. **用户的协作范式**：不知道先 search / 遇问题查别人怎么解 / 多借鉴优质 repo。**TDD 硬要求**。**分治+聚合+review** 三层 MapReduce 方式做大项目。见 `~/.claude/projects/-Users-e0-7-fuxi/memory/feedback_*.md`
5. **不要** 用 "v0.1 / v0.2 做太多会散" 做借口——那是分治能力问题，不是产品决策
6. **永远可以反驳用户**。他要协作者不要 yes-man。做好开发就是讨好他

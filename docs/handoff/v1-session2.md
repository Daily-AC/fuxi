# Handoff · v1 · Session 2 开工指引

> 给下一个 Claude Code session 接手人。上一个 session (2026-04-19 → 2026-04-20) context 超长，大量决策和踩坑记录在 `docs/decisions/` 和 `docs/session-review-2026-04-20.md`。本文是**10 分钟开工**指引。

---

## 立刻读（按顺序，≤ 25 min）

1. `CLAUDE.md` · 公理 + 常见陷阱（5 min）
2. `docs/architecture-v1.md` · v1 蓝图（5 min，重点 §0 命名总表 + §6 收敛判据）
3. `docs/decisions/03-tui-task-tree-override.md` · TUI 左栏是任务树（不是 agent list）（3 min）
4. `docs/decisions/04-intervene-idle-degrade.md` · intervene 自动退化（3 min）
5. `docs/session-review-2026-04-20.md` §5 + §7 · 今天的 bug 分类 + 我给下个 session 的提醒（5 min）
6. `~/.claude/projects/-Users-e0-7-fuxi/memory/feedback_*.md` · 协作范式（自动加载，不用找）

**跳过即踩坑**：跳过 03 → 会继续把左栏做成 agent roster；跳过 04 → 对 intervene 的测试会挂。

---

## 当前仓库状态

**分支**：`feat/fuxi-v0.1`（下一次 ship 合到 `main`；v1 名字没换，字面 v0.1 分支内迭代到 v1）

**最近 commit**（`git log --oneline -15`）：
```
ffd5dfa fix: Bug 1/2/4 · stderr 时机 + intervene idle 退化 + TaskCompleted 桥
2524749 feat(cli,scheduler): 接入 SystemEventBridge
6c00a97 feat(orchestrator): SystemEventBridge
7434dc8 feat(core): TriggerLookup trait
3ad9c95 fix(scheduler): Keeper 双实例合并
dd1981d fix(xuannv): 工具表扩充 memory/skill/cron 三族
3c68af0 feat(scheduler): C5 更漏
98c653d feat(memory): 策府 v1
2888799 feat(cli/tui): C2 三栏 TUI（会被 Fix-D 重写）
...
```

**门禁**：fmt ✓ / clippy -D ✓ / **301 tests 全绿**

**未合并的 worktree**（可能）：
```bash
git worktree list  # 如果有 ../fuxi-fd 说明 Fix-D 在跑或没清理
ls /tmp/fuxi-fd-done.txt 2>/dev/null  # 有 = Fix-D 完活待合
```

---

## 开工 · 先做 2 件事

### 1. 跑一次门禁基线

```bash
cd /Users/e0_7/fuxi
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

3 个都绿才继续。挂了先修（最可能是 Fix-D 改动没合干净，或 merge 冲突解错）。

### 2. 验证 Fix-D 是否完活

```bash
if [ -f /tmp/fuxi-fd-done.txt ]; then
    echo "Fix-D 完活。cherry-pick 到主分支："
    cat /tmp/fuxi-fd-done.txt
    cd /Users/e0_7/fuxi-fd && git log --oneline 3c68af0..HEAD
else
    echo "Fix-D 在跑 / 已放弃 / 没起 —— 查 /tmp/fuxi-fd.log 最后 100 行定位"
fi
```

---

## 上个 session 留的尾巴

### A · 必做（蓝图收敛判据未达标）

**Bug 3 · `--resume` wire** — fuxi REPL 启动时：
- 从 `oracle_facts` 读 `subject="xuannv" predicate="session_id"` 取上次 session
- 有则 `CcLaunchConfig { resume_session_id: Some(id), ..default }`，没则新起 `session_id = Uuid::new_v4()`
- 玄女 cc `system/init` 事件带 session_id → 捕获并 `fuxi memory record xuannv session_id=<new>`
- **等 Fix-D 合完**，基于新 repl.rs 改（避免冲突）

### B · 体感 / 产品完善

**Bug D.1 · 启动 greet 硬编码** — 现在每次启动都是"向用户问好"。应该：
- 查 oracle 看用户偏好 / 项目 context
- 查 triggers 看是否 pending（如用户睡觉期间触发过 trigger）
- 查 memory search "最近未完成任务"
- 综合做 context-aware 开场白（或什么都不说等用户先发）

**Bug D.2 · 跨会话连续性** — A 做完后天然连续。可视作 A 的续集。

### C · 想做但可等

**Bug 6 · Extractor 实装** — 当前 stub；v2 接入真 extractor 门客（cc headless 抽 fact）
**BUG-4 · 让贤发起源** — v1.1 加鲁班 skill 工具 + daemon Command::Handoff

---

## 开发范式（必须遵守）

从 `memory/feedback_*.md` 抄出来的**硬约束**，本 session 不要违反：

1. **先 search 不造轮子** · 遇问题先查别人怎么解 / 抄优质 repo
2. **TDD 硬要求** · 先写失败测再写实现；事后补测 = 走捷径
3. **分治 + 聚合 + review** · 大项目起 agent team，别用「做太多会散」当借口
4. **反驳用户不是讨好** · 做好开发才是讨好；但需 confirm 的 edge case（不可逆操作 / 意图含糊）仍要确认

---

## 高风险区（前任踩过，你小心）

- 改 `EventKind` 加变体 → **必** 同步更新 `events/store.rs::kind_tag` + `firehose/hub.rs::kind_tag` + `firehose/tui.rs::summarize + color_for` + 相关持久化测试。漏一处 clippy `-D` 就报
- Mac tempdir symlink `/var/folders/...` vs `/private/var/folders/...` → 对比要双端 `canonicalize`
- Clash / Surge TUN 把 127.0.0.1 吞 → `spawn_claude` 已注入 `NO_PROXY`，**手动起 claude 也要注入**
- Cargo.lock cherry-pick 后常坏 → 直接 `rm Cargo.lock && cargo build` 重生
- 玄女 cc session_id 在 oracle 里用 `subject="xuannv"` 统一 key，不要用 agent_id（agent_id 每次 spawn 新）

---

## 不要做的事

- 重做 `docs/research/` 下的 5 份 survey（上个 session 已穷尽）
- 改 `docs/decisions/` 下已定的 6 个决策（要改先写新 decision 07 说为什么改）
- 起新 C team 重写 C1-C5 已合的模块（能改就改，不能就加 layer）
- 用 "先简化 v0.1 再 v0.2 补" 这种说辞 —— v1 是蓝图，不切

---

## 我的偏好（顺便）

- 中文注释 / 英文代码标识符
- commit message `type(scope): 中文摘要` 格式
- decision 写独立短文件（100 行以内）
- 工具 bash 命令尽量带错误兜底（`|| true` / `2>&1 | tail`）

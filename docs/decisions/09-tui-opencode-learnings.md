# Decision 09 · TUI 系统性借鉴 opencode · 12 条打包一批 ship

**日期**：2026-04-21
**状态**：已采纳

## 背景

2026-04-21 session，用户反馈"tui 很多交互很乱"。让主线对比阅读：

1. `opencode`（sst/opencode · TypeScript + OpenTUI · alt-screen TUI）—— **同形态主要参照**
2. `claw-code`（ultraworkers/claw-code · Rust rustyline REPL · 行式流）—— 仅贡献命令分类 / spinner

研究报告结论：差距不在单点（slash / @ / 主题 / 复制 / 布局）任一项，**是系统性交互基线 + 活状态反馈缺失**。

用户拍板："全做，跟 #1 对齐的所有功能都做"。

## 决策

原 M4.4（D16 slash+@）范围扩张为 **M4-REDUX**——12 条一批全做（见 roadmap §M4.4）。

明确**延后 v1.2**：`@` mention（缺 file picker 后端）、extmark 受保护片段（ratatui 无等价物）、session timeline / fork-from（demo 性而非毕设必需）。

## 为什么是 "全做" 而不是 "分批"

用户原话："毕设只是顺带，别拿毕设当 ddl"。

**重排优先级**：
- 伏羲是**个人 AI agent 平台**，不是毕设 demo 的 1.0 snapshot
- TUI 体验是日常使用的门面，不是毕设评审的加分项
- opencode 的 12 条全是"日常使用立竿见影"的基线改进，没有一条是纯装饰

**成本可控**：12 条里 6 条小、4 条中、2 条大，估 2-3 session 可收。用 agent team 并行切半。

## 反对意见预演

- *"12 条一批 PR 太大，review 不动"* → agent team 拆 4 track + 4 commit 分开 merge，每 track 独立可 review
- *"v1.1 ship 被阻塞"* → v1.1 的原 M2/M3 已完，M4-REDUX 是补课不是新增。它 ship 的前提本来就是 M4 体验达标
- *"drag-release 复制 2-3 天成本大"* → 是，但这是用户最痛点（"现在 tui 很乱"的最大来源），且是 opencode 零按键的**真相**，不做就永远觉得"别人能我们不能"

## 代价

- 短期：v1.1 ship 再推 2-3 session
- 长期：部分 M5 scope（单栏化 D15 / 主题切换）提前到 v1.1 完成，M5 反而瘦身

## 关联 Decision

- 覆盖原 **Decision 06**（文化命名）—— /theme 切主题要支持多主题名，不止 mocha/latte
- 挤掉原 **M4.4 D16** —— @ mention 延 v1.2，只做 / slash
- 和 **Decision 03**（TUI 任务树 override）共存 —— 任务树改为 F2 召唤的 overlay，不砍
- 和 **Decision 04**（intervene idle 退化 dispatch）共存 —— user-turn 视觉差 (R1 之后重新 audit)

## 验证

agent team 每 track 完工后跑 `cargo test --workspace` + 用户手测 5 个场景：
1. busy 时点 `/` 弹浮层 + 选中 /help 无卡顿
2. 对话中途拖选一段文字 → 自动进剪贴板 Cmd+V 粘贴验证
3. 双击 Esc 中断当前 task 不退 TUI；Ctrl+C 才退
4. F2 切左栏任务树 overlay / F4 切右栏 meta overlay
5. `/theme latte` 运行时切亮色主题立即生效

## 参考

- `opencode` 关键文件（/tmp/opencode 下 clone）：
  - `packages/opencode/src/cli/cmd/tui/component/prompt/autocomplete.tsx:546-671` · slash/@ 浮层
  - `packages/opencode/src/cli/cmd/tui/app.tsx:80-85,287-296` · drag-release 复制
  - `packages/opencode/src/cli/cmd/tui/util/clipboard.ts:25-32` · OSC52 + pbcopy
  - `packages/opencode/src/cli/cmd/tui/routes/session/index.tsx:1058-1059` · sticky scroll
  - `packages/opencode/src/cli/cmd/tui/ui/toast.tsx` · toast stack
- `claw-code/rust/crates/rusty-claude-cli/src/render.rs:47-116` · Spinner 三态

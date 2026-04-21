# Decision 05 · 让贤（ConversationSwitch）wire 保留不删，发起源延 v1.1

**日期**：2026-04-19
**状态**：**已被 Decision 08 override（2026-04-21）**——让贤已整体拆除。原文保留作决策轨迹。

## 背景

T2 reviewer（2026-04-19 晚）评审 M1.5 orchestrator 补课时指出：

> **让贤（ConversationHandoffRequested）** wire 完了但完全是 dead code —— `request_handoff` 没有任何调用方（除测试），玄女 SKILL 没有 `fuxi handoff` 工具，铸牒司也没让贤指令。
>
> **BUG-4 修法**：daemon 加 `Command::Handoff { from, to, reason, brief }` → 调 `fuxi.request_handoff(...)`；鲁班 / 铸牒司 SKILL 加这条工具。**否则 ship 的时候删掉 EventKind 假装没设计过**。

## 决策

**保留 wire + 延 v1.1 接发起源**，不删。

`Fuxi::request_handoff` 公开 API 保留，事件变体 `ConversationHandoffRequested { from, to, reason, brief }` + `ConversationReturned` 保留，TUI 订阅 switch active 逻辑保留。

## 为什么反 T2 建议

1. **三独创之一**：让贤（ConversationSwitch）和 抄送（InterventionProxy）+ 真实时 Firehose 是伏羲差异于 ComposioHQ 的**三大独创赌注**。毕设 narrative 里这是旗帜
2. **删了难加回来**：EventKind 破坏性变更需同步更新 5 处（events/store + firehose/hub + tui/summarize/color_for + 持久化测试）。删了 v1.1 再加等于再过一遍这套
3. **风险可控**：不接发起源 = dead code 没副作用。编译期 `request_handoff` 有 `#[allow(dead_code)]` 或测试覆盖即可
4. **v1.1 加发起源 ≤ 1 天**：daemon 加 `Command::Handoff` + 鲁班 skill 加 `fuxi handoff --to ... --reason ...` 工具 + dispatch-protocol.md 加"什么时候让贤"规则

## v1.1 计划（非本次 scope）

- `crates/fuxi-cli/src/subcommands.rs` 加 `run_handoff`
- IPC 协议 `Command::Handoff { from, to, reason, brief }`
- 鲁班 / 铸牒司 SKILL.md 新增 `fuxi handoff --to <id> --reason "<r>" [--brief "<b>"]`
- dispatch-protocol 加 §10 「让贤判据」：
  > 当门客遇到超出当前 role 能力的子任务（比如鲁班发现需求澄清），应主动调 `fuxi handoff --to <pm-id>` 让贤，而不是低质量完活

## 代价

- 代码里有未被生产代码调用的 API（`#[allow(dead_code)]` 注释）—— 心理洁癖损失
- 测试覆盖要额外保留（不然 code coverage 指标难看）

## 验证

`cargo clippy -D warnings` 绿 = 没 dead_code 警告（因为有测试覆盖）。

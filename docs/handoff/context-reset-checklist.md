# Context Reset Checklist（执行板）

## 目标
- 降低上下文污染，避免重复循环。
- 每轮只围绕一个目标推进，并给出可验证证据。

## Checklist
- [x] 建立唯一入口：`docs/handoff/now.md`
- [x] 旧 handoff 标记 `stale`
- [x] 可逆停放历史脏改动（stash）
- [ ] 锁定本轮代码白名单（只改 orchestrator/cli/repl + tests）
- [ ] 收口 legacy 派工语义（`dispatch_to_any`）
- [ ] 基线门禁一次性跑通（fmt/clippy/test）
- [ ] 生成收敛报告（改了什么/证据/剩余1条）

## 执行规则（强约束）
1. 不做“仅状态同步”的提交。
2. 不在本轮引入新体验分支（UI 主题、额外交互）。
3. 提交必须附带至少一条测试或命令证据。
4. 汇报统一三行：
   - 改动文件
   - 验证证据
   - 剩余项（仅 1 条）

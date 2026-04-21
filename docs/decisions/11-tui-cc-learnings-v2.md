# Decision 11 · TUI 借鉴 cc 第二轮 · 12 条按 ROI 分三批

**日期**：2026-04-21
**状态**：已采纳

## 背景

M4-REDUX 合 + 用户跑了一屏看到仍有体验粗糙（system 行污染对话区 / event 噪声 / popup 跳中央 / 快捷键过载）。

用户反馈：个人偏爱 Claude Code CLI 的 TUI，提供源码压缩包让调研借鉴。

主线解包 `/tmp/cc-source/` + 派调研 agent 读 `src/ink/` 全模块，产出 12 条具体借鉴（见 2026-04-21 session transcript "cc TUI 设计文档"）。

## 决策

12 条按 ROI 分三批：

### 🔴 Batch C · v1.1 末立即做（4 条）

| 决策 | 规模 | 理由 |
|---|---|---|
| C1 · **TeammateSpinnerTree** · 并行门客独立 spinner + verb + token，`Ctrl+T` 展开折叠 | 大 | 伏羲卖点视觉载体，不做在 demo 里和普通 CLI 无差异 |
| C2 · **连续同类工具折叠** · 多次 Read/Grep/Glob 自动折一行 `Read(a) · Grep(b) (+2 more)` | 中 | 屏幕噪音根解，玄女派 10 个 Read 不会刷爆 |
| C3 · **Spinner 动词池** · `"玄女思考中/推敲中/衡量中/筹谋中"` 随机抽 | 小 | 零成本人格感 |
| C4 · **Ctrl+C 双击窗口 + 提示** · 第一次 interrupt + "再按一次退出"底部提示，双击内才真退 | 小 | 对称 Esc 双击（Decision 04 + β #6 已做） |

### 🟡 Batch D · v1.2 初和 Decision 10 一起（4 条）

| 决策 | 规模 | 依赖 |
|---|---|---|
| D1 · **Task-bound agent lifecycle + 任务树 UI** | 大 | Decision 10 主体 |
| D2 · **F4/F5 overlay → `/tree` `/meta` slash 收敛** | 中 | Decision 10 任务树重写后顺手合并 |
| D3 · **消息视觉层级** · 用户消息浅色整块背景 / assistant `●` 前缀 / 工具独立卡片 | 中 | repl render dialogue 重写 |
| D4 · **Slash popup 重构** · `/` 始终入 textarea，popup 变观察副作用；状态单份 | 中 | γ 之前双状态设计改造 |

### 🟢 Batch E · v1.2 后期或 v2（4 条）

| 决策 | 规模 |
|---|---|
| E1 · **`@` mention** · `@agent` 直接 DM + `@file` 附件（Decision 10 + file picker 后端都就位后）| 中 |
| E2 · **句中 mid-input `/foo` ghost text** | 中 |
| E3 · **Slash frecency 排序** · 空 `/` 列最近使用前 5 | 小 |
| E4 · **Session resume picker** · `/session` fuzzy 选 | 中 |

### Tab / Enter 行为拆分（按需补 Batch）

cc 的 `useTypeahead` Tab=补全不提交，Enter 对有参命令留空格等用户输入，对无参直接跑。

当前伏羲 popup Enter 一刀切直接执行（γ #13 / #17 设计）。

**小改动**、**ROI 中等**、**本来可以挤 Batch C**，但命令元数据要加 `argNames` 字段。

**决定**：挤进 Batch C 一起做（+1 条 C5）。

## 明确不抄的

- **React hook / memo 等 TypeScript 架构**——Rust 重绘全量便宜，不需要
- **`@"my file.ts"` 引号路径语义**——伏羲 `@` 主用来派门客，文件附件是 nice-to-have
- **StatusLineCommandInput（用户 shell hook 覆盖状态栏）**——功能逃生舱，伏羲场景不需要
- **cc 专属功能**：IDE integration (VS Code selection sync) / Remote session (WebSocket mirror) / KAIROS brief mode / `/rewind` 时间穿越——这些需要大额基础设施，v2+ 再考虑

## 代价

- Batch C 估 1 session；Batch D 和 Decision 10 绑一起 1-2 session；Batch E v1.2 再论
- TeammateSpinnerTree 需要**事件流 → UI aggregation** 的新 pipeline，得设计好订阅粒度（per-agent spinner 不是 ambient poll）
- 工具折叠要定**何时不折**（错误 / 在跑 / 同类但跨 agent）

## 验证

每批次按 TDD 契约 + 用户手测过验收清单：
- Batch C：连开 3 门客并行，能看到三 spinner 树；一屏派 10 次 Read 只占 1-2 行
- Batch D：任务树按 task 归属呈现，`/tree` `/meta` 替代 F4/F5
- Batch E：`@luban` 有两个活的能弹 picker

## 参考

- 调研产出：2026-04-21 session 里 cc-source 提取的"cc 12 条借鉴"设计文档
- cc 源引用（路径锚点，不搬码）：
  - `src/ink/components/Spinner.tsx` · spinner verb 池
  - `src/ink/components/VirtualMessageList.tsx` · 粘底滚动
  - `src/ink/hooks/useTypeahead.tsx` · slash/autocomplete 核心
  - `src/ink/components/PromptInput/` · 输入框结构
  - `src/ink/components/AssistantToolUseMessage.tsx` · 工具折叠卡
  - `src/ink/hooks/useExitOnCtrlCD.ts` · Ctrl+C 双击
  - `src/ink/events/defaultBindings.ts` · 全局快捷键表（仅 ~10 条）

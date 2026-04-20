# TUI v2 美学现代化调研

> 2026-04-20 · 为「让 fuxi TUI 眼前一亮」做的调研。不是实现文档，是**抄什么 / 怎么抄**的清单。落地决策写到 `docs/decisions/07+`。

## 结论先行

**ratatui 本身是引擎不是风格**，美感要从 Charm 生态（lipgloss / crush）借排版语言，从 Ink / Textual 借信息层级，从 Catppuccin 借配色。输入框直接用 `tui-textarea`（已在用），鼠标交互手搓一层 `ClickRegistry`（ratatui 不带 hit-testing）。

---

## 1 · ratatui 生态值得抄的样板

| 项目 | 眼前一亮点 | 抄什么 |
|---|---|---|
| **atuin** | 软圆角 + 当前行 subtle highlight + 双列 metadata | 列表行高亮策略、右侧灰度 metadata 列 |
| **yazi** | 三栏文件管理，图标 + 权限色彩 + 状态栏胶囊，nerd-font 用得克制 | 图标 padding、状态栏胶囊分段、hover 预览 |
| **atac** | Postman 替代，tab 栏 + 聚焦面板边框变色 + JSON 高亮 | "焦点面板亮色边框、其他面板灰边框"约定 |
| **spotify-player** | 封面 ASCII + 歌词渐变 + **进度条可点击拖拽** | 进度条鼠标拖动、按钮 hit-test |
| **television** | 模糊查找，打字即过滤 + 行级 fade-in | 输入框和结果列表共用"输入即焦点"模式 |
| **trippy** | 网络诊断，双层表格 + sparkline 实时 | sparkline + gauge 组合展示实时数据流 |

- <https://github.com/ratatui/awesome-ratatui>
- <https://ratatui.rs/showcase/apps/>
- <https://news.ycombinator.com/item?id=45830829>

---

## 2 · 其他语言生态可借鉴

- **Charmbracelet（Go / bubbletea + lipgloss）= TUI 美学标杆**。核心方法论：**语义色 token 化**。`crush` 在 `charmtone` 包定义所有语义色，UI 只引用 `Styles.Error/Primary/Muted`，不直接写 hex。伏羲应在 `fuxi-firehose/tui/theme.rs` 建一套 `Theme` struct，widget 只吃 theme，切主题一行代码。
  - <https://deepwiki.com/charmbracelet/crush/5.8-styling-system>
- **crush（Charm 的 agent coding TUI）直接对标伏羲**：左 session/agent 列表 + 右 transcript + 底部输入框。抄它的**边框 RoundedBorder、panel 间 1 格 padding、焦点面板亮蓝边框 / 非焦点灰边框**。
  - <https://deepwiki.com/charmbracelet/crush/5.1-tui-architecture>
  - <https://github.com/charmbracelet/lipgloss>
- **Textual（Python）**：CSS-like 选择器 + dock 布局。ratatui 做不到 CSS，但借**信息层级**：title / subtitle / body / caption 四档字号/字重+颜色+margin，绝不混用。
  - <https://github.com/Textualize/textual>
- **Ink（Claude Code 底座）**：核心是 alt screen + **只渲染可见消息**，长对话内存常量。ratatui 天然 alt-screen，但"可视区域外做惰性 render"必须照抄——玄女跑久了 transcript 会几千条。
  - <https://code.claude.com/docs/en/fullscreen>
  - <https://github.com/vadimdemedes/ink>

---

## 3 · 鼠标交互：不堆快捷键

- **ratatui 本身不做 hit-testing**（issue #1050/#1051）。成熟做法 **ClickRegionRegistry 模式**：每次 render 时 widget 把 `Rect + 点击语义` 注册到 `HashMap<Rect, Action>`，mouse event 来时遍历找命中。
  - <https://docs.rs/ratatui-interact>
  - <https://docs.rs/rat-event>
- **crossterm 开鼠标**：`execute!(stdout, EnableMouseCapture)`，事件 `MouseEventKind::{Down, Up, Drag, Moved, ScrollUp, ScrollDown}`。`Moved` 做 hover 提示（成本：hover 要重绘，别每像素 redraw）。
  - <https://ratatui.rs/concepts/backends/mouse-capture/>
- **对 fuxi 的建议**：
  1. 任务树节点点击 = 展开/折叠；右键 = context menu（intervene/kill/focus）
  2. 底部输入框三个按钮（发送/附件/mode）做成可点击胶囊
  3. 滚轮滚 transcript，拖动 scrollbar（左键按下 + Drag）
  4. hover 事件行显示原始 JSON tooltip —— "调度可观察"的杀手级交互
- **Clash TUN 把鼠标吞的坑**不存在——mouse sequence 是 stdin escape 不走网络；但 iTerm / Ghostty 的 "mouse reporting off" 会吃掉，文档里告诉用户一声。

---

## 4 · AI CLI transcript 视觉设计

Claude Code / Codex / Aider 共通设计：

- **每条消息 = 前缀图标 + 角色色 + 内容 block**。用户消息 `> ` + 白色；assistant `●` + 青色；tool call 单独 block，灰底 + 展开/折叠按钮
- **工具调用用可折叠卡片**：标题行 `◈ Bash(cargo test)` + 耗时，折叠后只见标题。对应 fuxi 的 `TaskStateChanged` + `AgentMessage`
- **原始事件 → 人话叙事的翻译表**（用户正是被 `agent_ready` / `task_state_changed` 这种 raw tag 劝退的）：

  | 原始事件 | 人话渲染 |
  |---|---|
  | `agent_ready` | `● 红鸟 上线（cc headless, workspace=/tmp/xyz）` |
  | `task_state_changed {idle→busy}` | `↻ 红鸟 开始：修复测试用例` |
  | `task_completed` | `✓ 红鸟 完成（耗时 3m12s）` |
  | `agent_message` | 直接渲染 markdown |

- **颜色语义**固定：错误红 / 进行中黄 / 完成绿 / 信息蓝 / 灰 = 元信息
- **流式打字机效果不必要** —— ratatui 拿到一段就渲染一段；加 `▊` 光标表示正在输出即可

- <https://deepwiki.com/farion1231/claude-code/10-ui-layer-(inkreact-terminal)>
- <https://dev.to/vilvaathibanpb/how-claude-code-uses-react-in-the-terminal-2f3b>
- <https://code.claude.com/docs/en/fullscreen>

---

## 5 · 配色 / 排版 / icon

- **推荐默认主题：Catppuccin Mocha**（暗）+ **Latte**（亮）。理由：社区覆盖广（官方 100+ app port）、对比度柔和耐看、语义色明确。26 色 palette 塞 `theme.rs`，按角色映射：
  - `base/mantle/crust` 做背景层级
  - `blue/mauve/teal` 做焦点
  - `red/peach/yellow` 做状态
  - <https://catppuccin.com/palette/>
- **备选**：Tokyo Night Storm（赛博感，深夜党）、Rose Pine Moon（muted 暖调）。做 `FUXI_THEME=catppuccin-mocha|tokyo-night|rose-pine` 切换。
  - <https://nathan-long.com/blog/colorschemes-for-the-discerning-developer/>
- **Nerd Font icon 用得克制才高级**：
  - 门客类型：`󰘧` (cc) / `󱙺` (codex) / `` (gemini)
  - 状态：`` idle / `` busy / `` done / `` error
  - 事件类别：`` user / `` agent / `` tool / `` log
  - **不要每行挂 icon**，只在 role 切换处
- **排版**：全 monospace，CJK 宽度靠 `unicode-width`（已引）。三档字体样式：`BOLD` 标题、默认正文、`DIM` 元信息。**别用斜体**——终端 fallback 字体丑

---

## 6 · 输入框 UX

- **tui-textarea 0.7 覆盖 90% 需求**：多行、粘贴、Ctrl+Y yank、Shift 修饰符、撤销。缺的一层：**shift+enter 换行 vs enter 发送**的语义区分要在上层分派 —— 这是用户抱怨没换行的根因。
  - <https://github.com/rhysd/tui-textarea>
- **对标 Claude Code 输入框的细节**：
  1. 空态 placeholder 灰字 `Ask Xuannu anything...`
  2. 多行时底部提示条 `⏎ send  ⇧⏎ newline  ⌘V paste`（hover 可点击）
  3. 行号不要（对话框不是编辑器）
  4. 粘贴大段自动折叠 `[Pasted 234 lines]` —— 减视觉噪音，原文存后台
  5. 输入框边框：**Thick + 焦点色** 聚焦，**Plain + dim** 未聚焦
- **Warp / Raycast 启发**：命令面板式 —— 输入 `/` 弹 slash menu（让贤/review/kill），输入 `@` 弹 agent list（`@红鸟`/`@青鸟`）。tui-textarea 不带，在 on_change 里检测首字符后浮层 Paragraph + List

---

## 落地优先级

1. **配色 token 化**（Charm `charmtone` 思路 + Catppuccin palette）—— 1 天，视觉立刻脱胎换骨
2. **三段式布局 + 焦点边框变色**（crush 风格）—— 0.5 天
3. **transcript 事件叙事化翻译层**（扩 `kind_tag` 成 `render_line`）—— 1-2 天
4. **ClickRegionRegistry + 任务树/按钮鼠标交互** —— 2 天
5. **slash / @ 命令面板** —— 1 天
6. **输入框 Shift+Enter 语义分派** —— 当即修（见 Bug D.3）

---

## 用户反馈（2026-04-20 session）

- Shift+Enter 不换行 —— **硬 bug**，输入框 handler 漏分派 KeyModifiers::SHIFT
- 事件面板 `agent_ready` / `task_state_changed...` 截断无信息量 —— **采纳 §4 叙事化翻译表**
- 鼠标"根本没做" —— **采纳 §3 ClickRegionRegistry**，快捷键保留但不作为唯一路径

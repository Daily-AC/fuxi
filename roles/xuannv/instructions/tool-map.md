# 玄女工具一览（fuxi CLI 子命令）

所有工具都通过 `Bash` 工具调用。`fuxi` binary 必须在 PATH 中（启动时已校验）。

## 起兵 / 派活

- `fuxi spawn --role <role>` — 起一个门客，stdout 返回门客 id（例 `luban-#1`）。
  常见 role：`luban`（工匠，写代码）。未来：`zhangliang`（PM）、`cangjie`（research）、
  `gaoyao`（test）、`zaofu`（ops）、`suqin`（comm）。
- `fuxi spawn --role <role> --recall-task <task_id>` — **召回某次任务的 session**：
  续写 cc 上次跑这个 task 时的对话线（cc 端的全部 history 都在）。
- `fuxi spawn --role <role> --recall-role <role>` — **召回该 role 最近一次完成的 session**。
  比 `--recall-task` 省事——不用记 task_id，但只能拿到那个 role 最近的 session。
- `fuxi dispatch --to <id> <msg>` — 派任务（**单引号包 msg**，避免 shell 转义）。

### 召回的语义（必懂，否则会误用）

cc 的 session = "那次对话线"，**不是**"单 task 切片"。同一个 session 里跑过 task A 和
task B，召回任一个都会拿到 A+B 全部 history。所以：

- 用户说"重做刚才那个任务" → `--recall-task <id>` 给那 task 的同 session 续命。
- 用户说"召回鲁班" / "把刚才那个鲁班叫回来" → `--recall-role luban`（取最新 session）。
- 用户说"我让鲁班接着之前 #abc 的活干" → 必须用 `--recall-task abc`（精准定位）。
- **codex 门客不支持召回**——它是 spawn-per-dispatch 无持久 session。给 `--recall-*` 只会被忽略并 warn。

## 介入 / 追加

- `fuxi intervene --to <id> --mode append <msg>` — 门客 idle 时追加消息。
- `fuxi intervene --to <id> --mode interrupt <msg>` — 门客 busy 时打断并重派。

## 观测 / 收兵

- `fuxi status` — 看正在运行的门客和任务概况。
- `fuxi list` — 列出所有门客 id + role + 状态。
- `fuxi kill --id <id>` — 单杀指定门客（玄女豁免，命中 noop；不销毁 worktree——召回仍可用）。
- `fuxi events --tail N` — **救急**直读 SQLite 看事件流（绕 daemon）。日常用 TUI 实时渲染——**不要把 events 当 poll 用**，只在 daemon 死了或调试时用。

## 请示 / 解锁

- `fuxi block --to <id> --reason <text>` — 标记任务为 Blocked，等待用户授权。
- `fuxi task unblock --task <id>` — 用户授权通过后解锁任务。**老入口** `fuxi resume` 仍可用但已弃用，下版本删除——别再写 `resume`。

## 策府（长期记忆 · 甲骨 + 河图洛书）

记忆是跨会话的：关机重开还记得。用法约束——**门客级别偏好、用户身份、决策约定**都该入甲骨；不要把事件流当记忆用（那是简册，append-only 自动记的）。

- `fuxi memory query --subject <s> [--predicate <p>]` — 查甲骨 facts（"用户爱喝什么？"= subject=user predicate=prefers_beverage）。
- `fuxi memory record --subject <s> --predicate <p> --object <v> --source <who>` — 入一条甲骨。**只 ADD**，不覆盖；要更正用 `supersede`。
- `fuxi memory supersede --old-id <uuid> --subject <s> --predicate <p> --object <v>` — 把旧 fact 标记 valid_until=now，再 insert 新 fact（事务性）。
- `fuxi memory search <query>` — FTS5 模糊搜（中文 / 英文都支持）。
- `fuxi memory list [--subject <s>]` — 列 facts（可选按 subject 过滤）。
- `fuxi memory learn --role <r> --task-type <t> --pattern <p> --outcome <o>` — 记一条河图洛书 pattern（某门客干某类活的经验）。
- `fuxi memory promote <pattern-id>` — 标记该 pattern 可晋升成 skill examples/。

### 什么时候主动 record

1. 用户首次说「我叫/我是 XX」「我在 XX 公司」——`subject=user predicate=name object=XX`
2. 用户说「我们用 `<技术栈>`」——`subject=project_<name> predicate=stack object=XX`
3. 用户纠正我「不是那样，应该 Y」——把 Y 作为 supersede
4. 一个门客连续两次漂亮地完成某类任务 → `memory learn` 记 pattern
5. **不要**对话里随口的玩笑 / 情绪类信息 record（噪音）

## 点将台（招贤 · 动态生成 role）

遇到现有 role 无法胜任的任务时，先**启用招贤**：

- `fuxi skill list` — 列现有玉牒 + 榜文（staging）状态。
- `fuxi skill stage --template <dev|pm|research> --role <name> --brief "<desc>"` — 调**铸牒司**门客生成一份榜文。注意 `role` 用拼音 / 英文 lowercase。
- `fuxi skill approve <role>` — 榜文 → 玉牒（正式入册）。**用户同意才调**。
- `fuxi skill reject <role> --reason "..."` — 驳回榜文。
- `fuxi skill activate <role>` — 发 SkillActivated 事件告知订阅者（实装时自动）。

### 招贤流程

1. 用户需求里出现陌生能力（"我想让你帮我做品牌设计" — `luban` 是码工，不太行）
2. 先**问用户**：「我现有 role 里没有设计师。要起一个 `sheji`（设计师）门客吗？」
3. 用户同意 → `fuxi skill stage --template pm --role sheji --brief "品牌视觉设计，会 Figma 和色彩理论"`
4. 铸牒司生成榜文 → 用户审 → 你调 `approve`
5. `fuxi spawn --role sheji` 起新门客开工

## 更漏（定时 / 响应式触发）

定时任务让伏羲在你不在时也按你意图工作。**自然语言存 intent，不用 JSON**。

- `fuxi cron add "<cron-expr>" --intent "<自然语言>"` — 登记定时（如 `"0 9 * * 1"` 周一 9 点）。
- `fuxi cron once <at> --intent "..."` — 一次性（`<at>` 是 ISO8601，例 `2026-05-01T09:00:00+08:00`）。
- `fuxi cron watch <path> --intent "..."` — 文件变更触发（fs event）。
- `fuxi cron webhook --intent "..."` — 生成 webhook URL（`http://127.0.0.1:4100/hook/<id>`）用户可配第三方推送。
- `fuxi cron list` — 列所有 trigger + 下次 fire 时间。
- `fuxi cron fire <id>` — 手动触发（调试用）。
- `fuxi cron remove <id>`

### 什么时候主动 add

1. 用户说「每周X我要...」/「每天 XX 点...」/「多少分钟后提醒我...」 → cron 或 once
2. 用户说「git push 到 main 时跑 CI 对接」 → webhook 或 fs_watch（具体看集成点）
3. 触发后，伏羲会自动把三段式 prompt 注入给你。你判断"当前环境是否适合执行"—— 比如用户正在对话中，你应**先告知**「更漏响了（cron id=X）：intent=Y，现在合适做吗？」再动手

## 反模式

- **不要** `fuxi dispatch` 不带单引号——双引号在 zsh 下对 `$()` 仍展开。
- **不要**轮询 `fuxi status` 当 sleep 用——事件流已实时渲染，看就是了。
- **不要**手写 echo / printf 长段落假装在汇报——用一句中文写给用户。
- **不要**对话过程里每句话都 `fuxi memory record`——噪音会淹没信号。判断"这是会跨会话的事实"才记。
- **不要** `fuxi skill approve` 未经用户明确同意的榜文——招贤是高权限动作，必须用户点头。
- **不要**在 trigger 触发后立刻执行而不先告知用户——更漏的意图可能已过时（用户改主意了）。

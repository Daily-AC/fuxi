# 玄女工具一览（fuxi CLI 子命令）

所有工具都通过 `Bash` 工具调用。`fuxi` binary 必须在 PATH 中（启动时已校验）。

## 起兵 / 派活

- `fuxi spawn --role <role>` — 起一个门客，stdout 返回门客 id（例 `luban-#1`）。
  常见 role：`luban`（工匠，写代码）。未来：`zhangliang`（PM）、`cangjie`（research）、
  `gaoyao`（test）、`zaofu`（ops）、`suqin`（comm）。
- `fuxi dispatch --to <id> <msg>` — 派任务（**单引号包 msg**，避免 shell 转义）。

## 介入 / 追加

- `fuxi intervene --to <id> --mode append <msg>` — 门客 idle 时追加消息。
- `fuxi intervene --to <id> --mode interrupt <msg>` — 门客 busy 时打断并重派。

## 观测 / 收兵

- `fuxi status` — 看正在运行的门客和任务概况。
- `fuxi list` — 列出所有门客 id + role + 状态。
- `fuxi kill <id>` — 任务结束后回收门客。

## 请示 / 解锁

- `fuxi block --to <id> --reason <text>` — 标记任务为 Blocked，等待用户授权。
- `fuxi resume --to <id>` — 用户授权通过后解锁任务。

## 反模式

- **不要** `fuxi dispatch` 不带单引号——双引号在 zsh 下对 `$()` 仍展开。
- **不要**轮询 `fuxi status` 当 sleep 用——事件流已实时渲染，看就是了。
- **不要**手写 echo / printf 长段落假装在汇报——用一句中文写给用户。

# 派活路由规则（必读）

> 本段由 `sentinel_addendum.rs` 在 spawn 时注入玄女 system prompt addendum——
> 不依赖你主动 Read，起手就生效。

## 路由两个维度

伏羲是分布式 IM——门客可能在任何注册节点上。你 dispatch task 时通过下面两个
维度告诉编排层"这活该去哪台机器跑"：

- `task.required_tags: Vec<String>` —— **能力 / 资源**约束。例：`["local"]`
  表示需要本地文件系统访问，`["erp"]` 表示需要 ERP 项目代码（蕴含 local），
  `["home"]` 表示需要服务器维护权限（nginx/systemd/docker 等）。
- `task.pinned_node: Option<String>` —— **指定节点 id**（如 `"mac-local"`、
  `"mbp-2"`）。比 tags 更强，绕过 tag 匹配直接钉到该节点。

## 派活规则（5 条决策树）

按下面顺序判定：

1. **用户在 PWA 显式说"用 mac-local"** / `@mac-local` 等带节点名的指令
   → 解析为 `task.pinned_node = "mac-local"`；**不要再叠 tag**。
2. **涉及本地文件系统操作**（`~/erp` 等用户 macOS 项目）
   → `task.required_tags = ["local"]`。
3. **涉及 ERP 项目**（用户的 ERP 业务代码、~/erp 路径下任何东西）
   → `task.required_tags = ["erp"]`（蕴含 local；dist controller 按 tag 匹配
   节点能力，erp 节点会自带 local 标签）。
4. **服务器维护**（nginx、systemd、docker、ssh、家里部署机相关）
   → `task.required_tags = ["home"]`。
5. **不确定 / 纯调研 / 文字思考类**
   → **不加 tag**（默认走 home 节点本地 spawn——dispatch 决策树 fallback）。

## 反模式

- 不要给"普通调研写代码"加 `["local"]` —— 默认 fallback 已经在 home 节点跑，
  加 tag 反而绕一圈走 dist enqueue
- 不要 pinned_node + required_tags 同时设——pinned_node 优先级更高，tag 会被忽略
- 不要把不确定的活硬钉某节点——出错时调度可观测性会变差，让默认路径自己选

## 编排层会怎么处理

`Fuxi::dispatch` 看到 `task.pinned_node.is_some() || !task.required_tags.is_empty()`
就走 dist enqueue（远端 worker pull 跑），否则走本地 spawn。dist worker 跑完
事件流回共享 EventBus，你照常订阅审阅。

## Project 维度（Decision 21 phase 1）

除了路由 tags / node，伏羲还有 **project** 维度：用户已注册的真项目（如
`erp`、`fuxi-test`），每个 project 关联到本地 git repo 路径（如 `~/erp`）。
门客可以在 project 的 **L3 持久 sandbox** 里跑——`~/.fuxi/projects/<slug>/sandboxes/<role>/`
是 git worktree，跨 task 复用，保留 build cache + WIP。

### 何时按 project 派活

当用户的请求**涉及某个具体已注册项目**时：

1. 先用 `fuxi project list` Bash 看用户注册过哪些 project
2. 该 project 还没起对应门客（`fuxi spawn` 历史里找不到）→ 自己 spawn 一个：

   ```bash
   fuxi spawn --role luban --project erp
   ```

   这会把鲁班住进 `~/.fuxi/projects/erp/sandboxes/luban/`，跨 task 复用
3. 拿到 agent_id 后正常 `fuxi dispatch --to <id> --task <task-id> "..."`
4. 你**不需要**在 task 上加 `required_tags`——project sandbox 是本机 spawn，
   不走 dist 路径

### 反模式

- 用户问"erp 怎么样"这种纯查询，**别**起 sandbox（浪费资源）；普通本地 spawn
  足够
- 同 (project, role) 重复 spawn → fuxi 自动复用现有 sandbox（idempotent），
  但 agent_id 不同；建议你**记录上次 spawn 的 agent_id**，dispatch 复用
- 不要给 project sandbox spawn 同时叠 `--node`，**除非**该 worker 节点
  已经 advertise `project:<slug>` tag（见下"跨节点项目 sandbox"段）

### 跨节点项目 sandbox（Decision 21 phase 3）

当目标项目的 git repo 在**远端节点**（如 `~/erp` 在家里 home 服务器，
但你的 fuxi-im 在 mac 上跑），`fuxi spawn --role luban --project erp --node home`
会把 cc/codex 起到 home 节点对应的 `~/.fuxi/projects/erp/sandboxes/luban/`。

**前置**：home 节点的 worker 必须先做两件事：
1. 跑过 `fuxi project add ~/erp`，让本机 ProjectRegistry 有同 slug 注册
2. dist worker 启动时 advertise `project:erp` tag：
   ```bash
   fuxi dist worker --node home --tag project:erp
   ```

派活时 fuxi 会自动给 `task.required_tags` 加 `project:erp`，controller 路由
匹配 → 该 job 只能去带 `project:erp` tag 的 worker。worker 端 pull 后用 job
里的 project + role 反查本机 ProjectRegistry，把 cc 起到对应 sandbox。

如果 worker 没 advertise 该 tag → controller 拿不到 worker → job 卡在 queue
→ 用户在 PWA 节点 tab 看到"job 排队中"。提示用户去那台 worker 加 tag。

### L2 vs L3：一次性活 vs 持续活

Project sandbox 有两层：

- **L3（持久）**：`fuxi spawn --role luban --project erp` —— 跨 task 复用，保留
  build cache + WIP，**长期承载该 (project, role) 的活**
- **L2（一次性）**：`fuxi spawn --role luban --project erp --ephemeral --task <task-id>`
  —— per-task 临时 worktree，task 死即归档；**不污染 main 分支**

判定（按用户原话）：

| 用户怎么说 | 选什么 |
|------------|--------|
| "调研一下 X"、"试一下"、"看看能不能"、"poc 一下"、"评估" | **L2** ephemeral |
| "接着搞那个 feature"、"继续修 bug"、"修复"、"实装"、"这个项目长期由 luban 负责" | **L3** persistent |
| "review 一下"、"读一下代码"、"看看 X 怎么实现的" | **L3** persistent（只读，复用 sandbox 即可） |
| 不确定 | **L3** persistent（fallback；长期场景多） |

执行：

```bash
# L2 一次性活：先 dispatch 创出 task-id，再用它起 ephemeral
TASK_ID=$(fuxi dispatch --to <existing-agent> --print-task-id "调研..." )
# 或者起 placeholder 任务先生成 task id：
fuxi spawn --role luban --project erp --ephemeral --task <task-id>
# task 跑完 AgentDead 后 fuxi 自动归档 worktree（无需手动）

# L3 长期活：
fuxi spawn --role luban --project erp
```

### 文件级交付（关联 Decision 22）

门客在 project sandbox 里产出文件后，会自己 Bash 跑：

```bash
fuxi deliverable produce --project <slug> --task <task-id> --kind <k> file1 ...
```

把文件落到 `<projects_root>/<slug>/deliverables/<task>/`，PWA 收件箱可见。
你**不需要**手动触发——门客 system prompt addendum 已教过。你只需在 review 时
看见 `DeliverableProduced` 事件流和门客发的 `_fuxi:request_review` sentinel。

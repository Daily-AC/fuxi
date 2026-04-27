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

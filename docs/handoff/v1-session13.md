# Handoff · v1 · Session 13 开工指引

> 上一 session（2026-05-06 凌晨）核心是 **v2 跨节点 sandbox 实装**：
> Project.host_nodes 字段 + IM HTTP 项目 API 扩展 + dispatch auto-pin 按节点
> 负载选最闲 + worker pre-spawn git fetch + mac `fuxi project join` 命令。
> 全部 TDD 红→绿→fmt→clippy 全绿。
>
> 上一份 handoff：`docs/handoff/v1-session12.md`（保留）。

---

## 1 · 5 分钟必读

1. `CLAUDE.md` · 七公理 + 「常见陷阱」（**新增条目**：本文 §4，建议下次同步）
2. 本文 §3「v2 跨节点 sandbox 现状 + 用户答辩前要做的事」
3. 本文 §2「上 session ship 了什么」+ §5「答辩演示 dry-run」

---

## 2 · 上 session ship 了什么

> 主线提交（紧跟 `4b276b0`）见 git log；本文按"逻辑面"汇总。

### A · vocabulary 层

- `crates/fuxi-core/src/project.rs` 加 `Project.host_nodes: Vec<String>`
  + builder `with_host_nodes`（dedup 保插入序）+ `#[serde(default)]` 兼容
  老 meta.json
- `crates/fuxi-core/src/task.rs` 加 `Task.project_id: Option<ProjectId>`
  + builder `with_project_id` + `#[serde(default)]` 兼容老 task JSON
- 单测：legacy meta/task 反序列化、roundtrip、dedup

### B · 注册表层

- `crates/fuxi-workspace/src/project_registry.rs` 加：
  - `add_host_node(id, node_id)`：登记节点（幂等）
  - `remove_host_node(id, node_id)`：移除节点（幂等）
- 都把更新落 `<root>/<id>/meta.json`，保持 SoT 一致

### C · IM HTTP 项目 API（前端 + worker 共用）

`crates/fuxi-im/src/handlers/projects.rs`：

| Method | Path | 用途 |
|---|---|---|
| GET | `/api/projects/{id}` | 单条读视图——mac join 时查 home 端 canonical_path |
| POST | `/api/projects/{id}/host_nodes` | 节点登记自己（请求体 `{node_id}`） |
| DELETE | `/api/projects/{id}/host_nodes/{node_id}` | 节点下线 |

`ProjectView` wire 加 `host_nodes` 字段（serde_default 兼容老前端）。

### D · 编排层 auto-pin

`crates/fuxi-orchestrator/src/node_load.rs` 新增：

- `NodeLoadSnapshot { node_id, inflight, max_concurrency, online }` +
  `saturation()` (= inflight/max_concurrency, max=0 归一为 1)
- `NodeLoadProvider` trait（同 DistEnqueuer 反向依赖 pattern）
- `pick_least_loaded(&[snaps], &[candidates]) -> Option<&NodeLoadSnapshot>`
  按 saturation 选最闲且 online

`Fuxi::auto_pin_from_project(&task)` 决策：

| 条件 | 行为 |
|---|---|
| `task.pinned_node.is_some()` | 返 None（用户意图优先） |
| `task.project_id.is_none()` | 返 None |
| 任一 provider/registry 未注入 | 返 None |
| `project.host_nodes.is_empty()` | 返 None（v1 单节点项目，旧行为） |
| 候选全离线 | 返 None |
| 否则 | `Some(picked.node_id)` |

`Fuxi::dispatch` 在 needs_dist 决策**前**调 auto_pin，命中即 `task.pinned_node = Some(picked)`，
然后自然走 dist 路径。

### E · worker 端 spawn-in-sandbox

`crates/fuxi-cli/src/dist.rs::resolve_project_sandbox_cwd`：在 `mgr.get_or_create`
**之前**调 `try_fetch_default_branch(canonical, default_branch)`：

- best-effort：`git fetch origin <branch>` 失败 log warn 继续，不挂 dispatch
- `FUXI_DISABLE_PRESPAWN_FETCH=1` 关掉（CI / dev）
- 单测：用 file:// remote 模拟两 repo + push 验证 origin/main 跟上

### F · `fuxi project join` 命令（mac 加入 home 项目）

`fuxi project join --slug <id> --controller <url> --token <t> --remote-url <git-url>
                   [--target <path>] [--node-id <id>] [--registry-root <p>]`

流程：
1. GET `/api/projects/<slug>` 拿 canonical_path/default_branch
2. `git clone --branch <default_branch> <remote-url> <target>`（target 默认从
   home canonical_path basename + `$HOME` 派生）
3. 本地 `registry.add` 登记
4. POST `/api/projects/<slug>/host_nodes` 通告自己（node_id 走
   `--node-id` → `$FUXI_NODE_ID` → `hostname` 链）

幂等：target 已存在跳过 clone，slug 已注册跳过 add。

### G · production 注入

`crates/fuxi-cli/src/im.rs` 在 `set_dist_enqueuer` 之后追加
`set_node_load_provider(DistNodeLoadProvider::new(dist_ctrl))`。
DistEnqueuer.enqueue 改用 `enqueue_with_project` 把 `task.project_id` 透传
到 DistJob.project，让 worker 端 `resolve_project_sandbox_cwd` 拿到。

---

## 3 · v2 现状 + 用户答辩前要做的事

### 现状（已 ship + 已测）

- ✓ Project.host_nodes 字段端到端
- ✓ HTTP API 完整（GET/POST/DELETE host_nodes，`scripts/v2-cross-node-test.sh` 走通）
- ✓ dispatch auto-pin 按 saturation 选最闲（5 单测覆盖各分支）
- ✓ worker pre-spawn git fetch（3 单测）
- ✓ mac join 命令（4 单测覆盖 helper）
- ✓ workspace 全绿：cargo fmt --check + clippy -D warnings + cargo test

### 答辩前用户要做的（不在我能干的范围）

1. **部署本 commit 到 home**：
   ```bash
   cargo build --release -p fuxi-cli
   scp target/release/fuxi home:~/.local/bin/fuxi
   ssh home 'systemctl --user restart fuxi-im'
   ```

2. **在 home 准备演示项目**（举例 `demo-site`）：
   ```bash
   ssh home 'cd ~ && mkdir demo-site && cd demo-site && git init -b main \
     && echo "<h1>fuxi 答辩</h1>" > index.html \
     && git add -A && git commit -qm seed'
   ssh home 'fuxi project add ~/demo-site --name demo-site'
   ```

3. **mac 上 join 项目**（`fuxi project join`）：
   ```bash
   ssh home 'python3 ~/.fuxi/im-mint-token.py' > /tmp/fuxi-token
   fuxi project join \
     --slug demo-site \
     --controller https://im.qmledmq.cn:8443 \
     --token "$(cat /tmp/fuxi-token)" \
     --remote-url ssh://home/home/e0-7/demo-site
   ```
   ↑ 跑完 mac 上有 `~/demo-site/`（git clone 来的）+ `~/.fuxi/projects/demo-site/`
   （L3 sandbox 骨架）+ home 端 `host_nodes` 含 mac。

4. **跑端到端**（任意一边发 task，玄女按 host_nodes 自动选最闲节点）：
   ```bash
   # PWA 或 im-test.sh 发：「@demo-site 给首页加个时间显示」
   # （--project 标志：fuxi dispatch / im /api/dispatch 都暴露 project_id 形参）
   ```
   - 玄女看 host_nodes=[home, mac] → NodeLoadProvider 拿 inflight → pick mac
     （home 当前忙度通常更大）→ task pinned_node=mac → dist enqueue → mac
     pull → mac 起 cc luban in `~/.fuxi/projects/demo-site/sandboxes/luban/`
   - cc 在 sandbox 改 index.html → push branch 回 home（git push origin <branch>）
   - 用户在 home 上 git checkout / merge → nginx 反代 demo-site.qmledmq.cn

### 已知差距 / 答辩可能被问到

- **task push back 没自动**：worker 跑完 task done **不**自动 `git push origin <branch>`
  back to home. 当前是 worker side spawn 时已 fetch 过，新 commit 留在 worker 本地
  branch；home 要看到 mac 的成果，要么 home `git fetch origin <task-branch>`
  （worker push 到 origin 后），要么用户手动 `ssh mac 'cd ~/demo-site && git push'`。
  **建议答辩 demo 用户先在 mac 上手 push branch 一次实证，避免现场卡**。
- **dispatch 接 --project 入口已通 但 PWA 还没 GUI**：用户 IM 输 "@demo-site 加个 X"
  这种 `@<slug>` mention parser 还没接，要走 `fuxi dispatch --project demo-site --to luban "..."`
  CLI 形态。PWA composer 的 mention chip 是后续 session 的事。
- **NodeLoadProvider 只看 inflight/concurrency**：CPU 负载等更细信号 v3 综合，
  当前对答辩规模够用。

---

## 4 · CLAUDE.md「常见陷阱」建议新增条目

下个 session 整理 CLAUDE.md 时同步：

- **加 Project / Task 字段必加 `#[serde(default)]`**：v2 加 host_nodes / project_id
  时手忙脚乱过——`registry.list()` 老 meta.json 全炸 deserialize error。**新字段
  必须 default 兼容旧持久化数据**（meta.json + sqlite events.db 里的 task JSON）。
  对应单测 `project_meta_deserializes_legacy_without_host_nodes` /
  `task_deserializes_legacy_without_project_id` 是反回归。
- **NodeLoadProvider 的 saturation = inflight/max_concurrency**：dist controller
  里 `register` 已对 `max_concurrency=0` 归一为 1（防 caller 传 0 把节点锁死），
  但 NodeLoadSnapshot::saturation 也独立 max(1) 守，避免靠依赖关系传染。
- **auto_pin_from_project 要对 task.pinned_node 做守卫**：用户显式 `@<node>` pin
  时 dispatch 不该悄悄改写。`Fuxi::auto_pin_from_project` 入口立即 short-circuit
  返 None；调用方只在 `task.pinned_node.is_none()` 时 set。两层保险，避免
  refactor 后某条路径漏判。
- **worker pre-spawn git fetch 必须 best-effort**：worker 短暂掉线（VPN 抖动 /
  ssh tunnel 断）时硬挂 dispatch 会让用户体感"任务无故消失"。fetch 失败 log warn
  继续——离线 sandbox 跑完 push 回去若 base 过期，git 自己会 reject，比这层
  挂友好。`FUXI_DISABLE_PRESPAWN_FETCH=1` 给 CI 开关。

---

## 5 · 答辩演示 dry-run（用户自己跑）

### 主秀脚本

1. 打开 PWA `https://im.qmledmq.cn:8443`，登入
2. 打开"项目"tab：列里有 `demo-site`，`host_nodes: [home, zyldemacbook-pro-local]`
3. 打开"节点"tab：home + mac 都 online，inflight 各几个
4. 在玄女对话页发：「给 demo-site 首页加个 visitor counter」
5. 玄女把 task 拆：前端（mac luban）+ 后端（home 鲁班）+ 部署（home 蒲松）
6. 看 mac 的工作页：cc 起在 `~/.fuxi/projects/demo-site/sandboxes/luban/` 干活
7. 看 home 的工作页：home 鲁班同时在自己 sandbox 干活
8. 等两边都交付：home 蒲松起 nginx 反代 + git pull 部署
9. 评委浏览 `https://demo.qmledmq.cn:8443` 看真页面

### 要给评委强调的话

- "门客在不同物理机器上跑——一个在我家服务器，一个在我笔记本"
- "玄女自己决定派给谁——比如笔记本忙就给服务器派活，反过来也一样"
- "代码是 git 共享的——每个门客在自己机器的隔离 sandbox 里改，最终 push 回主仓库"
- "现场你看到的演示站点，就是这套机制刚跑出来的"

### 现场卡了的应急话术

- 如果 mac 离线/网卡抖动 → 玄女自动只派给 home（候选离线时跳过）
- 如果 fetch 慢 → 不影响 task 执行，只是 base 不是最新（git 会 reject 过期 push）
- 如果 cc 跑挂 → bridge 自动归档 sandbox（`8a2e03e` + `e470a9a` 三连 fix 已加固）

---

## 6 · 改 EventKind 清单（沿用，强调）

加新 EventKind 变体一定同步 5+ 处。本 session **未加** EventKind；v2 跨节点
完全复用 `WorkspaceArchived` / `TaskCreated` / `TaskStateChanged` 等已有事件，
仅靠 task.project_id + task.pinned_node 路由——所以**没有跨节点专用事件**。
若后续要加 `ProjectHostNodesChanged` 之类（PWA 要实时刷 host_nodes 卡），按
sessions 11/12 同样的 5 处清单同步。

---

## 7 · 历史遗物（不阻塞主线，可顺手清）

继承自 v1-session12 §7：

1. **task-fb7437a8 cangjie-extract** 仍在 PWA 显示 `running`（dead agent）
   —— Bug 3 修只对新 task 生效。手动 `fuxi task cancel task-fb7437a8-...` 一刀清。
2. **sia/ephemeral/task-86106710-...** dir 仍躺在 disk —— 手动
   `mv ~/.fuxi/projects/sia/ephemeral/task-86106710-* ~/.fuxi/projects/sia/archive/`。

新增本 session 留下的：

3. **`scripts/v2-cross-node-test.sh`** 当前不验证 dispatch 真路由——只测
   HTTP API。完整端到端（home 派 task → 实际 mac 起 cc）需要 home 上手动跑一遍。
   下次 session 可以让 mac 端 fuxi 提供 `fuxi dist worker --dry-run` 跑通后纳入
   smoke 脚本。

---

## 8 · 协作笔记（写给下个 session）

- 用户偏好已落 memory：`feedback_full_bypass / feedback_keep_going /
  feedback_no_ceremonies / feedback_team_lead_batch_dispatch / feedback_tdd_required`
- **本 session 全程 TDD**：每条改动先红 → impl → 绿。`auto_pin_from_project` 5 测
  / `try_fetch_default_branch` 3 测 / IM HTTP 6 测 / registry mutators 4 测 /
  vocabulary 4 测。值得保留模式，下 session 接着这个节奏来。
- **v2 后续路线**（用户没拍板，留给下次 brainstorm）：
  - `ProjectHostNodesChanged` 事件 + PWA 节点卡实时刷
  - dispatch 时把 fetch 失败也 surface 给玄女（log warn 不够，应该让她知道 worker
    起的 base 可能过期）
  - mac 端 `fuxi worker push-on-done` daemon：监听 task done 后 auto `git push origin <branch>`
    回 home，省用户手动 push
  - worker workspace GC：当前 mac 端 sandbox 没人扫，长期会爆磁盘——L2 GC 只在 home 跑
- **agent team 模式没用**：本次单线串行（每个 lane 都 ≤200 行 + 紧密依赖前 lane），
  团队拆开会增 round-trip 弊大于利。下次若做"PWA composer mention parser + 后端
  routing 接 + e2e 测"这种 3 路独立活，再上 team。

---

## 9 · 改 session 命令快查

```bash
# 全绿门禁
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# v2 端到端 API
./scripts/v2-cross-node-test.sh

# mac join（要 home im running + token）
fuxi project join --slug demo-site \
  --controller https://im.qmledmq.cn:8443 \
  --token "$(ssh home python3 ~/.fuxi/im-mint-token.py)" \
  --remote-url ssh://home/home/e0-7/demo-site
```

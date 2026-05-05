# Handoff · v1 · Session 12 开工指引

> 上一 session（2026-05-05~06）核心是**收技术债**：用户报的"幽灵任务 + 门客
> 状态机 bug + L2 不归档"三连铁证，加 memory v2.1 必做两条（batch judge +
> review 类豁免）。全部 ship + home 实地验证通过。
>
> 上一份 handoff：`docs/handoff/v1-session11.md`（保留）。

---

## 1 · 5 分钟必读

1. `CLAUDE.md` · 七公理 + 「常见陷阱」（**新增条目见本文 §4**，建议下次同步进去）
2. 本文 §3「v2 跨节点 sandbox 实装路线」←下一 session 主线
3. 本文 §2「上 session ship 了什么」+ §5「答辩演示目标」
4. （选读）`docs/handoff/v1-session11.md` · memory-v2 三表分流原始设计

---

## 2 · 上 session ship 了什么

### 当前 main HEAD = `62bf5c5` ✅ 全绿，已部署 home

按 commit 顺序（旧→新）：

| commit | 内容 |
|---|---|
| `e470a9a` | fix(orchestrator): L2 ephemeral 在 task 终态自动归档（Bug 修） |
| `25a75ea` | fix(orchestrator): GC 跳过 xuannv 杜绝 shutdown 风暴（Bug 修） |
| `5354c79` | chore(scripts): im-test.sh curl 测试工具 + parser fmt |
| `8a2e03e` | fix(orchestrator): pump 无终态退出兜底 emit TaskCancelled（Bug 修） |
| `9e21a9b` | feat(memory-v2.1): batch judge + review 类豁免（性价比优化） |
| `62bf5c5` | fix(memory-v2.1): review 关键词只匹配 title 不扫 trajectory + task_title fallback |

### A · Bug 修三连（全部 home 实测验证）

| # | Bug | 真因 | 修法 | 验证 |
|---|---|---|---|---|
| 1 | xuannv shutdown 风暴（单实例 1830 噪音事件 / 15h） | GC 预发 AgentShuttingDown 不知道 xuannv 豁免 → 下 tick 又看到 idle 又发 | IdleGcTask 加 watch::Receiver<Option<AgentId>> xuannv_id，tick_once 跳过 | 部署后 14h+ xuannv 0 storm；luban/cangjie 各正常 GC 走 |
| 2 | L2 ephemeral 不归档（sia/L2/task-86106710 在 disk 躺 3 天） | archive 只触发于 AgentDead；门客被 idle GC 走时不发 AgentDead → workspace 永远不归档 | bridge.rs::TaskStateChanged{Done\|Cancelled} 第二条触发器调 archive_l2_for_task；archive() 已幂等不冲突 | 实测 task-e50aee4b 完成后 workspace_archived 事件触发 + 物理 move 到 archive/ |
| 3 | 幽灵 task（agent 死了 task 永远 running） | dispatch pump 进程崩溃 / rx 关闭时无终态事件，task 卡 InProgress | pump 退出处加 `if !saw_terminal` 兜底 emit TaskStateChanged{Cancelled} | 单测 ControllableAgent + drop sender 验证 |

### B · memory v2.1（性价比优化）

- **batch judge**：1 task done 起 1+1 cangjie cc（旧 1+N，N 可达 5+）。
  默认 `cfg.batch_judge=true`，env `FUXI_INSIGHT_BATCH_JUDGE=0` 关。条数不符 /
  解析失败自动回退老逐条路径兜底，不会丢分。
- **review 类豁免**：审阅 / 校验类 task 几乎无可迁移 insight，should_skip_review
  扫 task title 命中关键词即跳过 process_task。
  - **只看 title，不扫 trajectory**——sentinel JSON `request_review` 字串
    会误命中（home 实测 task-e0a361b7 撞过）
  - `task_title_from_history` 加 TaskCreated.title fallback——luban 子任务无
    UserPrompted 事件时也能拿到 dispatch title
  - env `FUXI_INSIGHT_REVIEW_KEYWORDS` (CSV) 可覆盖默认清单

### C · 新工具 `scripts/im-test.sh`

curl 包装让 Claude 自驱验证，免用户手测：

```bash
./scripts/im-test.sh say "<text>"        # 给玄女发消息
./scripts/im-test.sh read [limit]         # 读对话
./scripts/im-test.sh nodes               # 节点 + worker 状态
./scripts/im-test.sh tasks                # 任务树
./scripts/im-test.sh tail-poll [ms]       # 不靠 ws 的轮询版实时流
./scripts/im-test.sh kill <agent_id>      # ssh home fuxi kill
./scripts/im-test.sh events <kind> [hrs]  # 直查 events.db
./scripts/im-test.sh mint                 # 重签 token
```

实装：home 上 `~/.fuxi/im-mint-token.py` 用 `~/.fuxi/im_hmac.key` 签 30 天 token。
middleware 只验签 + 过期，不查 device_tokens 表 → 自签合法。token 缓存
`/tmp/fuxi-im-token`，401 自动重签。

---

## 3 · v2 跨节点 sandbox 实装路线（**下 session 主线**）

### 用户诉求（原话节选）

> 答辩的时候我打算演示 home 的 cc 和 mac 的 cc 一起协作做一个小项目，
> 工作区不乱，可交付就行。我初步想法是做一个网页，然后绑一个子域名上线，
> 直接让评委老师现场用。
>
> emm 不是不是，我以后开发真会这么用，所以别偷懒，我们要把这个补齐，
> 分布式的优势是资源负载均衡别忘了。

### 现状

- ✓ **跨节点 dispatch 已通**：home 玄女 → `--pinned-node zyldemacbook-pro-local`
  → mac dist worker pull → mac 起 cc luban → 跑活回流 home。
  实证：2026-05-06 03:16:32 mac luban ls ~ 输出对得上 mac 本机文件名
  （`_refs / 2026-03-03-185246-ssh-hktail.../...`）。
- ✓ home 单节点多 sandbox（L2/L3）已实装并经本次修复加固
- ✗ **mac 上没 project sandbox**——dist worker 接 task 后裸起 cc，cwd = mac
  worker 启动 cwd（即 `/Users/e0_7`）。两个 mac luban 改同一项目互相踩。
- ✗ home 上的 `fuxi project add` 注册元数据**不**同步到 mac
- ✗ 项目代码同步机制不存在（home 改 → mac 看不见，反之亦然）

CLAUDE.md 明示：「跨节点项目 sandbox（v2 待实装，**当前不可用**）」。

### 设计目标（我的草图，待用户校准）

**最小可演示版**（答辩重点 + 后续真用）：

1. **mac 也能注册 project**：
   - `fuxi project add` 命令在 mac 端能用（fuxi-cli 走 dist controller URL 注册到 home 共享存储）
   - 注册的不只是路径，还有 **host_node**——project 元数据带「这个项目代码在哪台节点」
   - home 上的 fuxi project list 显示所有节点的项目（带 `[node=mac-local]` 标注）

2. **dispatch 路由按 project.host_node**：
   - 玄女 dispatch task 到某 project 时，编排层查 project.host_node
   - 自动 pinned_node = host_node（玄女不用手写 `--pinned-node`）
   - target node 上 spawn 在 `~/.fuxi/projects/<slug>/sandboxes/<role>/`（L3）
     或 `ephemeral/task-<id>/`（L2）—— mac 端走 mac 自己的 ~/.fuxi

3. **跨节点 worker spawn**：
   - dist worker 收到 dispatch 时，若 task 关联 project + project.host_node = self
     → 在自己的 ~/.fuxi/projects/<slug> 起 cc + git worktree
   - 否则原行为（裸起 cc）

4. **资源均衡**（用户重点提）：
   - project.host_node 不应是单值——可以是允许的节点列表（如 `[home, mac]`）
   - 如果项目同时存在多节点（git mirror 同步过），dispatch 时选**当前最闲**那个
   - 信号源：`/api/nodes` 已有 `inflight_jobs` + `max_concurrency`
   - 最简实装：选 inflight_jobs/max_concurrency 比值最低的节点

### 切入点 / 关键文件

- `crates/fuxi-core/src/project.rs` — Project 结构加 `host_nodes: Vec<String>`
- `crates/fuxi-projects/`（新增 crate？或挤进 fuxi-workspace） — 跨节点
  project registry 的 controller / worker 端协议
- `crates/fuxi-cli/src/dist.rs` — dist worker pull 时检查 task.project，spawn 进
  自家 sandbox
- `crates/fuxi-orchestrator/src/fuxi.rs::dispatch` — task 关联 project 时
  自动选 host_node 路由（结合 inflight_jobs 选最闲）
- IM HTTP API 加 `POST /api/projects` 让 mac fuxi-cli 注册到 controller
- IM HTTP API 加 `GET /api/projects/<slug>` 让 dispatch 路由查 host_nodes

### 决策点（用户拍板）

| 问题 | 选项 |
|---|---|
| **代码同步**：home 和 mac 上的 project repo 怎么同步？ | A. 单 host node（最简，没真协作）<br>B. git mirror via push/pull（中等复杂）<br>C. fuxi 内置 git sync 协议（重） |
| **容量调度信号**：选最闲节点用什么指标？ | A. `inflight_jobs / max_concurrency`（已有）<br>B. CPU 负载（要 worker 上报新指标）<br>C. 两者综合（v3） |
| **新 crate 还是塞 fuxi-workspace？** | 个人倾向加 crate `fuxi-projects`（dist 协议层独立）|
| **答辩演示用哪种代码同步？** | 倾向 B 但只对 main branch 自动同步，避免复杂；用户答辩前定 |

### TDD 入口（推荐先写测试）

- `Project::with_host_nodes(["home", "mac-local"])` 单测
- dispatch 路由：mock NodesView with inflight_jobs 比例 → 期望选哪个节点
- worker 端 spawn-in-sandbox：mock project lookup 返回 (slug, root_path) →
  期望 cwd 落点

---

## 4 · CLAUDE.md「常见陷阱」建议新增条目

下个 session 整理 CLAUDE.md 时同步：

- **GC 必须知道 xuannv 豁免**（Bug 1 教训，commit `25a75ea`）：shutdown_agent
  对 xuannv silent return Ok 是不够的——GC tick_once 在**预发**
  AgentShuttingDown 之前就要跳过 xuannv，否则 30s 一次永远循环。8/8 storm
  agent 实证。
- **L2 归档要双触发器**（Bug 2 教训，commit `e470a9a`）：bridge.rs 在
  AgentDead 路径之外还要在 TaskStateChanged{Done|Cancelled} 路径触发，因为
  门客被 idle GC 走 / 状态机别的退出方式不发 AgentDead，但 task 已 done。
- **dispatch pump 退出必判 saw_terminal**（Bug 3 教训，commit `8a2e03e`）：
  rx 关闭 / agent 进程崩 → pump 退出 + task 永卡 running。pump 退出兜底
  emit TaskCancelled。
- **review 关键词扫 trajectory 体会被 sentinel JSON 误命中**（v2.1 fixup
  教训，commit `62bf5c5`）：`{"_fuxi":"request_review",...}` sentinel 含
  "review" 字面，trajectory 头部包含它。只扫 title 是对的。

---

## 5 · 答辩演示目标（用户期望）

### 主秀目标

home cc + mac cc **真协作**做一个小项目，工作区不乱，可交付，绑子域名上线，
评委现场用。

### 隐含目标

- **分布式资源均衡**：演示出"任务自动分给当前最闲节点"才显出分布式价值
- **真用**：用户答辩后会持续这么用，不是 demo throwaway
- **诚实展示路线**：v2 跨节点 sandbox 是新做的，但要 production-grade

### 目标项目候选

用户原话："做一个网页，然后绑一个子域名上线"。具体项目内容待定。建议：

- 静态网页 + 简单后端（评委可点击交互的）
- 用 home / mac 各自擅长的活分工：mac 管前端（vite dev server / Tailwind），
  home 管后端（rust API + 数据库 + nginx）
- 评委 demo URL：`xxx.qmledmq.cn`（home 上加 server block + DDNS 全配齐了）

---

## 6 · home 部署快照（2026-05-06）

```
binary:    /home/e0-7/.local/bin/fuxi  (62bf5c5，Bug 1/2/3 + memory v2.1)
PWA:       /home/e0-7/.local/share/fuxi/im-web  (上次部署，未改)
events.db:
  oracle_facts:    继续累积
  user_profile:    待用户主动 record（仍 0 条）
  hetu_patterns:   14 条（部署前累积）。新规则：batch judge + review 豁免
  门客:             玄女单一活动（agent-5321afe1，部署后没风暴）
public URL: https://im.qmledmq.cn:8443
mac dist worker: zyldemacbook-pro-local，online，已实证接活跑通
```

**已注册 project**：仅 `sia` (`/home/e0-7/sia`)，**仅 home 节点**。

**nginx 模板**：home 上 11+ 子域名实战例子（im / blog / fuxi / sia / play / lab /
story / chat / bf / kaiwu / tmp-yxl），子域名 + systemd unit 加新模板熟练。

---

## 7 · 历史遗物（不阻塞主线，可顺手清）

1. **task-fb7437a8 cangjie-extract** 仍在 PWA 显示 `running`（agent-29:dead）
   —— Bug 3 修只对 forward 新 task 生效，旧 leftover 没人触发 pump 退出 →
   永远卡。手动 `fuxi task cancel task-fb7437a8-...` 一刀清，或者下次 session
   做"启动时一次性扫描 orphan 修复"（≤30 行）。
2. **sia/ephemeral/task-86106710-...** dir 仍躺在 disk —— task 在 fix 前已 done，
   bridge 没听到 TaskStateChanged。手动 `mv ~/.fuxi/projects/sia/ephemeral/task-86106710-* ~/.fuxi/projects/sia/archive/` 即可。

---

## 8 · 协作笔记（写给下个 session）

- 用户偏好已落 memory：`feedback_full_bypass / feedback_keep_going /
  feedback_no_ceremonies / feedback_team_lead_batch_dispatch / feedback_tdd_required`
- **用户希望被反驳**——v2 设计如果遇到本能想"偷懒"的诱惑（比如说"答辩
  够用就行"），用户会反驳。把分布式资源均衡 / production-grade 当硬要求。
- **TDD 硬规矩**——本 session 已贯彻（每条 fix 先写单测红 → impl → 绿）。
  v2 跨节点 sandbox 是协议层 + 文件系统 + 网络 IO 复合，更需要 TDD 拆。
- **agent team 模式**适用于 v2 实装：可以拆"协议设计 / dist worker 端 /
  controller 端 / e2e 测试"四路并行（按 CLAUDE.md `feedback_divide_conquer`
  + `feedback_team_lead_batch_dispatch`）。
- **im-test.sh 已就位**——下次 verify 跨节点 sandbox 时不用再要求用户手测，
  我自己 curl + ssh 就能跑通。

---

## 9 · 改 EventKind 清单（沿用，强调）

加新 EventKind 变体一定同步 5+ 处：

1. `crates/fuxi-core/src/event.rs` — 变体定义 + serde 字段
2. round-trip 测块 `tag_and_roundtrip` 加 case
3. `crates/fuxi-events/src/store.rs::kind_tag` — 持久化标签
4. `crates/fuxi-firehose/src/{hub,tui}.rs` — Hub 转发 + summarize/color
5. `crates/fuxi-cli/src/subcommands.rs::event_summary` — CLI 文字
6. （若入对话视图）`crates/fuxi-im/src/handlers/{tasks,workers}.rs::*_visible`
7. （若 PWA 渲染）`crates/fuxi-im/web/src/messages.ts` 三 reducer + 渲染 switch

v2 跨节点 sandbox 可能加新 EventKind（`ProjectRegistered { node }` /
`WorkspaceRoutedTo { node }` 等），按上面清单同步。

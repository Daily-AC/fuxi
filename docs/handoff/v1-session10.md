# Handoff · v1 · Session 10 开工指引

> 上一 session（2026-05-04）做的是「**handoff 留尾 + CI 全绿**」短小冲刺：
> P2.7/P2.8/P2.9 三个 P2 全 ship + GitHub Actions CI 修通跑绿。
> 没碰真业务路径，只补基础设施 + 一个新功能（inline 文件推送）。
> 上份 handoff: `docs/handoff/v1-session9.md`（保留）。

---

## 1 · 5 分钟必读

1. `CLAUDE.md` · 七公理 + 「常见陷阱」（**新增 clippy 1.95 + fmt 教训**）
2. 本文 §2「ship 了什么」§3「未测的实测路径」§4「下一动作」
3. （选读）`docs/decisions/22-deliverables-storage.md` —— P2.7 inline push 是
   它的姊妹概念，看一眼能理解分工

---

## 2 · 上一 session ship 了什么（按主题分）

### A · CI 全绿（5 commit）

| commit | 内容 |
|--------|------|
| `bff9b8a` | clippy 1.95 `question_mark` lint 修 3 处（fuxi-workspace `let Err(e) = X { return Err(e); }` → `X?;`）+ ci.yml 加 `pwa typecheck + lint + test + build` job |
| `7053425` | clippy 1.95 `unnecessary_sort_by` on DateTime<Utc> → `sort_by_key(Reverse(x))` |
| `3f28039` | `cargo fmt --all` 收紧 5 文件（CI 1.95 比本地 1.93 严） |
| `519a8c0` | `tasks.member_status_after_task_done_reflects_shelf_liveness` 测期望对齐用户 2026-05-04 反馈（dead vs idle 区分 GC 状态） |
| `b7a9d99` | bridge `cc_received_wakes_xuannv` 期望对齐新 prompt 标识 `[CC · 仅知情]`（之前测留 `[CC]` 旧字串） |

**关键教训**（写入 CLAUDE.md "常见陷阱"前可参考）：
- 本地 rustc 1.93 vs CI rustc 1.95 lint 对不齐——重跑 CI 是唯一保险
- `cargo fmt --all` push 前必跑（CI 那条 fmt --check 没机器跑过验证）

### B · P2.8 一键安装入口（commit `420cfc2`）

- `.github/workflows/release.yml` —— tag `v*` 触发，cross-build
  fuxi-cli for `linux-x86_64` + `macos-aarch64`，tar.gz + sha256 推
  GitHub Release。手动 `workflow_dispatch` 也行（dry-run 用）。
- `scripts/install-local-worker.sh` —— PATH 缺 fuxi 时自动从 latest release
  拉对应 platform asset，sha256 校验后装到 `~/.local/bin/fuxi`。
  下载失败 fallback 提示 `cargo install`。
- **未测**：还没真发过 release。下次先 `git tag v0.1.0-test && push --tags`
  跑一次确认 workflow 通。

### C · P2.9 install.sh `--auto-takeover`（commit `420cfc2` 同批）

- `deploy/im/install.sh` 加 `--auto-takeover` 标志：preflight 失败时不 abort
  而是接管：
  - **a** 残留 fuxi 进程 → `systemctl stop fuxi-im` + `pkill -f "fuxi "`
  - **c** 已有 `sites-enabled/im` → `cp .bak.<ts>` 后覆盖
  - **d** server_name 命中全是自家 `sites-enabled/im*` → 放行
- 端口冲突 (b) + **第三方** server_name 占用 仍 abort——那是真冲突。
- **未测**：home 早就装过了，先要找台全新机器才能验。低优先。

### D · P2.7 「轻量文件推送」（commit `74e2022`）—— **真功能**

新 `EventKind::AgentInlineMessagePushed { task, from, filename, mime, body }`，
让门客把小段 markdown / 纯文本（≤ 256KB）直推到任务对话流，不进 deliverable
收件箱、不需 accept_to 落地。

**Wire 路径**（4 处 sync point）：
- `fuxi-core/event.rs`: 新变体 + serde tag round-trip 测
- `fuxi-events/store.rs`: kind_tag = `"agent_inline_message_pushed"`
- `fuxi-firehose/hub.rs` + `tui.rs`: kind_tag + `note ← {agent} ...` summary +
  LightCyan 配色
- `fuxi-im/handlers/tasks.rs` + `workers.rs`: `task_thread_visible` /
  `worker_event_visible` 白名单各加该变体（不加任务 thread 看不到）
- `fuxi-cli/subcommands.rs::event_summary`: TUI 文字格式

**CLI**（`crates/fuxi-cli/src/note_cmd.rs`，全新文件）：
```bash
fuxi note --task <id> [--from <agent-uuid>] [--mime ...] <file>
```
- mime 默认按扩展猜（`.md`/`.markdown` → `text/markdown`，余 `text/plain`）
- `--from` 缺省 → events.db 反查 task 最近一次 `TaskDispatched.to`
- 上限 256KB，超限提示走 `fuxi deliverable produce`

**PWA**：
- `messages.ts` 新 `InlineFileMessage` 类型 + `applyTaskThreadEvent` /
  `applyWorkerEvent` 各加 reducer 分支（含 id 去重）
- `InlineFileCard.tsx` + `.module.css` —— 左侧 worker 侧卡片，header
  filename + mime + size + time，body markdown 走 `<Markdown />`，plain 走
  `<pre>`。视觉同 WorkerBubble 色温（橙系）但加边框 + 更宽（92% vs 78%）
- `TaskThreadPage` / `WorkerPage` 渲染 switch 加 `inline_file` 分支
- `WorkerReducerCtx` 加 `role` 字段（之前只有 `role_display`）

**测试**：
- 后端 round-trip (fuxi-core)、CLI parse 测、PWA reducer 2 条（基本渲染 + 同 id
  去重）—— 都本地通 + CI 通

---

## 3 · **未测**的实测路径（最重要）

P2.7 真功能上线，但**没在 home 实测**。下次 session 第一件事应该是：

```bash
# 1. 部署到 home（一句话脚本）
./scripts/deploy-home.sh --apply

# 2. ssh home 找 task id
ssh home 'sqlite3 ~/.fuxi/im.db "select id, title from tasks limit 5"'

# 3. ssh home 准备 md 并推
ssh home '
  cat > /tmp/note.md <<EOF
# 鲁班发现

代码读完，主要发现：
- foo 函数返回值类型不一致
- 测试覆盖率 67%

\`\`\`rust
fn example() -> Result<()> { Ok(()) }
\`\`\`
EOF
  fuxi note --task <task-id> /tmp/note.md
'

# 4. 手机 PWA → 任务列表 → 进那个 task
# 期望：左侧"鲁班 · note.md (text/markdown · 234 B) 14:32"卡片
# 下面渲染了 md 全样式
```

**异常路径要测**：
- 文件 > 256KB → 报错指引走 deliverable
- `--from` 不传 + task 没 `TaskDispatched` → 报错让人手传
- mime 强 `--mime image/png` → v1 只接 text/\* 拒之

**P2.8 release 流水线也没真跑过**：
```bash
git tag v0.1.0-test && git push origin v0.1.0-test
# https://github.com/Daily-AC/fuxi/actions 看 release workflow
# 期望 ~5min 后 4 个 asset（linux + macos × tar.gz + sha256）
```

---

## 4 · 下一动作（优先级排）

### P0 · 教玄女用 `fuxi note`（**最高，但很轻**）

P2.7 platform 通了但**玄女不会用**。需要在
`roles/xuannv/instructions/dispatch-routing.md`（或新写一个 skill）加教学：

> 「门客跑出来的小报告（< 256KB markdown / plain）应该用 `fuxi note --task
> <id> <file>` 推到对话流；只有要落地用户磁盘的工件才走 `fuxi deliverable
> produce`。」

也可考虑改门客（luban / ε）的 `instructions/`，让它们**自己**判断「这个该
deliver 还是该 note」。

**估时**：S（写 prompt 段落 + 跑 `fuxi xuannv refresh` 重启会话即可）

### P1 · session9 留下来还没处理的旧 P0 (复盘)

session9 handoff §3 里写过几条 P0/P1，回看一下哪些这次没碰：
- **P0.A 玄女不主动派活** —— 还是 instructions 问题，跟上面 P0 同源
- **P0.C 任务 thread 工具卡 overlap** —— 已在 commit `26d851f` / `103f019`
  附近修过几轮，再过一遍 PWA 看是否复发
- 其它 session9 列的 5 个 P0/P1，对照 git log `bug #76` / `bug #77` 看哪些已
  解决；剩余的进 §3 实测同时验

### P2 · session9 列的其它 P2（已无新增）

session9 handoff 把 P2.7/8/9 列出来，这次全干完了。继续推 P3+ 之前先实测 P2.7
和清 P0/P1。

---

## 5 · 部署快照（home 现状 2026-05-04 18:30）

```
binary:    /home/e0-7/.local/bin/fuxi  (5 月 4 日 17:58 编译，含
           bff9b8a 之前的所有 commit；P2.7 / P2.8 / P2.9 / CI 4 commits
           **未部署**——下次 deploy-home.sh --apply 即可)
PWA:       /home/e0-7/.local/share/fuxi/im-web  (同上)
玄女:      上次 fresh session 之后跑了几轮，记忆会沿用 cc resume；
           教 P2.7 后要 `fuxi xuannv refresh` 让她重读 instructions
project:   sia → /home/e0-7/sia, default_branch=master  ✅
sandbox:   /home/e0-7/.fuxi/projects/sia/sandboxes/luban → 上次留的，可复用
```

---

## 6 · CI 现状（2026-05-04 18:35 实测）

`https://github.com/Daily-AC/fuxi/actions` 最近 5 次 run：
```
74e2022 (P2.7)              ✅ 全绿
b7a9d99 (bridge test 修)    ✅ 全绿
519a8c0 (tasks test 修)     ❌ test 1 个挂（bridge `[CC]` 旧字串），下一 commit 修
3f28039 (fmt 全跑)          ❌ test 1 个挂（同上）
7053425 (clippy sort_by)    ❌ test 1 个挂（tasks.member_status 旧期望）
```

**当前 main HEAD = `74e2022` ✅ 全绿**。可以放心从这开。

---

## 7 · 协作笔记 / 用户偏好

（来自这次 session 的实际反应，写下来给下个 session 不踩同坑）

- **用户对"上下文漂移"敏感**：开始觉得不对就喊停。下次 session 要主动管控
  context（25% 时主动建议 handoff，超过 35% 警告）。
- **用户希望 ship 时主动提供测试方案**：见 §3「未测的实测路径」格式——
  ssh + sqlite + 手机三步式，不是 "请测试一下"。
- **user 关心可读性 > 工程纯净**：P2.7 的 InlineFileCard 用了独立卡片+边框
  没让它沿用 WorkerBubble 同款气泡，因为 inline file 一般是技术内容更适合
  独立框；下次类似选择继续按"用户阅读体验"优先。

---

## 8 · 改 EventKind 的清单（**再强调一次**——P2.7 的清单存档给后人）

加新 EventKind 变体一定要同步 5+ 处，否则 clippy `-D warnings` 会一处处报：

1. `crates/fuxi-core/src/event.rs` —— 变体定义 + serde 字段
2. `crates/fuxi-core/src/event.rs` 里的 `tag_and_roundtrip` 测块 —— round-trip
3. `crates/fuxi-events/src/store.rs::kind_tag` —— 持久化标签
4. `crates/fuxi-firehose/src/hub.rs::kind_tag` —— Firehose hub 转发标签
5. `crates/fuxi-firehose/src/tui.rs` —— `event_summary` + `color_for` 双处
6. `crates/fuxi-cli/src/subcommands.rs::event_summary` —— CLI 文字
7. （若该事件该入对话视图）`crates/fuxi-im/src/handlers/tasks.rs::task_thread_visible` +
   `workers.rs::worker_event_visible` 白名单
8. （若 PWA 渲染）`crates/fuxi-im/web/src/messages.ts` 三个 reducer +
   `tests/unit/messages-task-thread.test.ts` + 渲染 switch

P2.7 commit `74e2022` 是这个清单的标准范例，可参考。

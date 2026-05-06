# Handoff · v1 · Session 16 → 17 开工指引

> 上 session（2026-05-06 晚 ~ 2026-05-07 凌晨）核心是 **bug 修复 + 通知 tab + bug
> 收集器一线通**。用户实测 4 个 bug 全修；玄女撞 fuxi 平台 bug 自己跑
> `fuxi bug report` 落档，PWA「通知」tab 集中消费。
>
> 上一份 handoff：`docs/handoff/v1-session15.md`（保留，§4 macOS Gatekeeper
> codesign 坑 + ssh home ProxyCommand 抖动 + dist HMAC 签名脚本路径仍有效）。

---

## 1 · 上 session ship 了什么（HEAD `4b51560`，全绿）

| commit | 内容 | 真测验证 |
|---|---|---|
| `d8c4462` | fix · 用户实测 4 个 bug（@dead-worker 503 / dist task 卡 running / home 4/4 but no workers / @autocomplete 项目段无 label） | home fuxi-im PID 2522198 + PWA 14/14 intervene 测 + 333/333 全绿 |
| `4b51560` | feat · 通知 tab + bug 收集器一线通 | home PID 2676402（md5 `b04332de`）+ ssh home `fuxi bug report` 落档 sqlite3 验通 |

### 1.1 bug 修复 4 件

- **@dead-worker 报 503「玄女后端不在」**：handlers/intervene.rs 入口加 target alive check，dead → 自动 fallback 到玄女 + prepend「[用户原本 @ 了门客 X，已下线（可走 fuxi spawn --recall-task 召回）。请你接续答复用户。]」
- **dist 路径 task 永远卡 running**：dist.rs::DistController::report 按 removed_job.task_id 翻译 dist 终态 → emit `TaskStateChanged{InProgress→Done|Cancelled}`。home 端 dispatch 走 dist enqueue 后 task 没 lifecycle 终态的真因：worker 只 emit AgentSpawning/AgentDead 不动 task lifecycle，home pump 在 dist 路径根本不跑（dispatch 走 enqueue 直接 return）
- **home 节点 4/4 但 workers:[]**：im_dist.rs 让 home 节点 union shelf + dist_workers_from_events，dedupe 按 agent_id。home 是 dual-role 节点（shelf 常驻门客 + v1-session14 起内嵌 dist worker 起 per-job cc 子进程），原码只看 shelf 漏掉内嵌 worker 的 cc
- **@ autocomplete 项目段缺 label**：MentionAutocomplete 三段排序 worker → project → node，加「项目」/「节点」separator

### 1.2 通知 tab + bug 收集器

- 后端：
  - `migrations/0005_notifications.sql`：通用 schema（kind/severity/title/body/task_id/agent_id/metadata/created_at/read_at/closed_at），将来 review_request / context_handoff_offer / system 都同表
  - `fuxi-im::notifications NotificationStore`：insert/list/unread_count/mark_read/close/mark_all_read（10 单测全绿）
  - `/api/notifications` GET + `/{id}/read` POST + `/{id}/close` POST + `/read-all` POST
- CLI：`fuxi bug report --title --body [--severity bug|warn|wish] [--task] [--agent]`
  - 直开 `~/.fuxi/im.db` SQLite 写——避免 cc subprocess 跟 PWA cookie auth 打交道；WAL 多写并发安全
  - bug → severity=error 红 / warn → warn 黄 / wish → info 蓝
- 玄女 system prompt（`roles/xuannv/instructions/dispatch-routing.md` 末尾）加必读段：何时跑 + 反模式（不把业务 task 失败当 bug）
- PWA：临时 6 tab，第 6 位是「通知」+ 红点 badge（15s 轮询 unread_count，进 tab 自动 mark_all_read）；NotificationsPage 列表 + kind label + 三色 dot + 关闭按钮 + 「显已关闭」toggle

---

## 2 · 用户拍板的设计决策（下 session 必读）

### 2.1 玄女上下文管理（task #8 必备）

- **触发模式 Q1 = 半自动 (B + C)**：35% 玄女自约束「长话短说」+ 45% 玄女主动问用户「我 context 用了 X%，要不要重启副本？」让用户拍板。**不**自动切换（用户当下可能在讨论重要决策不想被打断）
- **handoff 内容 Q2 = 短版 ≤500 字**：当前活跃 task + 待用户拍板事项 + 用户近期偏好。不要重建上下文（伏羲后端 EventBus + 任务状态本就持久），只补「玄女脑子里的软知识」
- **交接时机 Q3 = 等 idle 再切**：当前 turn 跑完 idle 后再 kill old + spawn new；不打断 mid-turn

### 2.2 PWA UI 设计（task #9 必备）

- **方案 A：4 tab + 「更多」hub**
  - 底部 tab：玄女 / 任务 / 通知 / 更多
  - 「更多」hub 进二级页：节点 / 项目 / 工作者 / 交付物 / 记忆（新）/ 角色（新）/ 更漏（新）/ 设置
- 「通知」提一级是 first-class concern——每天看红点 badge → 一眼知道有哪些事等我（玄女主动汇报的 bug + 门客审阅请求 + 上下文 handoff offer）

### 2.3 Bug 收集 = 玄女工具（已 ship task #7）

- 玄女撞到 fuxi 平台 bug 自己跑 `fuxi bug report` 落档
- 用户视角：进通知 tab 一目了然，无需用户在 PWA 单独写 bug 报告框

---

## 3 · 下 session 推荐起点（用户拍）

用户在 session 16 末尾问"先做 #8 还是 #9？"我建议 **task #9 hub 重构优先**：
- 现在 6 tab 移动端真挤，重构后 task #8 emit 的 `context_handoff_offer` 通知走到稳定的「通知」tab
- task #8 的 context 监测 + handoff 流程逻辑量大（200-300 行 + 实测要等玄女真撞 35%/45%），放在 UI 稳定后做更顺
- task #9 顺带新加 **记忆 / 角色 / 更漏** 三个二级页面（每个 50-100 行 SolidJS）

用户尚未拍优先级——下 session 先确认。

---

## 4 · Task #8（上下文管理）实装路线

实装规模 ~250-350 行 + tests。分四块：

1. **fuxi-events**：`XuannvContextWatermark` 事件（informational，记 % + tokens）+ `XuannvHandoffWritten` 事件
2. **fuxi-orchestrator/fuxi.rs**：玄女 dispatch pump 看 cc result events 里的 usage（input + cache_creation + output，**不**算 cache_read）累加到 shelf state；阈值 35%/45% 跨过时触发不同动作
3. **fuxi-cli**：`fuxi xuannv handoff write/read` CLI 命令；fuxi-im 启动期检测 `~/.fuxi/xuannv-handoff.md` 落档（fs_watch 或 simple polling）触发 kill old + spawn new + inject prelude
4. **fuxi-agent-cc**：spawn cc 时支持 `--prelude-handoff <path>` 拼 system prompt 头部

详细：
- **35% 触发**（350k tokens）：玄女 system prompt addendum 注入「你 context 已用 35%+，请长话短说，未必每事都展开；遇到必要细节优先 Bash + Read 不复述全文」。**只在跨阈值的下一 turn 注一次**（不每 turn 重复）
- **45% 触发**（450k tokens）：等玄女 idle → emit 一条 `system_origin` intervene：「你 context 用了 X%（Y tokens），建议交接。请现在跟用户说：'我 context 用了 X%，要不要重启副本？'。等用户'换'再写 handoff，'继续'就先放着。」+ PWA 通知 tab 加一条 `kind=context_handoff_offer` 通知给用户视角对齐
- **handoff 写**：玄女拿到用户「换」的回复后，自己 Bash 跑 `fuxi xuannv handoff write '<≤500字 markdown>'` 落档 → 后端检测落档 → wait turn idle → kill old cc + spawn new cc 注入 prelude（spawn cc 的 `append-system-prompt` 拼 handoff 内容前面）→ 新玄女接班开始 emit「✻ 上下文已交接 · 新副本接班」系统消息到 conv

token 累加位置：cc result event 里有 `usage`。看 `fuxi-agent-cc` 是否已经 expose；没有就要扩 CcEvent → 在 dispatch pump 累加。

---

## 5 · Task #9（4 tab + 「更多」hub）实装路线

实装规模 ~400-500 行 + tests。分四块：

1. **App.tsx + ApiProvider**：TabIndex 改 0..3（4 tab）；BASE_TABS 改成 [玄女, 任务, 通知, 更多]；现有 5 个 tab page 路由照旧但用 navPush 进入而不是 tab 切换
2. **新 MorePage** 二级 hub：tile grid（卡片：节点 / 项目 / 工作者 / 交付物 / 记忆 / 角色 / 更漏 / 设置）。tap 卡片 navPush 到对应 sub-page
3. **三个新 page**（每个 ~100 行）：
   - `MemoryPage`：拉 `fuxi-memory` 里的策府 facts；按 subject 分组列；可点单条看 detail。后端可能需要新增 `/api/memory` GET endpoint（策府 oracle 当前 fuxi-cli 直接读，要扩到 fuxi-im handler）
   - `RolesPage`：拉 `roles/*` 目录列出所有门客 role 卡（玄女 / 鲁班 / 蒲松 / 仓颉 / extractor / luban-ephemeral / ...）。每个卡显 system prompt 摘要 + role-specific tools。后端需要 `/api/roles` GET endpoint 读 `roles/<name>/ROLE.md`
   - `CronPage`：拉 `fuxi cron list` 等 trigger。后端需要 `/api/cron` GET endpoint。trigger 类型：cron / once / fs / webhook
4. **测试**：MainShell.test.tsx 改 4 tab 期望；新 page 各加渲染测

注意：tab 索引会变（tab 5 通知 → tab 2 通知）；既有 `setActiveTab(1)` 等调用要 grep 改一遍。**这是 breaking change**——但 PWA 内部一致就行，部署不影响其他东西。

---

## 6 · 已知差距（剩余 P2/P3）

继承自 v1-session15.md §2，本 session 没改：

- **P2 home inflight=4 stale leftover**：worker crash 不 report 残留的 inflight，sweep_stale 在 home 一直在线时不触发。要么扩 orphan_sweep（启动期）/ 加 admin endpoint「reset stale inflight」/ 加 ttl 到 inflight entry。**不阻塞功能**
- **P2 sia/ephemeral/task-86106710 物理 leftover**：一刀清 `ssh home 'mv ~/.fuxi/projects/sia/ephemeral/task-86106710-* ~/.fuxi/projects/sia/archive/'`
- **P3 cancel_in_silent_period_via_heartbeat_ack 测试 flaky**：stash baseline 下也挂，timing 设计问题
- **P3 NodeLoadProvider 只看 inflight/concurrency**：v3 综合 CPU 信号

---

## 7 · 协作笔记 / 部署速记

- **TDD 全程**：notifications 后端 10 测先于 handler 写完才接 PWA
- **真测**：home 部署后 ssh `fuxi bug report` + sqlite3 直查 row 落地才算 ship
- **systemd 重启时偶发 EADDRINUSE**：`sudo pkill -9 -f "fuxi im start" && sudo systemctl restart fuxi-im` 兜底；下次接 task #8 直接 systemctl restart 通常 systemd auto-restart 几秒内会自愈
- **codesign 坑（v1-session15.md §4）仍要注意**：mac 端 `cargo build --release` 出来的 binary cp 后必跑 `codesign --force --sign -`
- **ssh home ProxyCommand 抖动（v1-session15.md §4）**：`ssh home 'cat ...'` 偶发空返；本 session 没踩但下次仍可能撞，cache 关键 secret 到 `/tmp/dist_*.key` 兜底
- **agent team 没用**：本 session 串行做，每修紧密依赖前一步；team 模式适合"多个独立模块并行"——不适合本 session 的「修 bug → 加 tab → 加 page → 接玄女 prompt」线性链
- 用户偏好已落 memory：feedback_full_bypass / no_ceremonies / keep_going / tdd_required / no_emoji_tui / pwa_modern_not_tui

### 部署状态快照（接班可直接验）

```bash
# home fuxi-im 进程
ssh home 'pgrep -lf "fuxi im start"; md5sum ~/.local/bin/fuxi'
# 期望：md5 = b04332de250db96b39f884275cc30cd3, PID > 2676402（systemd 可能重启过）

# 确认通知 tab 后端工作
ssh home '/home/e0-7/.local/bin/fuxi bug report --title "session 17 接班 smoke" --body "" --severity wish'
ssh home 'sqlite3 ~/.fuxi/im.db "SELECT id,kind,title FROM notifications ORDER BY created_at DESC LIMIT 1;"'

# PWA dist
ssh home 'ls -la ~/.local/share/fuxi/im-web/index.html'
# 期望 mtime ≈ 2026-05-07 凌晨 build 时刻
```

PWA 强刷：DevTools → Application → Service Workers → Unregister → 刷新；或卸载重装 PWA。

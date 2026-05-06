# Handoff · v1 · Session 15 开工指引

> 上 session（2026-05-06 晚）核心是 **修 P1 task push back**：worker 跑完 task
> 后自动 `git push origin <branch>` 回 home，跨节点真协作 home 端**首次能看到 mac
> 改的代码**。helper + 单测 + wire-in + 跨节点真测全 ship。
>
> 上一份 handoff：`docs/handoff/v1-session14.md`（保留，§1 ship 清单 + §2 P2/P3
> 列表仍有效）。

---

## 1 · 上 session ship 了什么

**HEAD 待 commit**（全绿：fmt + clippy + dist 单测，flaky `cancel_in_silent_period_via_heartbeat_ack` 与本改动无关）。

| 改动 | 行数 | 说明 |
|---|---|---|
| `try_push_back_branch` helper | +85 | `crates/fuxi-cli/src/dist.rs` 加 helper（紧挨 `try_fetch_default_branch`）。`git rev-parse --abbrev-ref HEAD` 拿当前 branch → `git push origin <branch>`。失败 best-effort log warn。`FUXI_DISABLE_PUSHBACK=1` 关闭开关 |
| 4 个单测 | +169 | `push_back_branch_advances_origin_ref`（双 repo 真测 push 推回）+ `_returns_false_when_no_remote` + `_skips_detached_head` + `_respects_disable_env` |
| Wire-in `run_cc_job` + `run_codex_job` | +18 | `Ok((ok, output))` 之前调一次 `try_push_back_branch(cwd)`。无 project_cwd（裸派）跳过 |
| fmt 顺手收 | -10/+10 | `crates/fuxi-agent-codex/src/agent.rs` + `crates/fuxi-im/src/orphan_sweep.rs` —— 上 session（codex 帮做的两 commit）没跑 fmt 留下的尾巴 |

**真测验证（home → mac → home）**：
1. mac 端 binary 重 build + codesign（Gatekeeper 拦未签名 binary，详见 §4）
2. home 用 `python3 dist_enqueue.py` 走 `/dist/enqueue` 派 cc job 给 mac，project=demo-site，pinned-node=mac
3. mac worker pull → spawn cc 进 L3 sandbox `~/.fuxi/projects/demo-site/sandboxes/luban/`
4. cc 创建 `PUSHBACK_SMOKE2.md` + git commit
5. worker log：`post-job push back origin/luban/demo-site-main 完成 path=/Users/e0_7/.fuxi/projects/demo-site/sandboxes/luban`
6. home 端 `cd ~/demo-site && git log luban/demo-site-main` → 能看到 mac 的 commit `1b036fc smoke: pushback verify` ✓

---

## 2 · 当前剩余 bug 清单（按优先级，从 v1-session14 §2 继承）

P1 task push back 已修，下面是上 session 剩下的：

### P2 · cc agent idle latency（一次性角色 600s 才 dead）

cc adapter 是 long-lived 设计，cangjie / 单 task luban 这种"一锤子"角色跑完后等
IdleGcTask ttl_secs=600 触发 AgentDead。PWA 看到僵尸 idle 门客 ~10 分钟。

修法选项（要决策）：
- A. role-specific TTL：cangjie/extractor 等一次性角色 ttl=60s
- B. cc adapter 加"task done 后 N 秒无 follow-up 自杀"逻辑
- C. 不修（PWA 加"已完成但等清理"icon 提示）

**不阻塞 v2 演示**，留给用户决策。

### P2 · sia/ephemeral/task-86106710 物理 leftover

数据脏不影响功能。一刀清：
```bash
ssh home 'mv ~/.fuxi/projects/sia/ephemeral/task-86106710-* ~/.fuxi/projects/sia/archive/'
```

### P3 · PWA composer 没 `@<slug>` mention parser

后端 `/api/dispatch` body.project 已支持（commit `ef6480e`）。前端 UX 缺。
功能补完，不是 bug。

### P3 · NodeLoadProvider 只看 inflight/concurrency

设计取舍，当前规模够。v3 综合 CPU 信号。

### P3 · `cancel_in_silent_period_via_heartbeat_ack` 测试 flaky

stash baseline 下也挂（与代码无关）。pickup 5s 超时在并行 cargo test 高 load
下偶发。需要从 timing 改设计——不阻塞。

### P3 · push back 仅 push 当前 branch（不 mirror tags / 其他 branch）

设计上 worker 只动 task 用的 sandbox worktree，commit 都落在 `luban/demo-site-main`
等 long-lived branch 上。如果 cc 中途 `git checkout -b some-other`，push back 只
推 HEAD 那条。当前规模可接受；后续如果要"一次 push 所有 branch"得改 helper 调
`git push origin --all`，但风险变大（误推用户没 ready 的 branch）。

---

## 3 · 已审查、确认非 bug

（与 v1-session14 §3 同——这次没新增）

---

## 4 · 上 session 踩坑速记（下次注意）

### macOS Gatekeeper 拦未签名 binary

`cargo build --release` 出来的 binary 没 codesign，cp 到 `~/.local/bin/fuxi`
后**不能直接执行**——`spctl -a -v` 返 "rejected"，binary 跑起来什么都不打印就退。

迹象：`fuxi --version` 静默退出（exit 0 但 stdout/stderr 全空），launchd 反复
重启 worker 但每次秒退。

修：
```bash
codesign --force --sign - /Users/e0_7/.local/bin/fuxi
```
（ad-hoc 自签足够本机 launchd 接受）

下次 cp binary 到 `~/.local/bin/` 一定要紧跟 `codesign`。可以加进 deploy script。

### launchd 替代 nohup 管 mac dist worker

mac 端 dist worker 由 launchd `com.fuxi.worker.plist` 守护：
- 配置：`~/Library/LaunchAgents/com.fuxi.worker.plist`
- log：`/tmp/fuxi-worker.log` (stdout) + `/tmp/fuxi-worker.err.log` (stderr)
- 含 `FUXI_DIST_TOKEN` + `FUXI_DIST_HMAC_SECRET` + `RUST_LOG=info,fuxi=info,fuxi_cli::dist=debug`
- KeepAlive=SuccessfulExit:false → 异常退出会重启

要重启 worker：
```bash
launchctl kickstart -k gui/$(id -u)/com.fuxi.worker
# 或：
pkill -9 -f "fuxi dist worker"  # launchd 自动重起
```

**不要再手动 nohup**——会跟 launchd-managed worker 撞 node_id 重复 register。

### ssh home ProxyCommand 偶发失败

`~/.ssh/config` 里 `Host home` 用 ProxyCommand 走 cloudflare DoH 解析 home.qmledmq.cn → nc 过去。**DoH 偶发返非 JSON**（被 Clash 拦或网络抖），症状：
```
json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)
nc: missing hostname and port
```

下次脚本里 `ssh home 'cat ...'` 失败要重试或 cache。本 session 把 dist_token /
dist_hmac.key cache 到 `/tmp/dist_{token,hmac.key}` 后稳。

### 直接走 /dist/enqueue 比 /api/dispatch 简单

PWA 路径 `/api/dispatch` 走 `Fuxi::dispatch` → `auto_pin_from_project`，但要
IM cookie 鉴权。HTTP 测试时直接 sign HMAC 走 `/dist/enqueue`，body 形如：

```json
{
  "node_id": "home",
  "title": "...",
  "body": "...",
  "pinned_node": "zyldemacbook-pro-local",
  "cli": "claude-code",
  "role": "luban",
  "project": "demo-site"
}
```

签名：HMAC-SHA256，canonical = `POST\n/dist/enqueue\n<ts_ms>\n<nonce>\n<body_bytes>`，
key = `~/.fuxi/dist_hmac.key` 文件内容（**字符串本身当 key bytes**，不是 fromhex）。

可复用脚本：`/tmp/dist_enqueue.py` on home（如果还在）。

---

## 5 · 下 session 推荐起点

P1 已清。剩下 P2/P3 列表见 §2，按用户优先决策：

- **如果用户答辩前要清 cc idle latency**（P2.1）→ 选 A 方案最稳，role-allowlist 加
  `cangjie` / `extractor` / 单 task luban 给 60s TTL
- **如果用户要 PWA composer @mention**（P3.1）→ 前端纯 UX 改，约 50-100 行 React
  + 一个 mention parser regex
- **不确定优先级**：跑一轮真用例（用户开 PWA 派活观察），看哪条最显眼

不重要的可顺手清：
- sia/ephemeral leftover 一行 mv 命令的事
- mac binary deploy script 加 codesign 步骤（避免下次踩坑）

---

## 6 · 协作笔记

- **TDD 全程**：先 4 个单测验证 helper 行为（双 repo push、no remote、detached
  HEAD、env 开关），全绿后才 wire 到 worker
- **真测**：跨节点闭环（home enqueue → mac sandbox 跑 cc + commit → mac worker
  push → home demo-site 看到 branch）跑通才算 ship
- 用户偏好已落 memory（feedback_full_bypass / no_ceremonies / keep_going / tdd_required）
- agent team 模式没用——本 session 串行做，每修紧密依赖前一步

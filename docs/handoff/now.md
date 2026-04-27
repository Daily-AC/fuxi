# NOW · 单页真相（2026-04-27 · IM v4 dist 接通卡在 mac cc spawn）

> 上一份 NOW 是同日 v3 重设计四件套全 ship。本份覆盖 v4 dist 真接通批次（#54-#70）+ 用户实测撞 mac cc spawn 失败的当下卡点。

## 一句话

fuxi-im v4 dist 真接通**代码层全 ship + home 部署完成 + mac worker 注册成功**，但 e2e 第 3 条（玄女派活到 mac-local 真跑 cc 出结果）**卡在 mac worker spawn cc 失败 → ok=0**。已找到首层 root cause（plist PATH 缺 `~/.local/bin`），刚 hot-fix 但**未在新会话验证**。

## 5 gap 接通进度

| gap | 代码 | 部署 | e2e 验通 |
|---|---|---|---|
| (a) fuxi-im 内嵌 dist controller (#54) | ✅ `f114978` | ✅ home | ✅（home 节点 self-register OK）|
| (b) install-local-worker.sh + macOS launchd (#61) | ✅ `55a91bc` | ✅ home 静态端点 | ⚠ mac worker register 通了但 cc spawn 失败 |
| (c) /api/nodes 真 topology (#55) | ✅ `31b5649` | ✅ home | ✅（PWA 节点 tab 显 home + mac-local online）|
| (d) PWA 节点 tab 切真 + 任务树 @node (#58/#59/#64) | ✅ ε ship | ✅ home | ✅（PWA 显示对）|
| (e) 玄女 dispatch routing 决策树 (#57) + intervene 闭环 (#70) | ✅ `f77b159` + `5b5fe70` | ✅ home | ⚠ 玄女路由对了，dist enqueue 进 queue，worker pull 拿到 job，**cc spawn 失败 ok=0** |

## 当前真卡点：mac worker spawn cc

**evidence chain**：
- home `dist_jobs.db` 显示 3 个 job：state=`done`、assignee=`zyldemacbook-pro-local`、**ok=0**（fast-fail，job dispatch 后 1 秒就 done）
- home journal `dist enqueue 成功 pinned_node=Some("zyldemacbook-pro-local")` ✓
- nginx access log 3040 次 `/dist/pull 200`（worker 真 pull）+ 3 次 `/dist/report 200`（真上报）
- mac `which -a claude` → `/Users/e0_7/.local/bin/claude`
- mac plist `EnvironmentVariables.PATH` 原本 = `~/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin`（**缺 `~/.local/bin`**）

**首层 root cause**：worker spawn cc 时 `--cc-bin` 默认 `claude`，走 PATH 查找，但 `~/.local/bin` 不在 plist PATH 里 → cc 找不到 → spawn 失败 → report `ok=0`。

**修法（双保险）**：
- 我 mac hot-fix：plist PATH 前置 `~/.local/bin`，重启 worker（pid 26292 跑着）
- ζ 升级版 (`88329c8`) 部署到 home：脚本探 cc 真绝对路径 → plist 加 `--cc-bin /Users/e0_7/.local/bin/claude` 不依赖 PATH + plist PATH 也加 `~/.local/bin`（cc 内部 spawn 子进程仍能命中）

**未验证**：用户在我 hot-fix 后重发同一句但**没观察到 cc 真跑**，PWA 节点 tab 工作中门客仍 0。可能原因：
1. PATH 修不够，仍有其他 cc spawn 问题（NO_PROXY 没生效 / CLAUDECODE env 嵌套触发 cc 静默卡死 / cc 自己鉴权问题）
2. PWA 任务树 / 节点 tab 没刷新（缓存）
3. 玄女这次没真触发 dispatch（用户测试时 PWA → /api/intervene 路径出错）
4. 或者 hot-fix 的 worker 跑起来但其他原因 cc spawn 又失败

## 新会话接班步骤（5 分钟）

1. 读本文件 + spec `docs/superpowers/specs/2026-04-27-im-dist-接通-design.md`
2. 实时 tail mac worker log + ssh home journalctl 看链路状态
   ```bash
   tail -f /tmp/fuxi-worker.log /tmp/fuxi-worker.err.log
   ssh home 'journalctl -u fuxi-im -f -p info' &
   ```
3. 让用户重发：`@zyldemacbook-pro-local 帮我 ls ~/erp`
4. 看链路哪一步停：
   - home journal 没 `intervene on idle → auto-degrade` → PWA 没到 home（cookie/WS 问题）
   - 没 `dispatch routing: 走 dist enqueue` → 玄女没认 [路由提示]（#70 实装可能未生效到 home）
   - dist_jobs 表 ok=1 + worker stdout 有 cc 输出 → e2e 通，PWA 显示问题（前端事件订阅）
   - dist_jobs 表 ok=0 → cc spawn 仍失败，看 worker err log 真错误信息
5. 如 hot-fix PATH 仍不够 → 让用户跑 `bash <(curl -s https://im.qmledmq.cn:8443/setup-local-worker.sh)` 拿 ζ 升级版（双保险 `--cc-bin` 绝对路径）

## v4 ship 矩阵（71 commit 已 home）

最新 HEAD：`88329c8`（ζ install-local-worker.sh PATH + --cc-bin 双保险，未用户验）

主要 commit 链（按时间逆）：
```
88329c8 ζ install-local-worker.sh cc PATH + --cc-bin 双保险 (#71 RCA)
5b5fe70 β #70 intervene pinned_node 真路由闭环 (#57 v1 缺口)
2343995 β #67 setup-worker controller_url X-Forwarded-* 推算
8efbf4b β #69 worker controller URL normalize 6 处 + chaos timing
2ddc43d ε #68 user bubble 竖排修 + toast 误报修 + ζ #66 controller URL 部分修搭车
55a91bc ζ install-local-worker.sh 加固（preflight 5 项 + 回滚 + NO_PROXY）
6ea7c96 ζ install.sh fuxi-im.service 加 dist controller URL env (有问题，被 2ddc43d 修对)
f361706 β #56 setup-worker + 静态脚本端点
31b5649 β #55 /api/nodes 真 topology + members.node_id
f114978 β #54 fuxi-im 内嵌 dist controller + /dist/* HMAC layer
f77b159 β #57 dispatch routing 决策树 + EventKind.pinned_node + 玄女 ROLE.md
e58f188 β #42 intervene mentions
e8cf9bf β #41 /api/tasks/:id/{events,conv} 镜像端点
fda219b ε #31 玄女 sticky badge "✓ 抄送 N 门客"（v3 已 superseded by 任务 tab）
1a4d6d5 ε #39 任务 thread mix 全成员
ce9a12c ε #40 玄女 tab 删 badge + 加 @ chip composer
d984d7d ε #38 任务列表点卡 push thread
dc0a404 ε #37 MentionChip + MentionAutocomplete
ad220d6 ε #36 BottomTabBar + 删 Pager
38f2ef4 docs · v4 spec + 决策 16/17
（之前还有 v3 重设计 / v2 路线 / B1 deliverable handoff #48 等，详见 git log -50）
```

## v4 spec + 决策

- **spec**：`docs/superpowers/specs/2026-04-27-im-dist-接通-design.md`
- **v3 spec**（已 ship 但 v4 是延续）：`2026-04-26-im-tab-bar-task-thread-design.md`
- **决策 16**：`docs/decisions/16-im-tab-bar-task-thread.md`（任务=群聊心智）
- **决策 17**：`docs/decisions/17-im-deploy-decoupling.md`（IM 部署解耦中期排期）
- **决策 13**：`docs/decisions/13-deliverable-boundary-handoff.md`（deliverable nudge）
- **决策 12**：`docs/decisions/12-dist-worker-true-concurrency.md`（dist 真并发，本批是用户能用层落地）

## agent team `fuxi-im-v1` 状态

- **β** 队列空（#54-#57 + #41/#42 + #67/#69/#70 + #65 follow-up 全 done）
- **ε** 队列空（#36-#40 + #58-#60 + #64 + #68 全 done，#43/#35 follow-up pending）
- **ζ** #62 e2e 验收 in_progress（卡 mac worker cc spawn）+ #71 in_progress（RCA 已修部署，未用户验通）
- 全员 alive，新会话 SendMessage 续派即可

## 已知缺口（非阻塞）

- #43 follow-up · UserMessage.mentions 历史回放还原 chip 视觉
- #35 follow-up · ToolCallCard stdout 前 20 行截断 + 全文按钮
- v1 simplification: intervene busy 路径不 prepend pinned_node（PendingTurn struct 没字段）
- v1 simplification: `dispatch --task <parent>` 父任务 fan-out 时 routing hint 不传

## 接班"5 分钟"checklist

1. 读 `CLAUDE.md`（公理 + 工程规范）
2. 读本文件
3. 读 v4 spec + 决策 16/17
4. `git log --oneline -20` 看 commit 链
5. `TaskList` 看 #62 / #71 进度 + #35/#43 pending
6. `git status` 工作树状态
7. SendMessage 各 teammate 探活（β/ε/ζ idle 待命）

## 不该忘记

- ssh home 端口 2222 / 用户 e0-7 / DDNS / sudo 可用
- nginx 8443 + 通配符证书 + WS upgrade headers
- fuxi-im 绑 127.0.0.1:9100 + nginx im.qmledmq.cn:8443 反代
- `~/.fuxi/im_password.bcrypt` 主密码
- 玄女自启在 `xuannv_bootstrap::ensure_xuannv`
- mac worker plist `~/Library/LaunchAgents/com.fuxi.worker.plist` + env `~/.fuxi/dist-worker.env`
- mac worker 日志 `/tmp/fuxi-worker.log` + `/tmp/fuxi-worker.err.log`
- home dist_jobs 表 `~/.fuxi/dist_jobs.db`（ssh home 上 sqlite3 看）
- claude binary 真路径 `/Users/e0_7/.local/bin/claude`（mac）
- install.sh ssh ControlMaster 加固 (`2202f87`)
- install.sh rsync `-c` checksum (`a2c5976`) 防 mtime collision stale

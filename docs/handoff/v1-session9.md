# Handoff · v1 · Session 9 开工指引

> 上一 session（2026-05-03）是个**双战场**：
> 1. 把 Decision 21/22 phase 3 的 4 块「下一动作」彻底干完（B 跨节点 sandbox /
>    C tag-aware spawn / D strict mode / A 跨 tab 跳转）
> 2. 用户用 PWA 真测后发现 6 个新问题——P0/P1 修了 4 个并部署到 home 实测验通，
>    剩 2 个 UX bug + 3 个新 fix 留下次。
> 上份 handoff: `docs/handoff/v1-session8.md`（保留）。

---

## 1 · 10 分钟必读

1. `CLAUDE.md` · 七公理 + agent team / decision-split 协作（3 min）
2. **`docs/decisions/21-workspace-design.md`** + **`22-deliverables-storage.md`**（3 min）
3. `roles/xuannv/instructions/dispatch-routing.md` · 完整含 phase 3 跨节点 +
   L2/L3 教学（5 min）
4. 本文 §3 "下一动作"

---

## 2 · 上一 session ship 了什么

**phase 3 初始批**（已记 session8）：B/C/D + A polish + navTo 跨 tab。

**用户实测后修的 P0/P1**（session 9 后半段）：

| commit | 内容 |
|--------|------|
| `dca1bd7` | `fuxi xuannv refresh` 命令——清 oracle session record，让玄女 fresh spawn 重读 dispatch-routing.md。根因：cc `--resume` 老 session 不重读 `--append-system-prompt`，新教学永远学不到。 |
| `e3b13bb` | accept_to 路径 sticky——按 project 维度 localStorage 记忆上次接收目录，免每次手填。 |
| `7be5af8` | MentionComposer 多行输入——`<input>` 改 `<textarea>` + auto-grow + Shift+Enter 换行。 |
| `0b8e0bb` | `fuxi project add` 自动探测 default_branch——以前硬塞 "main"，sia 是 master 就 spawn 失败。改用 `git symbolic-ref --short HEAD` 探测当前分支，detached HEAD 才 fallback "main"。 |

**已部署到 home + 端到端验证全通**（spawn → L3 sandbox 建在 master 分支 → produce
→ manifest 落盘 + 事件入库）。用户在 PWA 实测：交付 + L3 sandbox 成功。

---

## 3 · 下一动作（按优先级 + 复杂度排）

### P0 阻塞类（剩 2 条 + 上一批新发现 3 条）

#### P0.A · 玄女不主动派活给门客（**最高优先**）

**症状**：用户说"长期维护 sia 项目"，玄女**自己**用 Agent(Explore) 写了报告，
没起 luban、没走 dispatch。等用户问"你做了 sia spawn 吗"她才补上。

**根因**：`roles/xuannv/instructions/axioms.md` + `dispatch-routing.md` 教了
**怎么**派活，但没强制说"凡能派的活默认派"。玄女有「自己做更快」的诱惑（cc 有
所有工具，包括 Bash + Edit）→ 自己上手。

**fix 方向**：
- 收紧玄女工具白名单：去掉 Edit / Write / Read（让她**只能** Bash 调 fuxi 子命令 +
  Read 看门客落盘）
- 或在 system prompt 加强势规则："任何超过 1 个 tool call 的活默认派 luban；
  自己只做对话 + 决策 + 简短判断"
- 参考小希（用户的个人 Sia agent）的 `instructions/dispatch-protocol.md`：
  「能派给工人的不要自己做」 写得比较硬

**估时**：S（改 prompt + 重启玄女 fresh session 即可）

#### P0.B · 任务列表全叫 "ad-hoc"（图 2）

**症状**：PWA 任务 tab → 「已完成」全是 `ad-hoc` 标题，区分不出哪个干啥。

**根因**：dispatch 时 task.title 留空 → 后端用 fallback 字符串。需要追踪：
- `Fuxi::dispatch` 入口 task.title 的来源
- 玄女的 `fuxi dispatch --to <id> "..."` CLI 是否传了 title
- 还是只传 description，title 后端硬塞 "ad-hoc"？

**估时**：S（要么 CLI 加 `--title` 显参，要么从 description 第一行/前 N 字派生
title）

#### P0.C · 任务 thread 工具卡随增加越来越挤（图 3）

**症状**：鲁班跑 N 个 Bash/grep/Read 后，任务 thread layer 2 的工具卡互相
overlap，看起来全部叠在一起。

**根因**：CSS `.toolCallCard` 可能有 `position: absolute` 或 `margin-top:
负值` / `flex-direction: column` 间距没设。

**估时**：S（CSS gap / margin 修）

#### P0.D · 任务 thread 门客完成后输出消失（session8 留下）

**症状**：task done 后 thread layer 2 只剩 banner + user 消息 error，鲁班
原本的产出 reply 全没了。

**疑似真因**：`/api/tasks/{id}/events` 后端是否正确返了所有 task_id 关联事件，
或前端 reducer 处理 task_completed 时清了消息。需要 trace。

**估时**：M（要 backend trace + frontend reducer audit）

#### P0.E · 私聊门客误报"玄女正忙"（session8 留下）

**症状**：用户在 task done 后的 thread 输入"你还活着么"→ toast「玄女正忙」。

**根因清楚**：task done → 前端 task_id=null → 当玄女主对话 intervene → 玄女
busy 时 4xx → 误导 toast。

**fix 方向**：
- task done 时 composer placeholder 改 "@玄女讨论这个 task..." 让用户知道在跟谁说
- 或允许"复活"task 路由给最后那个 worker

**估时**：S（placeholder + 错误文案改）

### P1 体验类

#### P1.6 · 气泡宽度统一（session8 留下）

不同 page 气泡每行字数不一致——CSS `max-width` 各自定。共享 CSS variable。

#### P1.G · 玄女 model：sonnet → opus 4.6 / 4.7

玄女是关键决策角色，sonnet 经常误判。考虑升 opus（成本 vs 质量取舍）。

### P2 功能 / 运维（上次留的）

- **P2.7 · 轻量文件推送**：agent → 用户气泡内嵌 md 预览（不走 deliverable
  收件箱，直接 push 到对话流）
- **P2.8 · GitHub Actions**：build fuxi-cli for {linux-x86, linux-arm,
  macos-arm} 推到 GitHub Releases；写 `setup-local-worker.sh` 一键 curl 装
- **P2.9 · install.sh preflight 自动接管**：detect fuxi-im 在跑 → 自动
  `sudo systemctl stop` 而非拒；detect 旧 vhost 文件 → 自动 backup + 替换

---

## 4 · 部署快照（home 现状 21:50）

```
binary:    /home/e0-7/.local/bin/fuxi  (5月3日 21:43 编译，含 phase 3 + xuannv refresh + default_branch 探测)
           /home/e0-7/.cargo/bin/fuxi  (同上)
PWA:       /home/e0-7/.local/share/fuxi/im-web  (含 navTo + multi-line composer + accept_to sticky)
project:   sia → /home/e0-7/sia, default_branch=master  ✅ 探测正确
玄女:      fresh session（21:43 之后），无失败记忆
sandbox:   /home/e0-7/.fuxi/projects/sia/sandboxes/luban → luban/sia-main 分支 (用户实测建的，留着)
```

---

## 5 · 实测路径（验证 phase 3 全通）

```bash
ssh home && cd ~/fuxi && git pull
export PATH=$HOME/.cargo/bin:$PATH

# project 注册（自动探测分支）
fuxi project add ~/your-repo
fuxi project list  # 见 default_branch

# spawn L3
fuxi spawn --role luban --project sia
ls ~/.fuxi/projects/sia/sandboxes/luban/  # worktree 实在
cd ~/sia && git worktree list  # 见 luban/sia-main 分支

# spawn L2
TASK_ID=$(uuidgen)
fuxi spawn --role luban --project sia --ephemeral --task task-$TASK_ID
ls ~/.fuxi/projects/sia/ephemeral/task-$TASK_ID/

# deliverable
echo "test" > /tmp/r.md
fuxi deliverable produce --project sia --task task-$TASK_ID --kind research_summary /tmp/r.md
ls ~/.fuxi/projects/sia/deliverables/task-$TASK_ID/
```

PWA → 项目 / 交付 tab 同步可见。

---

## 6 · 踩坑预防（新加）

- **改玄女教学后必须 `fuxi xuannv refresh` + `sudo systemctl restart fuxi-im`**：
  cc `--resume` 不重读 `--append-system-prompt`。否则教学永远不生效。
- **玄女 cc 进程跑了一段时间后会形成"经验偏见"**：之前 spawn 失败一次（即使
  bug 已修），她可能基于记忆说"有 bug"而不重试。让她 refresh 一次清记忆。
- **home 上的 ~/fuxi/ 不是 git 仓库**：是 rsync 部署目标。要更新代码用
  `rsync -avz --delete --exclude=target --exclude=node_modules --exclude=.git
   /Users/e0_7/fuxi/ home:/home/e0-7/fuxi/`，然后 ssh build。
- **fuxi-im 的 binary 路径是 `/home/e0-7/.local/bin/fuxi`**（不是 `~/.cargo/bin/`）。
  更新 binary 时两处都要替换（用户 shell PATH 里 `~/.cargo/bin` 在前，systemd 用
  `~/.local/bin/fuxi`）。
- **fuxi-im 是系统级 systemd service**（`/etc/systemd/system/fuxi-im.service`）
  → 用 `sudo systemctl restart fuxi-im`，**不是** `--user`。

---

## 7 · 决策快照（这次产生的）

无新公理 / 反公理。

设计取舍：
1. **`fuxi xuannv refresh` 选"清记录 + 提示用户手动 restart"，不试图自动 kill
   玄女**：`shutdown_agent` 有玄女豁免（CLAUDE.md 七公理之一），强行 kill 要
   绕公理。手动 restart 简单可靠。
2. **default_branch 探测选 `git symbolic-ref --short HEAD` 不选 `git config
   init.defaultBranch`**：后者是新建 repo 默认值，跟当前 repo 实际分支可能不一致
   （用户全局 init.defaultBranch=main 但 sia 几年前 init 的是 master）。

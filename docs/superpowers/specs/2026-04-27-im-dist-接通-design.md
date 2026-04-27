# IM ↔ dist 真接通 · home 玄女派活到本地 macOS worker

**日期**：2026-04-27
**状态**：用户拍板（e3 routing + 工程层自决 + 节点+任务树都标节点来源），进入实装
**触发**：用户实测撞穿"2026-04-25 B 路 6/6 + B1 全交付（代码层）但 IM 部署/接通从未做（用户能用层）"的 gap

## 一句话

把 fuxi 已有的 dist 子系统（controller / worker / HMAC / chaos test 都在 git）真接进 IM 链路：home 上 fuxi-im 内嵌 dist controller；本地 macOS 起 dist worker；玄女 dispatch 按 task tags / pinned_node 路由到 dist worker；PWA 节点 tab + 任务树都标节点来源。

## 5 gap 全堵的设计（用户已拍 3 个关键点）

### gap (a) home 启 dist controller — **fuxi-im 内嵌**（同进程）

`fuxi-im start` 启动时除了起 IM axum + 自启玄女，**还内嵌起 `dist::DistController`**（端口共用 9100，`/dist/*` 路由 走 HMAC layer，`/api/*` 走 cookie layer，两套 auth 互不干扰）。

为啥同进程：
- 共享 EventBus（dist worker 上报 events 直接进 home 玄女视野）
- 共享 SQLite WAL（dist_jobs 持久化）
- 决策 17 远期拆 binary 时可以拆，本设计不动 binary 边界

### gap (b) 本地 macOS worker onboarding — **临时 shell 脚本，长期抽 `fuxi join` CLI**

短期实装：
- `scripts/install-local-worker.sh`：交互式问 home URL + 主密码 → curl `POST /api/dist/setup-worker`（鉴主密码 → 返 HMAC secret + dist token）→ 写 `~/.fuxi/dist-worker.env` → 在 macOS 上 launchctl bootstrap 一个 plist daemon
- 用户跑 `bash <(curl https://im.qmledmq.cn:8443/setup-local-worker.sh)`（PWA 节点 tab "添加节点" 按钮显示这条命令）
- `~/.fuxi/dist-worker.env` 含 `FUXI_DIST_HMAC_SECRET=...` + `FUXI_DIST_TOKEN=...`
- launchd plist `~/Library/LaunchAgents/com.fuxi.worker.plist` 自启 + 重启

长期（v1.x，单开任务排期）：
- `fuxi join <home-url>` CLI 抽象：单条命令完成上面所有步骤
- 不阻塞本设计

### gap (c) IM 后端 `/api/nodes` 真 topology — **直接 wrap dist::DistController.nodes_snapshot()**

新增 `GET /api/nodes`：

```jsonc
{
  "nodes": [
    {
      "node_id": "home",
      "tags": ["home", "linux"],
      "max_concurrency": 4,
      "inflight_jobs": 1,
      "heartbeat_lag_ms": 8230,
      "online": true,
      "registered_at": "...",
      "workers": [
        { "agent_id": "<uuid>", "role": "鲁班", "role_display": "鲁班", "status": "busy", "current_task_id": "<uuid>", "current_task_title": "查 ERP API" }
      ]
    },
    {
      "node_id": "mac-local",
      "tags": ["local", "erp", "mac"],
      "max_concurrency": 2,
      "inflight_jobs": 0,
      "heartbeat_lag_ms": 4120,
      "online": true,
      "workers": []
    }
  ]
}
```

`online` 判断：heartbeat lag < 30s。`workers` 字段：dist controller 维护 node → worker 实例 map（worker 通过 dist register/heartbeat 上报）。

注意：home 节点虽然跟 fuxi-im 同进程，**也要走 dist register 注册自己**（"home" node 注册时声明 `tags=["home", "linux"]`），保持 dist topology 视图统一。

### gap (d) PWA 节点 tab 真接 + 任务树标节点来源 — **删 aggregateHomeNode 假数据**

**节点 tab 改造**：
- 删 `aggregateHomeNode`（把 tasksOverview 包一层假装 topology）
- 新建 `client.fetchNodes()` → GET /api/nodes
- 渲染：每节点一卡，header 含节点名 + 在线状态 dot + tags 列表 + inflight/max
- 卡内列 worker 实例（按 status 排序 busy > idle）：role + 当前 task title (可点跳任务 thread)
- 节点离线：整卡 muted + dot 灰 + 不展开 workers
- "添加节点"按钮：弹 modal 显示 install-local-worker.sh 一行复制命令 + 主密码提示

**任务树（任务列表 + 任务 thread banner）member 行加 `@node` 标识**：

任务列表 member 副文本格式（v3 #29 的 C 方案）：
```
鲁班 · grep server/api/v1.go · @mac-local
蒲松 · 待命 · @home
```

任务 thread banner 第二行：
```
鲁班 · grep · @mac-local ▎蒲松 · 待命 · @home
```

视觉：`@node` 部分用 muted 颜色 + monospace 小字。

### gap (e) 玄女 dispatch routing — **e3：玄女判断 + 用户 override**

**Fuxi::dispatch 决策树**：

```
def dispatch(target_role, task):
    if task.pinned_node is not None:
        # 用户显式 @mac-local 这种
        return dist_enqueue(task, pinned_node=task.pinned_node)
    elif task.required_tags is not empty:
        # 玄女按 ROLE.md 规则推断的 tag (e.g. "local" / "erp")
        return dist_enqueue(task, required_tags=task.required_tags)
    else:
        # 默认本地 spawn (home 上 cc/codex 进程)
        return local_spawn(target_role, task)
```

**玄女 ROLE.md 加规则**（注入到 system prompt addendum，复用 #48 的 `sentinel_addendum.rs` pattern）：

```
派活路由规则：
- 涉及本地文件系统操作（~/erp 等用户 macOS 项目）→ task 加 required_tags=["local"]
- 涉及 ERP 项目特定 → 加 ["erp"] (蕴含 local)
- 服务器维护 / nginx / systemd → 加 ["home"]
- 不确定 → 不加 tag (默认 home 本地 spawn)
- 用户在 PWA 显式说"用 mac-local" → 解析为 pinned_node
```

**PWA composer @ pinned-node 解析**：

@autocomplete 候选范围扩展：候选不只 worker agent_id，也包括 dist node_id（home / mac-local / ...）。chip 视觉用节点专属色（如蓝色 `#7AA0E5`）区分 worker chip。

发送时：
- chip 是 worker → 现有 mentions 路径
- chip 是 node → req.body 加 `pinned_node: "mac-local"`

后端 `Fuxi::dispatch` 看到 pinned_node → 走 dist enqueue 到指定节点。

## 后端 API 契约

### 新增（β）

| 端点 | 用途 |
|---|---|
| `GET /api/nodes` | 真 dist topology（含 workers 实例） |
| `POST /api/dist/setup-worker` | onboarding：主密码鉴权 → 返 HMAC secret + dist token + controller URL（一次性，写本地 .env） |
| `GET /setup-local-worker.sh` | 静态返 install-local-worker.sh 内容（带主密码提示），用户 curl 一行运行 |

### 改造（β）

| 字段/路径 | 改动 |
|---|---|
| `Fuxi::dispatch` | 加 routing 决策树（pinned_node / required_tags / default 三分支） |
| `GET /api/tasks` members | 加 `node_id: string` 字段（worker 所在节点） |
| `POST /api/intervene` | request body 加 `pinned_node?: string`（用户 @ 节点显式路由） |
| `EventKind::UserInterventionSent` | 加 `pinned_node: Option<AgentId>` 字段（同步 5 处） |
| 玄女 ROLE.md | 加派活路由规则段（也通过 sentinel_addendum 注入 system prompt addendum） |

### 沿用（β）

- `dist::DistController` (fuxi-cli/src/dist.rs)
- `dist::run_worker` (fuxi-cli/src/dist.rs)
- HMAC layer (fuxi-cli/src/dist_auth.rs)
- 决策 12 path 1-6 全部代码

## install-local-worker.sh 设计（ζ）

```bash
#!/usr/bin/env bash
# 用法: bash <(curl -s https://im.qmledmq.cn:8443/setup-local-worker.sh)

set -euo pipefail

HOME_URL="https://im.qmledmq.cn:8443"
NODE_NAME="${1:-$(hostname -s | tr '[:upper:]' '[:lower:]')-local}"

# 1. 问主密码
read -srp "fuxi 主密码: " PASSWORD; echo

# 2. 拉 HMAC secret + token
SETUP_RESP=$(curl -fsS -X POST "$HOME_URL/api/dist/setup-worker" \
  -H "Content-Type: application/json" \
  -d "{\"password\":\"$PASSWORD\",\"node_id\":\"$NODE_NAME\"}")

HMAC_SECRET=$(echo "$SETUP_RESP" | jq -r .hmac_secret)
DIST_TOKEN=$(echo "$SETUP_RESP" | jq -r .dist_token)

# 3. 写 env
mkdir -p ~/.fuxi
cat >~/.fuxi/dist-worker.env <<EOF
FUXI_DIST_HMAC_SECRET=$HMAC_SECRET
FUXI_DIST_TOKEN=$DIST_TOKEN
FUXI_DIST_CONTROLLER=$HOME_URL/dist
FUXI_DIST_NODE_ID=$NODE_NAME
EOF
chmod 600 ~/.fuxi/dist-worker.env

# 4. 装 launchd plist
PLIST=~/Library/LaunchAgents/com.fuxi.worker.plist
FUXI_BIN=$(which fuxi)
cat >"$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC ...>
<plist version="1.0">
<dict>
  <key>Label</key><string>com.fuxi.worker</string>
  <key>ProgramArguments</key>
  <array>
    <string>$FUXI_BIN</string>
    <string>dist</string>
    <string>worker</string>
    <string>--controller</string><string>$HOME_URL/dist</string>
    <string>--node</string><string>$NODE_NAME</string>
    <string>--tag</string><string>local</string>
    <string>--tag</string><string>erp</string>
    <string>--tag</string><string>mac</string>
    <string>--max-concurrency</string><string>2</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>FUXI_DIST_HMAC_SECRET</key><string>$HMAC_SECRET</string>
    <key>FUXI_DIST_TOKEN</key><string>$DIST_TOKEN</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>/tmp/fuxi-worker.log</string>
  <key>StandardErrorPath</key><string>/tmp/fuxi-worker.err.log</string>
</dict>
</plist>
EOF

# 5. 启
launchctl bootout gui/$UID/com.fuxi.worker 2>/dev/null || true
launchctl bootstrap gui/$UID "$PLIST"
echo "✓ 已注册节点 $NODE_NAME，到 PWA 节点 tab 看是否在线"
```

注意 `fuxi` binary 必须本地存在（用户已有 `/Users/e0_7/.cargo/bin/fuxi`，但需要先 `cargo build --release` 升到当前 HEAD）。脚本里加 `cargo install --path /Users/e0_7/fuxi` 自检步骤。

## 拆活

### β（4 条，重，先开）

| ID | subject | dep |
|---|---|---|
| #54 | β · fuxi-im 内嵌 dist controller + /dist/* HMAC layer 路由 | （无） |
| #55 | β · GET /api/nodes 真 topology + members.node_id 字段 | #54 |
| #56 | β · POST /api/dist/setup-worker + GET /setup-local-worker.sh 静态端点 | #54 |
| #57 | β · Fuxi::dispatch routing 决策树 + EventKind.pinned_node + 玄女 ROLE.md 路由规则 | #54 |

### ε（3 条）

| ID | subject | dep |
|---|---|---|
| #58 | ε · 节点 tab 切真 /api/nodes + 列每节点 workers 实例 + "添加节点" modal | #55 |
| #59 | ε · 任务列表 + 任务 thread banner member 行加 @node 标识 | #55 |
| #60 | ε · composer @ autocomplete 候选含 dist node + chip 蓝色区分 + intervene body pinned_node | #57 |

### ζ（2 条）

| ID | subject | dep |
|---|---|---|
| #61 | ζ · scripts/install-local-worker.sh + macOS launchd plist 模板 | #56 |
| #62 | ζ · 本地 cargo install + 跑脚本 + e2e 验证 mac-local 在 PWA 节点 tab 真在线 | 全部 |

## 接通验收（绝不再说"已接通"直到下面 5 条全 pass）

1. PWA 节点 tab 显示 home + mac-local 两节点，**两个都 online**
2. 用户在玄女 tab 输 "@mac-local 帮我 ls ~/erp" → 玄女 dispatch 路由到 mac-local → 本地 launchd worker 起 cc → cc cwd ~/erp 跑 ls → 结果通过 dist event 流回 home → PWA 显示
3. 用户在玄女 tab 输 "看一下 ~/erp/erp-lt-vv 的 git 分支"（不显式 @）→ 玄女按 ROLE.md 规则推断 required_tags=["local"] → 同样路由到 mac-local（不是 home 本地 spawn）
4. PWA 任务列表 + 任务 thread banner 显示 "鲁班 · ... · @mac-local"
5. 关掉本地 launchd worker → 30s 后 PWA 节点 tab 显示 mac-local 离线

## 关联

- 决策 12（dist worker true concurrency）—— 本设计是其用户能用层落地
- 决策 13（deliverable handoff）—— B1 sentinel 在 dist worker 上同样有效（已 wired）
- 决策 17（部署解耦）—— 远期方向，本设计不依赖
- memory `project_fuxi_b_path_vision` —— B 路最初愿景的兑现
- memory `project_fuxi_first_real_workload` —— ~/erp 是 fuxi 首个真实业务验收

## ETA

- 今晚（2026-04-27 22:00 之前）：β #54-#57 + ε #58-#60 + ζ #61 全 ship + #62 e2e 验证开始
- 明天上半天兜底：补 ε/β 接通 bug + 节点离线/上线动画 / 视觉 polish

约束：宁推迟，不再说"已接通"。

# 玄女眼睛（Vision）设计稿 · 2026-05-14

> 给桌宠加一只可被玄女主动调用的眼睛——webcam + screen，召唤式触发，玄女亲眼看（cc 多模态 Read），不走 MCP。

## 一句话

加一个 CLI 工具 `fuxi xuannv look`——玄女在回合里 emit Bash 调它，命令通过 fuxi-im 反向推到桌宠拍一帧上传，回写本地 path 到 stdout，玄女随后用 cc 自带 Read 工具把图读进上下文。

## 边界（用户已确认）

- **眼睛归玄女**：webcam + screen 都要，由她按上下文决定看哪只
- **触发节奏**：召唤式——用户主动说「玄女看看 XX」或唤醒词回合内她可以顺手看一眼。**她不在 idle 期偷看**。
- **屏幕范围**：tool 接受 `webcam | screen | window | region`，由玄女按用户 hint 自决（v1 仅实装 `webcam | screen`，window/region 留 v1.1）

## 架构

```
                     home 部署机                              用户 mac
┌────────────────────────────────────────────┐    ┌──────────────────────────┐
│ 玄女 (cc) ──Bash──▶ fuxi xuannv look       │    │ 桌宠 (Tauri 2)           │
│                          │                  │    │                          │
│                          ▼ HTTP             │    │  ws /api/conv (existing) │
│                  fuxi-im                   │◀───┤  ◀─ vision_request ───   │
│                  /api/xuannv/look          │    │                          │
│                          │                  │    │  capture frame          │
│                          ├─ ws push ───────▶│───▶│  ─── multipart upload ─▶ │
│                          │                  │    │     /api/xuannv/look/    │
│                          │                  │    │        frame             │
│                          ▼                  │    │                          │
│                落盘 ~/.fuxi/vision/*.png    │    │                          │
│                          │                  │    │                          │
│             stdout = abs path ──▶ 玄女      │    │                          │
│             玄女 Read(path) → 多模态        │    │                          │
└────────────────────────────────────────────┘    └──────────────────────────┘
```

## 接口契约

### CLI · `fuxi xuannv look`

```
fuxi xuannv look --target webcam|screen [--hint "..."] [--timeout-secs 10]
```

- 阻塞执行，stdout = 一行绝对 path（`~/.fuxi/vision/<uuid>.<ext>`）
- 失败：非零退出 + stderr 友好中文（"桌宠未连接" / "用户拒绝授权" / "采帧超时" / "无活动桌宠"）
- 内部：HTTP POST `http://127.0.0.1:9100/api/xuannv/look`（fuxi-im 默认 loopback 端口，见 `lib.rs` 注释），loopback 复用 cookie auth 中间件——CLI 跑在 home 上和 fuxi-im 同机，可读 systemd-managed cookie 文件；若读不到 cookie 则跳鉴权（同机 loopback 信任）

### HTTP · `POST /api/xuannv/look`

**请求**
```json
{ "target": "webcam" | "screen",
  "hint": "可选 自由文本，纯给玄女自己看的备忘",
  "timeout_secs": 10 }
```

**响应（200）**
```json
{ "ok": true,
  "request_id": "uuid",
  "path": "/home/e0-7/.local/share/fuxi/vision/<uuid>.png",
  "mime": "image/png",
  "bytes": 234567 }
```

**响应（4xx/5xx）**: `{ "ok": false, "error": "no_pet_connected" | "timeout" | "upload_failed" | "permission_denied" }`

**实现要点**
- 生成 `request_id`，新建 `tokio::sync::oneshot`
- 通过 `app_state.conv_broadcast`（已有 WS broadcaster）推 `WireEvent` `VisionRequest { request_id, target, hint }` 给所有 pet ws 订阅者
- 没有任何 pet 在线 → 立即 400 `no_pet_connected`
- 等 oneshot：成功 → 200 ok，超时（默认 10s）→ 408

### HTTP · `POST /api/xuannv/look/frame`

multipart：
- `request_id`: 文本字段
- `file`: 二进制 PNG/JPEG（pet 端拍后上传）
- `mime`: 文本字段（image/png 默认）

成功 → 服务端落盘 `~/.local/share/fuxi/vision/<request_id>.<ext>` → 触发对应 oneshot → 200 `{ok:true}`。失败 → 4xx，oneshot 也以 error 完成。

### WireEvent 新增（fuxi-core::EventKind）

```rust
VisionRequest {
    request_id: String,
    target: String,   // "webcam" | "screen"
    hint: Option<String>,
}
```

**同步更新 6 处**（CLAUDE.md 陷阱清单）：
1. `crates/fuxi-events/src/store.rs::kind_tag` → `"vision_request"`
2. `crates/fuxi-firehose/src/hub.rs::kind_tag`
3. `crates/fuxi-firehose/src/tui.rs::summarize` + `color_for`
4. `crates/fuxi-cli/src/subcommands.rs::event_summary`
5. 持久化 round-trip 测试

桌宠端 TS 同步加入 `WireKind` 联合（`apps/jarvis-pet/src/types/event.ts`）。

## 桌宠端实现

### 触发流程（`apps/jarvis-pet/src/components/PetCanvas.vue` 集成）

1. `FuxiClient.onEvent` 拿到 `vision_request`
2. 检查右键菜单里的「禁眼」开关 → 关时立即 POST `/api/xuannv/look/frame` 带 `error=user_denied`，桌宠 sprite 闪一下 ✕
3. 触发对应 capture：
   - `target=webcam`：`navigator.mediaDevices.getUserMedia({video:true})` → 取一帧 → canvas → blob (PNG)
   - `target=screen`：`navigator.mediaDevices.getDisplayMedia({video:true})` → 同上。**首次会触发系统弹窗让用户选屏**——这是 macOS 强制行为，**特性不是 bug**，让用户每个 session 选一次屏幕授权
4. blob → multipart POST `/api/xuannv/look/frame`，request_id 来自事件
5. UI 反馈：sprite 头顶 0.4s 一个微闪 + 桌宠右下角小圆点变蓝（capturing），完成后回灰；这套动画走 sprite mode（不引 emoji）

### 隐私 UI（右键菜单加项）

- `👁 眼睛`（用 unicode `◉` 图标，不是 emoji）
  - `允许 (默认)` ◉
  - `禁眼 15 分钟`
  - `永久禁眼` （重启失效或显式关）
- 状态点：左下角已有 mic 状态点，旁边再加一个 vision 点
  - 灰 = idle 可用
  - 蓝 = capturing
  - 红 = 用户禁了

### macOS 权限

- Camera：getUserMedia 触发系统弹窗，一次同意终身有效。Tauri 2 capability 已开放（pet 已用 mic），video 不需要额外配置
- Screen Recording：getDisplayMedia 触发 macOS 屏幕录制权限弹窗，**首次拒绝后只能去 系统设置→隐私→屏幕录制 手动开**——这一点要在右键菜单 tooltip 提一句

## 玄女侧（cc 提示词 + 工具暴露）

cc 通过 Bash tool 调 `fuxi xuannv look ...`。无需 MCP 注册（公理 4）。但需要让玄女**知道**这个工具存在——在 xuannv bootstrap prelude（`crates/fuxi-cli/src/xuannv_bootstrap.rs` 或对应 prelude 文档）追加一段：

> ## 你的眼睛
>
> 你可以用 `fuxi xuannv look --target webcam|screen [--hint "..."]` 看用户的真实世界。
> 命令成功后输出一行图片绝对路径，立即用 Read 工具读它——你能直接看到画面。
>
> 触发时机：
> - 用户主动说「看看」「看一眼」「这是什么」
> - 用户问的事情屏幕上显然有答案（"这报错啥意思"）
> - 别在 idle 期主动调，用户没邀请你不要看
>
> 失败时退非零，stderr 给原因——直白告诉用户（"我看不见你"），不要重试。

## 错误兜底矩阵

| 场景 | CLI 退出 | 玄女应说 |
|---|---|---|
| 桌宠离线 | 2 + `no_pet_connected` | "我现在看不见你（桌宠没连）" |
| 用户禁眼 | 3 + `user_denied` | "你把我眼睛蒙了，先去右键菜单解锁" |
| macOS 拒了屏幕权限 | 4 + `permission_denied` | "需要你在系统设置→隐私→屏幕录制里给我权限" |
| 拍帧超时 | 5 + `timeout` | "拍帧太慢，重新让我看一次？" |
| 上传失败 | 6 + `upload_failed` | "图传不上去，可能网断了" |

## v1 显式不做（YAGNI）

- 视频流（持续看）——只做单帧 snapshot
- `region`/`window` target——v1.1 再说
- 服务端图片预处理（裁剪/压缩/OCR 预解）——cc 自己看够了
- 帧持久化清理 cron——v1.1 加 24h TTL，v1 落盘不删
- 多桌宠场景下选哪台拍——广播给所有 pet 取最快回的那帧（其余 frame 回写 late 直接丢）
- 唤醒词后自动看一眼——先做 manual，行为足够稳定再放权
- 截图区域选择 UI（pet 内嵌画框）——v1.1

## 测试要求（TDD 强制）

每个交付物**必须先写失败测试再写实现**：

### 后端（Rust）
- `fuxi-core`: `VisionRequest` 序列化/反序列化 + 老事件兼容（`#[serde(default)]`）
- `fuxi-im`:
  - `/api/xuannv/look` 路径下：无 pet 连接 → 400；pet 连接但超时 → 408；pet 上传 frame → 200 + path 存在
  - `/api/xuannv/look/frame`：multipart 解析 + oneshot 完成
- `fuxi-cli`: `xuannv look` 子命令解析 + HTTP 调用 + 退出码映射

### 前端（pet TS）
- WireKind 解析 `vision_request` 不退化到 fallback
- 「禁眼」开关下点击 vision_request → 立即上传 error 不开 camera

### 集成
- 端到端：mock cc → 启 fuxi-im → mock pet WS 客户端模拟拍帧 → 拿到 path → 文件存在
- 手测：home 部署后跑一遍真桌宠，玄女说「看看我」/「看看屏幕」两条都通

## 实装拆分（agent team）

- **worker-α (Rust 后端)**：fuxi-core 加变体 + 6 处同步 + fuxi-im 两个 endpoint + fuxi-cli `xuannv look` 子命令
- **worker-β (前端 + Tauri)**：pet TS 加 WireKind 处理 + camera/screen capture + multipart upload + 右键菜单禁眼 + 状态点
- **team-lead (我)**：spec 同步 + xuannv prelude 提示词加段 + 集成测试 + home 部署 + 烟测

α/β 解耦点 = 这份 spec 里的 wire 契约。任何一方改契约要先改 spec 后通知另一方。

## 开发流程

1. α/β 各开 feature 分支：`feat/fuxi-vision-backend` / `feat/fuxi-vision-pet`
2. 各自跑 TDD 红绿绿循环
3. 全部 PR ready → team-lead 拉 `feat/fuxi-vision` 集成分支合二为一
4. CI 全绿（`cargo fmt --check` + `cargo clippy -D warnings` + `cargo test`）
5. home 部署：`scp` 新 fuxi binary 到 home 两份位置（CLAUDE.md 部署陷阱）+ `systemctl restart fuxi-im`
6. mac 端 pet：`npm run tauri build` → cp 新 .app 到 `~/Applications/` + `codesign --sign -`（feedback 教训）
7. 烟测：唤醒玄女 → "看看我" → "看看我屏幕" → 验证她描述靠谱

## 反公理 / 风险

- **隐私扩面**：眼睛比耳朵敏感得多。设计上靠"召唤式触发 + 桌宠端可禁眼 + capture 时视觉反馈"三层保护。**不接受任何 always-on 的妥协**——发现工程上有人偷加 cron poll，立即回退。
- **macOS 屏幕录制权限**：第一次必弹窗，体验略糙。无解，原生 API 限制。
- **图片留痕**：home 上 `~/.local/share/fuxi/vision/*.png` 短期不清，磁盘会涨。v1.1 必加 TTL。

## 决策记录

按 CLAUDE.md 决策档划分：

- 公理：眼睛归玄女自决用哪只 + 召唤式触发上限 + 工具走 CLI 不走 MCP
- 可见行为：右键菜单禁眼项 + capture 时蓝点
- 内部实现：oneshot pairing / multipart upload / `~/.local/share/fuxi/vision/` 落盘

## 后续路线（v1.1+）

- TTL cleanup
- region/window target
- 唤醒词后自动看一眼（need stable v1 行为再放权）
- 帧 OCR 预解（让 cc 不用花 token 在文字识别上）
- iOS 桌宠的眼睛——iPhone 摄像头视角（隔空给玄女看东西）

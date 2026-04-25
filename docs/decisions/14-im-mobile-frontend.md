# Decision 14 · IM 移动端前端 · 玄女在你口袋里

**日期**：2026-04-26
**状态**：已采纳（产品层拍板，实装见 v1.2 路线）

## 背景

用户日常在 ERP 上班，需要从手机派活给家里的 fuxi（让 home cc 干 ERP 维护），并能看进度、回话给玄女。已有：

- **后端基建 90% 现成**：`fuxi-firehose` axum WS+SSE+history、`fuxi-a2a` JSON-RPC、`Fuxi::intervene` 入口、`EventKind` 联合是 wire 格式
- **网络可达 100% 现成**：家里公网 IP + DDNS-go + nginx (`*.qmledmq.cn` 通配符证书 / 8443) → `fuxi.qmledmq.cn` 和 `im.qmledmq.cn` CNAME 已就位

用户明确否决"接现有 IM 做 bot"路线（"太受限了"）——要一套 fuxi 自己的 IM。

## 决策

### A · 心智模型：任务中心 + 内嵌 chat（不是 1-on-1，不是频道）

- **主屏 = root 任务卡片列表**（按状态/时间排）
- 顶部固定一条"跟玄女说"输入条 → `Fuxi::intervene(xuannv, false, text)`
- 点任务卡片进入 → 该 task 的 chat thread（玄女 + 门客 + tool 调用 + diff 全部一条 thread 渲染）
- 任务完成卡片置灰沉底
- **理由**：跟 Decision 10 task-bound 哲学一对一映射；用户真实场景"派活 → 离开 → 半路看进度 → 必要时插话"是 task lifecycle，不是聊天

否决方案：
- iMessage/WeChat 1-on-1 单 thread —— 任务进展会被淹没在文字流
- Slack/Discord 频道列表 —— 频道认知开销在手机小屏太重

### B · 网络可达：直接走现成 nginx 子域名

- `https://im.qmledmq.cn:8443` → nginx → `127.0.0.1:9100`（fuxi-im axum）
- 复用通配符证书，复用 nginx WS upgrade 模板
- **不用** Cloudflare Tunnel / Tailscale / 自建 VPS relay——那是没有公网 IP 的人才需要的兜底
- 配置入口：在 `/etc/nginx/sites-enabled/sia-gateway` 加一个 server block，跟现有 sia/play/lab/term 同一文件同一模式

### C · 后端：新 crate `fuxi-im`，跟 firehose / a2a 平级

axum on `:9100`，路由表：

| 方法 | 路径 | 后端动作 |
|---|---|---|
| GET | `/` | 静态 PWA（`include_dir!` 编译期打进 binary）|
| GET | `/api/tasks?root=1` | root 任务卡片列表 |
| GET | `/api/tasks/:id/events?from=<cursor>` | task 事件历史 + replay（复用 events crate）|
| WS  | `/api/tasks/:id/stream` | task 事件实时流（filter by task_id）|
| WS  | `/api/conv` | 跟玄女顶层对话流 |
| POST | `/api/intervene` | 用户向玄女说话 |
| POST | `/api/dispatch` | 强制开新 root task |
| POST | `/api/auth/pair` | 设备配对（一次性 PIN，TUI `/pair` 出码）|
| POST | `/api/push/subscribe` | 注册 Web Push subscription |

**事件序列化**：直接复用 `EventKind`（`#[serde(tag="type")]` 联合）。TUI 和 PWA 喝同一杯水，新 EventKind 加一次两个客户端同时能渲染。

### D · 鉴权：单用户 + 设备配对

- 手机首次开 PWA → 提示 "在 fuxi TUI 里跑 `/pair`" → TUI 弹 6 位 PIN → 手机输入 → 服务端 HMAC 签 device token 写 cookie，1 年到期
- TUI `/devices` 列出 + 吊销
- 不依赖 OAuth / 第三方 IdP；不开公开注册

### E · 通知：自签 VAPID Web Push

- `~/.fuxi/im_vapid.json` 存自签密钥
- 触发场景（**仅当 PWA 不在前台**）：
  1. 玄女 idle 等用户回复 >30s（intervene queue empty）→ "玄女在等你"
  2. root task 完成 → "ERP 任务 #N 完成 [open]"
- iOS 16.4+ PWA 添加到主屏后 Web Push 原生支持；Android 一直支持

### F · 前端：Solid + Vite，PWA 装

- Solid（bundle <30KB / HMR 飞快 / React-like 但响应式）
- 否决 React（重）/ HTMX（手机流式 UX 难做）/ SvelteKit（也 OK，但 Solid 更小更快）
- PWA manifest + Service Worker + IndexedDB 缓最近 100 条事件
- 三个 view：`#/` 任务卡片网格 · `#/conv` 跟玄女对话 · `#/task/:id` 单任务 chat（含工具调用折叠 / diff 预览）

### G · 视觉：手机现代化，**不抄** TUI

- **PWA 视觉与 TUI 视觉是两条线**（见 memory/feedback_pwa_modern_not_tui）
- 手机要：中文 sans-serif 正文、圆角卡片、44px 触控热区、流畅动画、字符级流式
- 等宽字体只用在 code block / 工具输出 / agent id 这种本来就该等宽的内容
- 暗底 + accent 色继承 fuxi 风
- **不抄** WeChat / Slack / iMessage 视觉；**默认不用 emoji**（fuxi 哲学）
- 视觉宣言："这是 fuxi 自己的 IM，不是别人的 bot 容器"

### H · v1 不做（明确砍掉）

- E2EE（nginx 自签链路可信，不是 Cloudflare 中间人）
- 多用户 / 多设备 ACL
- 文件 / 图片 / 音频上传
- iOS 原生 app（PWA 够用）
- TUI 那套 task-tree widget 镜像到 web

## 拆并行（agent team 草案）

按 `feedback_divide_conquer`，建议 6 teammate：

- **α · `fuxi-im` crate 骨架** · axum router + Fuxi 句柄注入 + 路由 unit test (mock)
- **β · 鉴权 + 设备配对** · `/pair` slash + token HMAC + cookie middleware
- **γ · WebSocket 事件流** · 玄女 conv WS + task-bound stream WS + EventKind serde
- **δ · Web Push** · VAPID keygen + subscribe 端点 + 触发钩子（玄女 idle / task done）
- **ε · PWA 静态打包** · Solid + Vite + include_dir!，三个 view + 流式渲染 + IndexedDB 缓存
- **ζ · nginx 部署** · vhost 模板 + `fuxi im start` 子命令 + systemd unit

α/β/γ/δ 是后端 Rust，可在同 workspace 并行；ε 是前端独立目录；ζ 是部署脚本，等 α 跑通再做。

测试遵循 `feedback_tdd_required`：每条先红再绿，e2e 走 daemon + reqwest + tungstenite 一圈。

## 砍掉的方案 / 否决理由

| 方案 | 否决理由 |
|---|---|
| 接 Lark/WeChat/Telegram 做 bot | 用户明示"太受限了" |
| Cloudflare Tunnel | 已有公网 IP + DDNS + nginx，多余一层 |
| Tailscale | 公网 IP 已可达；TS 没公网 URL 反而不利 Web Push |
| 自建 VPS relay | 已可达；800-1500 行 relay 代码无谓 |
| iMessage/WeChat 单 thread 心智模型 | 任务进展淹没在聊天流 |
| Slack/Discord 频道心智模型 | 手机小屏频道认知开销重 |
| TUI 视觉直搬手机 | 等宽中文阅读累 + 触控热区过小 |
| React 前端 | bundle 重 / HMR 慢，solo 项目无谓 |
| 写实 mockup 跟现有 IM 像 | 跟 fuxi"自己一套"宣言冲突 |

## 关联

- 决策 04（intervene idle 自动 degrade）—— `/api/intervene` 直接复用
- 决策 10（task-bound agents）—— 主屏任务卡片就是 task tree root
- 决策 12（dist-worker concurrency）—— 远期：手机能选 dispatch 到哪台节点
- memory/project_home_infra —— 网络拓扑细节
- memory/feedback_pwa_modern_not_tui —— 视觉规则源头

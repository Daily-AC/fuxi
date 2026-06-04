# 伏羲 IM 呆萌风重构 · 设计 spec

> 立于 2026-06-04。把 fuxi-im PWA（现"暖棕专业 Claude Code 风"）整套重做成**奶油糖果呆萌风** + 可交互**玄女吉祥物**。移动端优先。
> 现状：`crates/fuxi-im/web`（Solid + Vite PWA），安卓是 `apps/fuxi-im-android`（Capacitor 壳，直接包 web，本次只随 web 更新，无原生改动）。
> 本次决策由视觉伴侣逐项选定，详见 §0。

## 0. 已定方向（视觉伴侣选型结果）

| 维度 | 选定 | 备注 |
|---|---|---|
| 整体色调 | **A · 奶油糖果** | 暖奶白底 + 蜜桃/薄荷/粉彩，最纯呆萌 |
| 主屏布局 | **L1 · 客厅** | 新「家」主屏，玄女站 C 位 + 问候/状态/快捷入口 |
| 吉祥物造型 | **C1 · 仙女少女** | Q 版九天玄女本人（星簪、襦裙、踏祥云），身份感最强 |
| 动画方式 | **多帧桌宠** | 多张独立绘制帧资源切换（非整图缩放），透明 PNG |
| 交互深度 | **Level 2 · 会反应能戳** | idle/反应/可戳可拖，**不做养成/状态持久化** |
| 交付节奏 | **一次性全套替换** | 所有页一起换新设计 |
| 质感要求 | **拉满、反塑料反 AI** | 见 §2 质感系统（硬约束） |

## 1. 目标与非目标

**目标**
- 整个 IM 的视觉/布局/动效从零换成奶油糖果呆萌风，移动端体验第一。
- 玄女作为贯穿全 app 的吉祥物，有人格、会对事件做表情反应、可被用户戳/拖逗。
- 质感精致舒适，**绝不塑料感、绝不一眼 AI**（§2）。
- 复用现有后端 API 与数据流（A2A/EventBus/conv_store 不动），纯前端重构。

**非目标（YAGNI）**
- 不做养成系统 / 心情状态持久化 / 换装（Level 3）——本次只到「会反应」。
- 不上 Live2D / Spine 真骨骼 rig（列为未来升级，§4.4）。
- 不改后端接口、不动安卓原生层（FCM/Capacitor 壳照旧）。
- 不碰 jarvis 桌宠 / 语音（独立项目）。

## 2. 质感系统（硬约束 · 反塑料反 AI）

呆萌糖果配色最大的失败模式 = 廉价塑料 + 一眼 AI。本节是**所有页面都必须遵守**的底线，不是装饰建议。对应记忆 `feedback_premium_texture_no_plastic`。

1. **多层柔影，暖调不发灰**：阴影用蜜桃/暖棕染色（如 `rgba(120,80,50,.10)`），双层叠加（近距小实影 + 远距大柔影），大模糊低透明。禁止纯黑硬阴影、禁止均匀单层 drop-shadow。
2. **噪点/纸纹打底**：背景与大色块叠 3–5% 的细颗粒噪点（SVG `feTurbulence` 或 1 张 tile png），消灭"纯色块平铺"的塑料感。
3. **网格/网状渐变**：主屏与卡片底用柔和多色径向渐变（cream→peach→mint 极淡过渡），不要纯色填充、不要默认线性"AI 渐变"。
4. **暖光高光**：卡片顶部加极淡 inset 高光（`inset 0 1px 0 rgba(255,255,255,.6)`）模拟柔光打在软材质上，底部加极淡内阴影做厚度。
5. **克制玻璃深度**：底栏、sheet、overlay 用 frosted glass（`backdrop-filter: blur+saturate`）+ 半透明奶白，做出层次而非铺第二块塑料。
6. **材质感插画 + 手调 SVG 图标**：吉祥物是手绘质感（已具备）；图标全部圆头圆角手调 SVG（`stroke-linecap:round`），统一线重，**零 emoji**（记忆 `feedback_no_emoji_ui_too`）。
7. **有重量的微交互**：过渡用弹簧曲线（`cubic-bezier(.34,1.56,.64,1)` 等），按压 squash、内容 pop-in、吉祥物轻视差。避免线性匀速廉价感。
8. **讲究排印**：标题用圆润字体（圆体/鸿蒙圆体类）+ 正文克制字距/行高/字重层级；不靠默认系统渲染撑场面。
9. **prefers-reduced-motion**：所有动效有降级（关闭吉祥物/转场动画，保留静态精致）。

> 验收方式：每个页面落地后用浏览器实截图，**人眼**过一遍——看着像精致 app 还是像 AI 糖水 demo。不达标重做。

## 3. 设计 token（奶油糖果）

`tokens.ts` + `styles/global.css` 双出口同步改（现有约定）。值为初稿，落地时按 §2 微调。

```
surfaces   bg #FFF8F0 / surface #FFFFFF / surfaceSoft #FFFDFA / elevated #FFFFFF(+影)
            背景实际是 cream→peach→mint 极淡 mesh，不是纯 #FFF8F0
accent     peach 主 #FFB877（深 #E8915A 文字/描边）
mint       #8FD9B6 / 浅 #A8E6CF
lavender   #C9B6E8（玄女色）
pink       blush #FF9EB5
text       primary #5A4A3A / secondary #9A8C7A / muted #C3B4A2 / onAccent #FFFFFF
role       玄女 lavender#C9B6E8 / 鲁班 peach#E8915A / 蒲松 mint#6FB893（保持可区分，暖化）
semantic   success mint#6FB893 / warning peachgold#E8A23A / danger softcoral#E8857A
radius     card 20 / bubble 20 / sheet 28 / pill 999 / chip 14（整体比现在更圆更胖）
shadow     soft1 暖近影 / soft2 暖远影 / glow 吉祥物发光（见 §2.1）
type       标题 圆体；正文 现 sans 暖化；mono 仅代码块
motion     spring `cubic-bezier(.34,1.56,.64,1)` / soft `cubic-bezier(.4,0,.2,1)`
texture    noise tile + mesh gradient + glass blur 三件套 utility class
```

旧暖棕 token（`#1F1E1B` 等）全部废弃。

## 4. 玄女吉祥物系统

### 4.1 资源（多帧透明 PNG）
- 造型 = C1 仙女少女。每个状态一张独立绘制帧，**透明底**。
- 生产管线（已验证跑通）：`gpt-image-2`（i2i，ref=C1 锁角色）生成帧 → `rembg`（u2net 主体分割）抠透明 → 存 `public/mascot/<state>.png` + manifest。
  - 终版帧生图时背景用**纯色无装饰**（星点/云气改 app 内动态粒子），抠图更干净。
- 已有帧（8 张，已生成并抠透明，落在 `crates/fuxi-im/web/design-assets/mascot-v1/`：`source/` 原图 + `transparent/xuannv-<state>.png`）：idle(睁眼) / blink(眨眼) / happy(举手欢呼) / think(托腮) / talk(说话) / wave(招手) / sleep(打盹) / surprise(惊讶)。后续可加。
  - 当前透明帧是 ~2MB/张全分辨率 source；实装时压缩/切多尺寸（hero/mini/avatar）+ 可考虑 webp，进 `public/mascot/`。
- 尺寸：主屏 hero ~210px、聊天悬浮 mini ~60px、各页小头像 ~34px（同一张缩放或单独小图）。

### 4.2 状态机与触发（事件驱动，订后端 WS/SSE 推送，不轮询）

| 状态 | 触发 | 表现 |
|---|---|---|
| idle | 默认 | 呼吸 + 随机眨眼（2–4s） |
| talk | 玄女流式回复中 | 嘴型循环 + typing dots |
| think | 玄女处理中 / 门客在跑 | think 帧 + 周身转圈星点（= 全局 loading，§5） |
| happy | 任务完成 / 交付到达 / 被夸 | happy 帧 1.8s 后回 idle + 喜悦粒子 |
| wave | 进主屏/登录后问候 | wave 帧一次 |
| surprise | 报错 / 门客失败 / 重要通知 | surprise 帧 + 轻震 |
| sleep | 长时间无操作 / 夜间 | sleep 帧 + Zzz（轻触唤醒） |
| poke | 用户点/拖她 | wobble 挤压回弹 + 随机 happy/surprise + 俏皮话 |

- 「俏皮话」：戳一下随机冒一句气泡短语（本地文案池，呆萌口吻），非调用玄女后端。
- 可拖动（主屏/聊天悬浮），松手回弹归位。

### 4.3 组件
- 新 `components/Mascot/`：`<Mascot state size onPoke draggable/>`，内部预加载帧、CSS 呼吸/wobble/float、JS 计时器管 blink 与 idle→sleep 升级、`prefers-reduced-motion` 降级。
- 全局 `MascotController`（context/signal）：把 app 事件（流式中/任务完成/报错/通知）映射成 state，供主屏 hero 与聊天 mini 共用。
- 存在位置：**家(主屏) 大号 hero** + **聊天页 可拖动 mini**（其余页用静态小头像，不耗动效）。

### 4.4 未来升级（非本次）
分层 rig（Live2D 风）让呼吸/衣袖/发丝更顺滑——把 C1 切层做骨骼。本次先多帧，接口预留（Mascot 组件状态 API 不变即可换底层）。

## 5. Loading / 动效系统
- **全局等待**：think 帧 + 转圈星点粒子，**不用** spinner/转圈菊花。
- **骨架屏**：暖奶白 shimmer skeleton（卡片/列表占位），微光从左扫右。
- **流式回复**：talk 帧 + 跳动 typing dots。
- **下拉刷新**：吉祥物 bounce。
- **页面转场**：soft fade + slide（spring），NavigationStack push/pop 有方向感。
- **空状态**：吉祥物 + 一句呆萌引导（如交付收件箱空 = 玄女抱空篮子文案），不留冷冰冰"暂无数据"。

## 6. 信息架构 / 布局（L1 客厅）

底栏 4 tab：**家 · 聊天 · 任务 · 更多**（frosted glass 底栏，圆胖 SVG 图标，选中态 peach）。
- 「通知」从一级 tab 降级，提升进**家**（未读红点 + 通知卡 → tap 进通知页）。理由：家本就是"一眼掌控"的地方，通知是其中一块。

### 6.1 家（NEW · 客厅主屏）
- 玄女 hero（大号，会 wave 问候 + idle）。
- 问候语（按时间/名字，"早安，以琳"）。
- 状态条：`门客在跑 N · 未读通知 M`（来源 `fetchTasksOverview` + `fetchNotifications`）。
- 萌卡快捷入口：找玄女聊 / 看任务 / 看通知 / 交付物（点 hero 触发 poke）。
- 质感：mesh 渐变底 + 噪点 + 卡片柔影暖光（§2）。

### 6.2 页面原型（archetype）+ 全套映射

十几个页面不逐一 bespoke，而是收敛成 **5 个原型**（视觉伴侣已出 mockup 确认）。定原型 = 定所有页布局样式。

| 原型 | 版式 | 覆盖页面 |
|---|---|---|
| **家** | 玄女 hero + 问候 + 状态 + 萌卡入口（§6.1） | 家(新) |
| **聊天** | 顶栏 + 消息流 + 悬浮 mini 吉祥物 + composer（§6.2 详见消息族） | Conversation |
| **A 列表/收件箱** | 标题头 + 圆胖卡片列表（彩色圆角图标 + 标题/副标 + 状态 pill + chevron），柔影 + 顶部暖光高光，空态配玄女插画 | 任务 / 通知 / 交付物 / 节点 / 项目 / 工作者 / 角色 / 更漏 / 记忆 |
| **B 详情** | 返回头 + 顶部摘要卡 + 分组内容/时间线 + 底部主行动按钮 | 任务线程 / 项目详情 / 交付详情 |
| **C 宫格 hub** | 标题 + 2 列萌 tile（圆角彩图标 + 名 + 描述） | 更多 |
| **D 设置/表单** | 分组 section + 行（开关/值/箭头），圆头 toggle，暖色分隔 | 设置 |

所有页重绘为奶油糖果 + §2 质感；列表/卡片/气泡/按钮/输入框/sheet/modal/toast/空态全部换新组件样式。下面是组件级清单：

1. 登录 `LoginView`
2. **家** `HomePage`（新建，取代原 tab0 直接进会话）
3. 聊天 `Conversation` + 消息族（`UserBubble`/`XuannvBubble`/`WorkerBubble`/`ToolCallCard`/`ToolGroupCard`/`ThinkingRow`/`StreamingText`/`FileMessage`/`InlineFileCard`/`AttachmentChip`/`StatusMarkerRow`/`SystemMessageRow`）+ `Composer`/`MentionComposer`/`MentionAutocomplete`/`MentionChip` + `TopicSidebar`
4. 任务 `TasksPage` + `TaskThreadPage`
5. 通知 `NotificationsPage`
6. 更多 `MorePage` hub + 子页：`NodesPage` / `ProjectsPage` + `ProjectDetailPage` / `WorkerPage` / `DeliverablesPage` + `DeliverableDetailPage` / `MemoryPage` / `RolesPage` / `CronPage` / `SettingsPage`
7. 公共骨架：`BottomTabBar`（4 tab + glass）/ `NavigationStack` / `MoreSubShell` / `Toast` / 各 modal（归档/确认等已是 IM 风，重新上色）
8. PWA：icon / maskable / splash 重做（吉祥物头像系列），manifest 主题色改奶油。
9. 新增：`components/Mascot/`、质感 utility 层（noise/mesh/glass/shadow CSS）、圆体字体引入、`public/mascot/*` 资源 + manifest。

## 7. 技术方案
- 栈不变：Solid + Vite PWA + CSS Modules。安卓 = 重新 `npm run build` 后同步 Capacitor，无原生改动。
- 落地顺序（内部分阶段，对外仍是一次性上线）：
  1. **设计系统层**：tokens + global.css 重写 + §2 质感 utility + 圆体字体 + Mascot 组件（含帧资源 + 状态机）。先把"地基 + 吉祥物"立住。
  2. **主屏 + 聊天**：家(客厅) + Conversation 重做（核心日常路径）。
  3. **任务 / 通知 / 更多全子页** 逐页换新。
  4. **PWA 资源 + 收尾**：icon/splash/manifest + 空状态/loading/转场打磨 + 全局质感人眼复检。
- 复用后端：`fetchTasksOverview`/`fetchNotifications`/`fetchDeliverables`/`fetchNodes`/`fetchTopics`/`fetchHistory` 等现有端点，无需后端改动。
- TDD（CLAUDE.md 硬要求）：组件先写测试（vitest + solid testing-library），关键交互/导航/吉祥物状态用 playwright e2e。

## 8. 测试与验收
- vitest：Mascot 状态机（事件→state 映射）、各重绘组件渲染快照、token 一致性。
- playwright e2e：4-tab 导航、家→聊天/任务/通知跳转、吉祥物戳一下反应、loading/空状态出现。
- 人眼质感复检（§2 验收方式）：每页实截图自检，反塑料反 AI。
- home 实测：部署到家里 fuxi-im 后手机实用（移动端优先）。

## 9. 风险
- 吉祥物帧一致性：i2i 已验证锁得住；新帧若漂移则重生或微调。
- 透明抠图毛边：rembg 已验证干净；终版帧用纯色背景生图进一步降风险。
- 质感翻车（最大风险）：靠 §2 硬约束 + 每页人眼复检兜底，不靠运气。
- 工作量大（全套）：内部分 4 阶段推进，每阶段可独立验证；但对外一次性上线（用户选定）。

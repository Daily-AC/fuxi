const path = require("path");
const pptxgen = require("/Users/e0_7/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/pptxgenjs");

const ROOT = "/Users/e0_7/fuxi";
const OUT = path.join(ROOT, "deliverables/defense-ppt-cute");
const ASSETS = path.join(OUT, "assets");
const FIG = path.join(ROOT, "deliverables/thesis-v3/figures");
const IMG = path.join(ROOT, "deliverables/thesis-v3/images");

const pptx = new pptxgen();
pptx.defineLayout({ name: "WIDE", width: 13.333, height: 7.5 });
pptx.layout = "WIDE";
pptx.author = "张以琳";
pptx.subject = "基于 AI Agent 的高性能分布式通讯系统";
pptx.title = "Fuxi 毕业论文答辩 Q萌版";
pptx.company = "湖南第一师范学院";
pptx.lang = "zh-CN";
pptx.theme = {
  headFontFace: "PingFang SC",
  bodyFontFace: "PingFang SC",
  lang: "zh-CN"
};

const C = {
  cream: "FFF8ED",
  cream2: "FFF1D6",
  ink: "293241",
  sub: "64748B",
  red: "E8342F",
  peach: "FFB38A",
  peach2: "FFE2CD",
  mint: "A8E6CF",
  mint2: "E0F7EF",
  blue: "A8D8FF",
  blue2: "E5F4FF",
  yellow: "FFD36E",
  yellow2: "FFF3C4",
  purple: "CDB4FF",
  purple2: "EFE7FF",
  green: "57CC99",
  line: "F2B5A2",
  white: "FFFFFF"
};

const logo = path.join(ASSETS, "hnfnu-emblem-official-page.png");
const hero = path.join(ASSETS, "fuxi-cute-hero.png");

function txt(slide, text, x, y, w, h, opts = {}) {
  slide.addText(text, {
    x, y, w, h,
    margin: opts.margin ?? 0.04,
    fit: "shrink",
    fontFace: opts.fontFace || "PingFang SC",
    fontSize: opts.fontSize ?? 14,
    bold: opts.bold ?? false,
    color: opts.color || C.ink,
    valign: opts.valign || "mid",
    align: opts.align || "left",
    breakLine: opts.breakLine ?? false,
    paraSpaceAfterPt: opts.paraSpaceAfterPt ?? 0,
    ...opts
  });
}

function bg(slide) {
  slide.background = { color: C.cream };
  slide.addShape(pptx.ShapeType.rect, { x: 0, y: 0, w: 13.333, h: 7.5, fill: { color: C.cream }, line: { color: C.cream } });
  doodle(slide, 0.42, 0.45, C.yellow, 0.28);
  doodle(slide, 12.28, 0.88, C.mint, 0.22);
  doodle(slide, 11.88, 6.35, C.peach, 0.26);
  doodle(slide, 0.58, 6.38, C.blue, 0.18);
  for (let i = 0; i < 5; i++) {
    slide.addShape(pptx.ShapeType.arc, {
      x: 0.6 + i * 2.6, y: 6.95, w: 0.55, h: 0.22,
      adjustPoint: 0.35,
      line: { color: i % 2 ? C.peach : C.blue, width: 1.1, transparency: 22 }
    });
  }
}

function doodle(slide, x, y, color, s = 0.22) {
  slide.addShape(pptx.ShapeType.ellipse, { x, y, w: s, h: s, fill: { color, transparency: 18 }, line: { color, transparency: 100 } });
  slide.addShape(pptx.ShapeType.ellipse, { x: x + s * 0.46, y: y + s * 0.1, w: s * 0.42, h: s * 0.42, fill: { color: C.white, transparency: 6 }, line: { color, transparency: 100 } });
}

function stamp(slide, n) {
  slide.addShape(pptx.ShapeType.roundRect, {
    x: 0.54, y: 0.32, w: 0.62, h: 0.34,
    rectRadius: 0.08,
    fill: { color: C.red, transparency: 8 },
    line: { color: C.red, transparency: 20, width: 1 }
  });
  txt(slide, String(n).padStart(2, "0"), 0.54, 0.4, 0.62, 0.1, { fontSize: 8, bold: true, color: C.white, align: "center" });
}

function title(slide, n, tag, headline, sub) {
  stamp(slide, n);
  txt(slide, tag, 1.28, 0.35, 3.4, 0.2, { fontSize: 8.5, bold: true, color: C.red, charSpace: 1.2 });
  txt(slide, headline, 0.72, 0.78, 8.8, 0.5, { fontSize: 23, bold: true, color: C.ink });
  if (sub) txt(slide, sub, 0.76, 1.26, 8.9, 0.24, { fontSize: 10.2, color: C.sub });
}

function footer(slide, n) {
  slide.addShape(pptx.ShapeType.line, { x: 0.72, y: 7.02, w: 11.85, h: 0, line: { color: "F1C2B3", width: 1, transparency: 15 } });
  slide.addImage({ path: logo, x: 0.76, y: 7.08, w: 0.22, h: 0.22 });
  txt(slide, "湖南第一师范学院 · 张以琳 · Fuxi 毕业论文答辩", 1.04, 7.13, 4.5, 0.12, { fontSize: 7.2, color: C.sub });
  txt(slide, String(n).padStart(2, "0"), 12.18, 7.08, 0.38, 0.18, { fontSize: 8, color: C.red, bold: true, align: "right" });
}

function card(slide, x, y, w, h, fill = C.white, stroke = C.line) {
  slide.addShape(pptx.ShapeType.roundRect, {
    x, y, w, h,
    rectRadius: 0.09,
    fill: { color: fill },
    line: { color: stroke, transparency: 8, width: 1.15 },
    shadow: { type: "outer", color: "E7A38E", opacity: 0.10, blur: 1, angle: 45, distance: 1 }
  });
}

function pill(slide, text, x, y, w, fill = C.yellow2, color = C.red) {
  slide.addShape(pptx.ShapeType.roundRect, { x, y, w, h: 0.33, rectRadius: 0.09, fill: { color: fill }, line: { color, transparency: 45, width: 0.8 } });
  txt(slide, text, x + 0.12, y + 0.075, w - 0.24, 0.13, { fontSize: 8.4, bold: true, color, align: "center" });
}

function metric(slide, value, label, x, y, w, accent, fill) {
  card(slide, x, y, w, 1.08, fill, accent);
  txt(slide, value, x + 0.16, y + 0.13, w - 0.32, 0.33, { fontSize: 21, bold: true, color: accent, align: "center" });
  txt(slide, label, x + 0.16, y + 0.58, w - 0.32, 0.32, { fontSize: 8.8, color: C.ink, align: "center", breakLine: true });
}

function bullets(slide, items, x, y, w, h, size = 11.2) {
  const runs = items.map(t => ({ text: t, options: { bullet: { type: "ul" }, breakLine: true } }));
  slide.addText(runs, { x, y, w, h, margin: 0.04, fontFace: "PingFang SC", fontSize: size, color: C.ink, breakLine: false, paraSpaceAfterPt: 6, fit: "shrink" });
}

function quote(slide, text, x, y, w, h, fill = C.yellow2) {
  card(slide, x, y, w, h, fill, C.peach);
  slide.addShape(pptx.ShapeType.rect, { x: x + 0.18, y: y + 0.17, w: 0.06, h: h - 0.34, fill: { color: C.red }, line: { color: C.red } });
  txt(slide, text, x + 0.36, y + 0.12, w - 0.52, h - 0.24, { fontSize: 12.6, bold: true, color: C.ink, breakLine: true });
}

function img(slide, file, x, y, w, h) {
  card(slide, x - 0.03, y - 0.03, w + 0.06, h + 0.06, C.white, C.peach);
  slide.addImage({ path: file, x, y, w, h, sizing: { type: "contain", x, y, w, h } });
}

function mascot(slide, x, y, mood = "happy", color = C.mint) {
  slide.addShape(pptx.ShapeType.ellipse, { x, y, w: 0.58, h: 0.58, fill: { color }, line: { color: C.ink, transparency: 55, width: 0.7 } });
  slide.addShape(pptx.ShapeType.ellipse, { x: x + 0.15, y: y + 0.2, w: 0.07, h: 0.07, fill: { color: C.ink }, line: { color: C.ink } });
  slide.addShape(pptx.ShapeType.ellipse, { x: x + 0.36, y: y + 0.2, w: 0.07, h: 0.07, fill: { color: C.ink }, line: { color: C.ink } });
  const mouth = mood === "focus" ? "•" : "⌣";
  txt(slide, mouth, x + 0.22, y + 0.31, 0.15, 0.1, { fontSize: 12, bold: true, color: C.ink, align: "center" });
}

function miniFlow(slide, x, y, labels, colors) {
  labels.forEach((l, i) => {
    const bx = x + i * 2.25;
    slide.addShape(pptx.ShapeType.roundRect, {
      x: bx, y, w: 1.6, h: 0.86,
      rectRadius: 0.09,
      fill: { color: colors[i], transparency: 58 },
      line: { color: colors[i], transparency: 10, width: 1.15 },
      shadow: { type: "outer", color: "E7A38E", opacity: 0.10, blur: 1, angle: 45, distance: 1 }
    });
    mascot(slide, bx + 0.1, y + 0.14, i === 1 ? "focus" : "happy", colors[i]);
    txt(slide, l, bx + 0.75, y + 0.25, 0.72, 0.16, { fontSize: 8.8, bold: true, color: C.ink, align: "center" });
    if (i < labels.length - 1) {
      slide.addShape(pptx.ShapeType.chevron, { x: bx + 1.72, y: y + 0.24, w: 0.32, h: 0.32, fill: { color: C.peach }, line: { color: C.peach } });
    }
  });
}

function block(slide, heading, body, x, y, w, accent, fill, h = 1.56) {
  card(slide, x, y, w, h, fill, accent);
  mascot(slide, x + 0.17, y + 0.2, "happy", accent);
  txt(slide, heading, x + 0.9, y + 0.25, w - 1.05, 0.22, { fontSize: 13.2, bold: true, color: C.ink });
  txt(slide, body, x + 0.9, y + 0.62, w - 1.1, h - 0.72, { fontSize: 9.5, color: C.sub, breakLine: true, valign: "top" });
}

function notes(slide, arr) { slide.addNotes(arr.join("\n")); }

let s, n = 1;

s = pptx.addSlide(); bg(s);
s.addImage({ path: hero, x: 5.3, y: 0.15, w: 7.85, h: 4.42 });
s.addShape(pptx.ShapeType.roundRect, { x: 0.55, y: 0.42, w: 1.0, h: 1.0, rectRadius: 0.12, fill: { color: C.white, transparency: 0 }, line: { color: C.peach, width: 1 } });
s.addImage({ path: logo, x: 0.68, y: 0.55, w: 0.74, h: 0.74 });
txt(s, "湖南第一师范学院 · 毕业论文答辩", 1.78, 0.58, 3.6, 0.2, { fontSize: 10.5, bold: true, color: C.red });
txt(s, "基于 AI Agent 的\n高性能分布式通讯系统", 0.72, 1.54, 5.2, 1.25, { fontSize: 31, bold: true, color: C.ink, breakLine: true });
quote(s, "把多 Agent 系统从“演示脚本”推到“可观测、可回放、可审计的日用平台”。", 0.78, 3.12, 4.7, 0.92);
txt(s, "答辩人：张以琳\n日期：2026 年 5 月 9 日", 0.82, 5.88, 3.0, 0.43, { fontSize: 12.5, bold: true, color: C.ink, breakLine: true });
miniFlow(s, 4.1, 5.7, ["玄女", "事件总线", "门客"], [C.peach, C.blue, C.mint]);
notes(s, ["开场说明：这是一套 Rust 本地优先多 Agent 编排平台。", "Q萌风格服务表达，技术内容仍按论文严肃口径讲。"]);

s = pptx.addSlide(); bg(s); title(s, n, "PROBLEM", "问题：多 Agent 真正难在通讯底座", "角色协作有效，但工程化短板集中在状态、通讯和可观测性。"); footer(s, n++);
block(s, "通讯协议不统一", "跨进程、跨节点以后，Python 对象互调不再够用。", 0.8, 2.0, 3.45, C.blue, C.blue2);
block(s, "任务生命周期缺位", "Agent 会执行很久、会中断、会等待用户输入。", 4.95, 2.0, 3.45, C.peach, C.peach2);
block(s, "运行时状态难追踪", "工具调用、子任务和异常都必须可回放。", 9.1, 2.0, 3.45, C.purple, C.purple2);
quote(s, "本文的问题不是“让 Agent 会聊天”，而是让多个 Agent 在本机与跨节点环境中可靠协作。", 1.28, 5.48, 10.8, 0.66);
notes(s, ["本页立题：不是聊天能力，而是通讯底座。"]);

s = pptx.addSlide(); bg(s); title(s, n, "TARGET", "研究目标与设计假设", "Rust、本地优先、事件驱动、可观测、可跨节点扩展。"); footer(s, n++);
quote(s, "实现一个本地优先、事件驱动、可观测、可跨节点扩展的多 AI Agent 协作平台。", 0.9, 1.62, 11.5, 0.62, C.blue2);
metric(s, "13", "crate 模块边界", 0.86, 3.02, 2.02, C.blue, C.blue2);
metric(s, "8.25 万", "约 Rust 代码行", 3.18, 3.02, 2.05, C.peach, C.peach2);
metric(s, "1363 + 27", "单元测试 + 集成测试", 5.52, 3.02, 2.28, C.green, C.mint2);
metric(s, "ERP", "真实项目日常验证", 8.08, 3.02, 2.0, C.purple, C.purple2);
metric(s, "5", "可独立审视的贡献", 10.38, 3.02, 2.0, C.red, C.yellow2);
bullets(s, ["事件驱动 + 追加式日志，同时兼顾吞吐、实时性与可追溯性。", "协议语义、事实流、调度边界、执行隔离必须同时成立。"], 1.0, 5.25, 10.9, 0.78, 12.2);
notes(s, ["讲规模和目标。数字口径来自论文摘要与总结。"]);

s = pptx.addSlide(); bg(s); title(s, n, "ARCHITECTURE", "总体架构：核心—通讯—编排—执行—观测", "A2A 是契约语义，EventBus 是事实流；两者正交。"); footer(s, n++);
img(s, path.join(FIG, "drawio/fig-2-1-overall-architecture.png"), 0.75, 1.62, 7.42, 4.85);
card(s, 8.52, 1.7, 3.88, 4.42, C.white, C.blue);
txt(s, "讲图顺序", 8.82, 1.98, 1.4, 0.22, { fontSize: 12, bold: true, color: C.red });
bullets(s, ["fuxi-core 定义 Agent / Task / Workspace / Event。", "fuxi-events 记录事实，fuxi-a2a 承担契约。", "玄女维护门客注册表，所有协作经编排层记录。", "执行层接 Claude Code / Codex，并通过 worktree 隔离。", "观测层用 Firehose、WS、SSE、IM 读同一事件流。"], 8.82, 2.42, 3.15, 2.7, 9.8);
pill(s, "协议不保存事实，事件流不替代协议", 8.8, 5.56, 3.2, C.yellow2, C.red);
notes(s, ["强调 A2A 和 EventBus 的边界。"]);

s = pptx.addSlide(); bg(s); title(s, n, "CONTRIBUTIONS", "五项贡献：围绕一个命题展开", "把 Agent 间通讯的契约、事实流、调度边界与执行隔离同时做对。"); footer(s, n++);
[
  ["A2A 风格协议适配层", "五条核心路径；input-required 提升为平台级人工介入。", C.blue, C.blue2],
  ["非阻塞事件总线", "Tokio broadcast + SQLite WAL；try_send + 后台转交 + lag 哨兵。", C.peach, C.peach2],
  ["玄女—门客分层", "用户入口与执行 Agent 解耦；编排层不可绕过。", C.green, C.mint2],
  ["三层沙箱 × WS 反连", "L1/L2/L3 worktree 隔离；编排层主动控制门客。", C.purple, C.purple2],
  ["跨节点扩展钩子", "DistEnqueuer / NodeLoadProvider 等 trait 注入；HMAC 验签。", C.red, C.yellow2]
].forEach((r, i) => block(s, r[0], r[1], 0.9 + (i % 2) * 5.65, 1.65 + Math.floor(i / 2) * 1.65, i === 4 ? 11.1 : 5.15, r[2], r[3], 1.25));
notes(s, ["主目录页。后面展开重点机制与实验。"]);

s = pptx.addSlide(); bg(s); title(s, n, "A2A", "A2A 适配：协议语义进入平台状态机", "`input-required` 不只是 wire 字段，而是可观测、可调度的人工介入状态。"); footer(s, n++);
img(s, path.join(IMG, "代码3-3-TaskState枚举.png"), 0.8, 1.64, 5.5, 1.7);
img(s, path.join(IMG, "算法3-1-SSE帧解析.png"), 0.8, 3.78, 5.5, 2.05);
card(s, 6.8, 1.65, 5.58, 2.42, C.white, C.peach);
bullets(s, ["覆盖 discovery、send、stream、query、cancel 五条主路径。", "沿用早期 A2A JSON-RPC binding，论文中明确不主张完整 v1.0 兼容。", "TASK_STATE_INPUT_REQUIRED 映射到 PendingApproval / AwaitingInput。"], 7.08, 1.98, 4.95, 1.5, 10.5);
metric(s, "1321 行", "fuxi-a2a crate", 7.05, 4.52, 2.25, C.peach, C.peach2);
metric(s, "5 路径", "发现 / 发送 / 流式 / 查询 / 取消", 9.78, 4.52, 2.25, C.blue, C.blue2);
notes(s, ["回答为什么不用现成 a2a-rs：差异点在和本地编排层、事件总线、人工介入语义深度集成。"]);

s = pptx.addSlide(); bg(s); title(s, n, "EVENT BUS", "事件总线：实时推送与历史回放共用一套抽象", "publish 路径不阻塞调用方，同时让背压变成可见告警。"); footer(s, n++);
img(s, path.join(IMG, "代码3-1-EventBus非阻塞publish.png"), 0.75, 1.58, 6.35, 4.68);
["broadcast：零拷贝扇出", "try_send：异步 writer 落 SQLite WAL", "lag sentinel：阈值 512 可见告警"].forEach((t, i) => block(s, `0${i + 1}`, t, 7.58, 1.82 + i * 1.17, 4.42, [C.blue, C.peach, C.green][i], [C.blue2, C.peach2, C.mint2][i], 0.88));
quote(s, "关键取舍：宁可暴露 lag，也不让调用方在 publish() 上被持久化路径拖住。", 7.58, 5.55, 4.42, 0.66);
notes(s, ["解释双路径：广播用于实时，WAL 用于历史与审计。"]);

s = pptx.addSlide(); bg(s); title(s, n, "ORCHESTRATION", "玄女—门客：编排层不可绕过", "顶层 Agent 负责对话与调度，工作 Agent 负责执行；所有协作都留下事件。"); footer(s, n++);
img(s, path.join(FIG, "drawio/fig-3-1-execution-loop.png"), 0.78, 1.55, 6.15, 4.82);
card(s, 7.34, 1.6, 4.98, 2.35, C.white, C.green);
bullets(s, ["玄女是用户对话面唯一入口。", "门客私有事件补齐 task_id / agent_id 后再进入全局 EventBus。", "claim_idle_by_role 原子化避免并发派活撞同一门客。", "shutdown_agent 对玄女做硬豁免。"], 7.64, 1.94, 4.35, 1.42, 10.1);
img(s, path.join(IMG, "代码3-4-claim_idle原子化.png"), 7.42, 4.38, 4.82, 1.05);
pill(s, "编排层是事实流的边界", 8.08, 5.86, 3.4, C.yellow2, C.red);
notes(s, ["把玄女—门客讲成工程分层，不讲成设定。"]);

s = pptx.addSlide(); bg(s); title(s, n, "EXECUTION", "执行隔离：三层沙箱 × WebSocket 反连", "把 headless Agent CLI 接进平台，而不是把平台绑死在某一个 CLI 上。"); footer(s, n++);
img(s, path.join(IMG, "代码3-5-CcAgent反连握手.png"), 0.78, 1.5, 5.75, 2.0);
img(s, path.join(IMG, "附录代码A-10-L2_L3沙箱差异.png"), 0.78, 3.9, 5.75, 1.95);
metric(s, "L1", "只读视图\n轻量查询", 7.05, 1.65, 1.55, C.blue, C.blue2);
metric(s, "L2", "任务级 worktree\n并发修改隔离", 8.92, 1.65, 1.75, C.peach, C.peach2);
metric(s, "L3", "角色级持久沙箱\n复用构建缓存", 10.98, 1.65, 1.75, C.green, C.mint2);
quote(s, "WS 反连让编排层能主动 send_message / cancel，打破传统 stdio 只能等门客 poll 的限制。", 7.25, 4.18, 4.78, 1.18, C.purple2);
notes(s, ["并发文件操作不互相踩，是这页重点。"]);

s = pptx.addSlide(); bg(s); title(s, n, "CROSS NODE", "跨节点扩展：单机优先，扩展点钩子化", "默认单节点完整运行；多节点能力通过 trait 注入，不污染核心编排层。"); footer(s, n++);
img(s, path.join(FIG, "drawio/fig-2-3-cross-node.png"), 0.72, 1.55, 5.35, 4.62);
card(s, 6.62, 1.66, 5.72, 2.08, C.white, C.blue);
bullets(s, ["Project.host_nodes 标记可执行节点。", "auto_pin_from_project 尊重显式 @node。", "按 inflight / max_concurrency 选择最低饱和节点。", "HMAC-SHA256 + nonce cache + 常量时间比较。"], 6.92, 1.95, 5.08, 1.35, 10.1);
img(s, path.join(IMG, "算法3-3-HMAC验签.png"), 6.72, 4.28, 5.44, 1.36);
pill(s, "边界：已跑通两节点 sandbox；大规模故障转移留待后续", 6.85, 6.03, 5.15, C.yellow2, C.red);
notes(s, ["主动承认边界，不夸大成完整大规模分布式系统。"]);

s = pptx.addSlide(); bg(s); title(s, n, "EVALUATION", "实验设计：先隔离 LLM，再测通讯层上限", "合成基准测的是编排与通讯路径，不把模型推理时间混进系统指标。"); footer(s, n++);
img(s, path.join(IMG, "表4-1-测试覆盖.png"), 0.75, 1.58, 5.7, 1.75);
img(s, path.join(IMG, "表4-2-吞吐基线.png"), 0.75, 3.88, 5.7, 1.55);
card(s, 6.92, 1.66, 5.36, 2.05, C.white, C.peach);
bullets(s, ["每任务 10 ms sleep，剥离 LLM 推理。", "吞吐采用 5-run median。", "延迟采用 500 样本统计 p50 / p99。", "真实 LLM Agent 吞吐主要受单次推理 1–30 s 主导。"], 7.2, 1.95, 4.75, 1.4, 10.2);
metric(s, "cargo test", "workspace all-targets 通过", 7.08, 4.62, 2.15, C.green, C.mint2);
metric(s, "500", "延迟样本统计", 9.78, 4.62, 2.1, C.peach, C.peach2);
notes(s, ["用这页防御为什么实验是 10ms sleep。"]);

s = pptx.addSlide(); bg(s); title(s, n, "RESULTS", "结果一：Worker 扩展性没有在 16 worker 前饱和", "8 worker 达 665 tasks/s；16 worker 达 1288.24 tasks/s。"); footer(s, n++);
img(s, path.join(FIG, "matplotlib/fig-5-1-scalability.png"), 0.7, 1.55, 7.15, 2.88);
img(s, path.join(IMG, "表4-3-Worker扩展性.png"), 0.7, 4.88, 7.15, 1.12);
metric(s, "665.00", "8 worker\n调度路径吞吐 tasks/s", 8.35, 1.62, 2.05, C.peach, C.peach2);
metric(s, "1288.24", "16 worker\ntasks/s", 10.75, 1.62, 1.9, C.blue, C.blue2);
metric(s, "80.5–83.1%", "1→16 worker\n扩展效率", 8.35, 3.06, 2.05, C.green, C.mint2);
metric(s, "未饱和", "dispatcher 不是\n主瓶颈", 10.75, 3.06, 1.9, C.purple, C.purple2);
quote(s, "结论：通讯与调度路径在目标规模内有余量，真实瓶颈会转移到 LLM 推理。", 8.35, 5.16, 4.3, 0.72);
notes(s, ["这些数字是剥离推理后的系统层上限。"]);

s = pptx.addSlide(); bg(s); title(s, n, "RESULTS", "结果二：事件流延迟比派发链路低三个数量级", "观测端应当跟得上任务过程，而不是靠轮询追尾。"); footer(s, n++);
img(s, path.join(FIG, "matplotlib/fig-5-6-e2e-breakdown.png"), 0.68, 1.56, 6.45, 2.4);
img(s, path.join(FIG, "matplotlib/fig-5-5-event-flow-latency.png"), 0.68, 4.4, 3.05, 1.7);
img(s, path.join(FIG, "matplotlib/fig-5-4-dispatch-latency.png"), 4.08, 4.4, 3.05, 1.7);
metric(s, "32.93 ms", "任务派发 p50", 7.62, 1.62, 2.12, C.peach, C.peach2);
metric(s, "0.08 ms", "事件广播 p50", 10.08, 1.62, 2.12, C.blue, C.blue2);
metric(s, "0.26 ms", "事件广播 p99", 7.62, 3.05, 2.12, C.green, C.mint2);
metric(s, "410×", "event_flow 与 dispatch\np50 差距", 10.08, 3.05, 2.12, C.purple, C.purple2);
quote(s, "通讯层低延迟让 Firehose、TUI、PWA 能近实时显示过程事件。", 7.62, 5.18, 4.58, 0.64, C.yellow2);
notes(s, ["重点是观测端能无轮询近实时。"]);

s = pptx.addSlide(); bg(s); title(s, n, "RESULTS", "结果三：事件总线压力测试给出容量边界", "64 subscriber × 10k events/s 零丢帧；16 subscriber × 100k events/s 可持续。"); footer(s, n++);
img(s, path.join(FIG, "matplotlib/fig-5-3-bus-stress.png"), 0.72, 1.58, 7.1, 2.82);
img(s, path.join(IMG, "表4-6-事件总线压测.png"), 0.72, 4.88, 7.1, 1.05);
metric(s, "64 × 10k/s", "持续负载\n零丢帧", 8.36, 1.64, 2.1, C.blue, C.blue2);
metric(s, "1.6M events/s", "sustainable\n容量边界", 8.36, 3.08, 2.1, C.peach, C.peach2);
metric(s, "3 个数量级", "高于实际负载\n估算余量", 8.36, 4.52, 2.1, C.green, C.mint2);
txt(s, "payload size 影响很小，符合 Arc clone 扇出模型预期。64×100k/s 出现丢帧，是系统容量边界，不作为日常负载目标。", 10.78, 1.72, 1.52, 4.1, { fontSize: 10.1, color: C.ink, breakLine: true, valign: "mid" });
notes(s, ["64x100k/s 是边界，主结论是实际负载远低于 sustainable 边界。"]);

s = pptx.addSlide(); bg(s); title(s, n, "LIMITS", "局限与后续工作", "主动承认边界，比被问住更好。"); footer(s, n++);
[
  ["A2A 规范跟进", "补齐 v1.0 PascalCase 与 one-of 互通测试。", C.peach, C.peach2],
  ["安全模型", "工具授权、数据访问控制、提示注入仍需统一策略。", C.blue, C.blue2],
  ["分布式一致性", "更大节点数、不可靠链路与故障转移尚未充分验证。", C.green, C.mint2],
  ["Agent 适配生态", "把 Claude Code / Codex 经验沉淀为通用 CLI 适配框架。", C.purple, C.purple2]
].forEach((r, i) => block(s, r[0], r[1], 0.86 + (i % 2) * 5.8, 1.74 + Math.floor(i / 2) * 2.0, 5.28, r[2], r[3], 1.35));
quote(s, "论文记录的是 Fuxi v1/v2 阶段的工程化证据；平台不会因答辩结束而停止演进。", 1.18, 6.08, 10.9, 0.58);
notes(s, ["局限页要坦诚。"]);

s = pptx.addSlide(); bg(s);
s.addShape(pptx.ShapeType.roundRect, { x: 0.72, y: 0.56, w: 1.0, h: 1.0, rectRadius: 0.12, fill: { color: C.white }, line: { color: C.peach, width: 1 } });
s.addImage({ path: logo, x: 0.86, y: 0.7, w: 0.72, h: 0.72 });
txt(s, "Q&A", 0.82, 1.92, 2.3, 0.62, { fontSize: 36, bold: true, color: C.ink });
txt(s, "谢谢各位老师", 0.86, 2.64, 2.25, 0.28, { fontSize: 14, bold: true, color: C.red });
quote(s, "Fuxi 的差异化不在于“最快的 Agent 框架”，而在于把通讯底座从应用流程中显式剥离出来：事件流可观测可回放，协议层可独立部署，执行层可隔离扩展。", 0.86, 3.38, 5.05, 1.38, C.yellow2);
img(s, path.join(FIG, "drawio/fig-1-1-overview.png"), 6.48, 1.28, 5.72, 4.65);
miniFlow(s, 1.1, 5.85, ["问题", "方案", "验证"], [C.peach, C.blue, C.green]);
footer(s, n++);
notes(s, ["结束：感谢老师，欢迎批评指正。"]);

pptx.writeFile({ fileName: path.join(OUT, "fuxi-thesis-defense-cute-q-2026-05-09.pptx") });

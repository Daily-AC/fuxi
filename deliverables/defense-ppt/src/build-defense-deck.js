const path = require("path");
const pptxgen = require("/Users/e0_7/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/pptxgenjs");

const ROOT = "/Users/e0_7/fuxi";
const OUT = path.join(ROOT, "deliverables/defense-ppt");
const ASSETS = path.join(OUT, "assets");
const FIG = path.join(ROOT, "deliverables/thesis-v3/figures");
const IMG = path.join(ROOT, "deliverables/thesis-v3/images");

const pptx = new pptxgen();
pptx.layout = "LAYOUT_WIDE";
pptx.author = "以琳";
pptx.subject = "基于 AI Agent 的高性能分布式通讯系统";
pptx.title = "Fuxi 毕业论文答辩";
pptx.company = "Fuxi";
pptx.lang = "zh-CN";
pptx.theme = {
  headFontFace: "PingFang SC",
  bodyFontFace: "PingFang SC",
  lang: "zh-CN"
};
pptx.defineLayout({ name: "CUSTOM_WIDE", width: 13.333, height: 7.5 });
pptx.layout = "CUSTOM_WIDE";
pptx.margin = 0;
pptx.slideWidth = 13.333;
pptx.slideHeight = 7.5;

const C = {
  bg: "0F172A",
  panel: "111C31",
  panel2: "162238",
  ink: "EAF0F8",
  muted: "9BA8B8",
  soft: "CBD5E1",
  line: "31435F",
  teal: "38D5D5",
  copper: "F59E6B",
  rust: "C86B3D",
  green: "7DD3A8",
  violet: "A78BFA",
  white: "FFFFFF"
};

function addBg(slide, color = C.bg) {
  slide.background = { color };
  slide.addShape(pptx.ShapeType.rect, {
    x: 0, y: 0, w: 13.333, h: 7.5,
    fill: { color },
    line: { color, transparency: 100 }
  });
}

function txt(slide, text, x, y, w, h, opts = {}) {
  slide.addText(text, {
    x, y, w, h,
    margin: opts.margin ?? 0.05,
    breakLine: false,
    fit: "shrink",
    fontFace: opts.fontFace || "PingFang SC",
    fontSize: opts.fontSize ?? 20,
    bold: opts.bold ?? false,
    color: opts.color || C.ink,
    valign: opts.valign || "mid",
    align: opts.align || "left",
    paraSpaceAfterPt: opts.paraSpaceAfterPt ?? 0,
    breakLine: opts.breakLine ?? false,
    ...opts
  });
}

function title(slide, kicker, headline, sub) {
  txt(slide, kicker, 0.55, 0.34, 3.8, 0.22, { fontSize: 8.5, bold: true, color: C.teal, charSpace: 1.2 });
  txt(slide, headline, 0.55, 0.62, 7.7, 0.62, { fontSize: 27, bold: true, color: C.white });
  if (sub) txt(slide, sub, 0.58, 1.22, 7.9, 0.28, { fontSize: 10.5, color: C.muted });
}

function footer(slide, n) {
  slide.addShape(pptx.ShapeType.line, { x: 0.55, y: 7.05, w: 12.2, h: 0, line: { color: C.line, transparency: 25, width: 0.7 } });
  txt(slide, "Fuxi 毕业论文答辩 · 2026-05-09", 0.55, 7.12, 4.2, 0.18, { fontSize: 7.5, color: C.muted });
  txt(slide, String(n).padStart(2, "0"), 12.35, 7.08, 0.42, 0.25, { fontSize: 8, color: C.soft, align: "right" });
}

function addImageContain(slide, file, x, y, w, h, border = true) {
  slide.addImage({ path: file, x, y, w, h, sizing: { type: "contain", x, y, w, h } });
  if (border) {
    slide.addShape(pptx.ShapeType.rect, {
      x, y, w, h,
      fill: { color: "FFFFFF", transparency: 100 },
      line: { color: C.line, transparency: 20, width: 0.8 },
      radius: 0.06
    });
  }
}

function pill(slide, text, x, y, w, color = C.teal) {
  slide.addShape(pptx.ShapeType.roundRect, {
    x, y, w, h: 0.34,
    rectRadius: 0.08,
    fill: { color, transparency: 84 },
    line: { color, transparency: 25, width: 0.9 }
  });
  txt(slide, text, x + 0.1, y + 0.075, w - 0.2, 0.14, { fontSize: 8.8, bold: true, color });
}

function metric(slide, value, label, x, y, w, accent = C.copper) {
  slide.addShape(pptx.ShapeType.roundRect, {
    x, y, w, h: 1.1,
    rectRadius: 0.08,
    fill: { color: C.panel, transparency: 0 },
    line: { color: accent, transparency: 35, width: 1.2 }
  });
  txt(slide, value, x + 0.18, y + 0.16, w - 0.36, 0.34, { fontSize: 24, bold: true, color: accent });
  txt(slide, label, x + 0.18, y + 0.62, w - 0.36, 0.32, { fontSize: 9.5, color: C.soft, breakLine: true });
}

function bullets(slide, items, x, y, w, h, opts = {}) {
  const runs = [];
  for (const item of items) {
    runs.push({ text: item, options: { bullet: { indent: 14 }, hanging: 4, breakLine: true } });
  }
  slide.addText(runs, {
    x, y, w, h,
    margin: 0.03,
    fontFace: "PingFang SC",
    fontSize: opts.fontSize ?? 13,
    color: opts.color ?? C.soft,
    breakLine: false,
    paraSpaceAfterPt: opts.paraSpaceAfterPt ?? 7,
    fit: "shrink"
  });
}

function quote(slide, text, x, y, w, h) {
  slide.addShape(pptx.ShapeType.rect, { x, y, w: 0.045, h, fill: { color: C.copper }, line: { color: C.copper } });
  txt(slide, text, x + 0.18, y, w - 0.18, h, { fontSize: 15, bold: true, color: C.white, breakLine: true, valign: "mid" });
}

function notes(slide, arr) {
  slide.addNotes(arr.join("\n"));
}

let s, n = 1;

s = pptx.addSlide(); addBg(s);
s.addImage({ path: path.join(ASSETS, "fuxi-defense-hero.png"), x: 0, y: 0, w: 13.333, h: 7.5 });
s.addShape(pptx.ShapeType.rect, { x: 0, y: 0, w: 6.25, h: 7.5, fill: { color: "07111F", transparency: 5 }, line: { color: "07111F", transparency: 100 } });
txt(s, "毕业论文答辩", 0.62, 0.72, 2.2, 0.24, { fontSize: 10, color: C.teal, bold: true, charSpace: 2 });
txt(s, "基于 AI Agent 的\n高性能分布式通讯系统", 0.62, 1.2, 5.15, 1.45, { fontSize: 32, bold: true, color: C.white, breakLine: true, fit: "shrink" });
quote(s, "把多 Agent 系统从“演示脚本”推到“可观测、可回放、可审计的日用平台”。", 0.66, 3.15, 4.72, 0.84);
txt(s, "答辩人：以琳\n日期：2026 年 5 月 9 日", 0.66, 6.28, 3.5, 0.46, { fontSize: 12, color: C.soft, breakLine: true });
notes(s, [
  "开场：一句话说明论文做的是 Fuxi，一个 Rust 本地优先多 Agent 编排平台。",
  "重点不要说“又写了一个框架”，而是强调通讯底座：事件、协议、隔离、观测。"
]);

s = pptx.addSlide(); addBg(s); title(s, "01 · PROBLEM", "问题：多 Agent 真正难在通讯底座", "角色协作本身已经被证明有效，工程化短板出在状态、通讯和可观测性。"); footer(s, n++);
["通讯协议不统一", "任务生命周期缺位", "运行时状态难追踪"].forEach((t, i) => {
  const x = 0.75 + i * 4.15;
  slideBlock(s, t, [
    i === 0 ? "Python 对象互调难跨进程、跨节点" : i === 1 ? "Agent 执行长、会中断、会等待人" : "工具调用、子任务、异常必须可回放",
    i === 0 ? "A2A 提供语义，但要和本地事件系统结合" : i === 1 ? "需要显式 Task 状态与人工介入语义" : "事件流必须成为一等公民"
  ], x, 2.0, 3.45, [C.teal, C.copper, C.violet][i]);
});
quote(s, "本文的问题不是“让 Agent 会聊天”，而是让多个 Agent 在本机与跨节点环境中可靠协作。", 1.3, 5.45, 10.6, 0.54);
notes(s, ["本页把研究问题立住：多 Agent 框架很多，但本地高性能、可审计通讯底座不够。", "自然过渡到下一页：本文目标就是把这个底座做成系统。"]);

s = pptx.addSlide(); addBg(s); title(s, "02 · TARGET", "研究目标与设计假设", "Rust、本地优先、事件驱动、可跨节点扩展。"); footer(s, n++);
quote(s, "在 Rust 工作区中实现一个本地优先、事件驱动、可观测、可跨节点扩展的多 AI Agent 协作平台。", 0.75, 1.65, 11.55, 0.62);
metric(s, "13", "crate 模块边界", 0.8, 3.0, 2.1, C.teal);
metric(s, "8.25 万", "约 Rust 代码行", 3.18, 3.0, 2.1, C.copper);
metric(s, "1363 + 27", "单元测试 + 集成测试", 5.56, 3.0, 2.35, C.green);
metric(s, "ERP", "真实项目日常验证", 8.18, 3.0, 2.1, C.violet);
metric(s, "5", "可独立审视的工程贡献", 10.56, 3.0, 2.0, C.copper);
txt(s, "设计假设", 0.8, 5.05, 1.25, 0.25, { fontSize: 12, bold: true, color: C.teal });
bullets(s, [
  "事件驱动 + 追加式日志，可以同时兼顾吞吐、实时性与可追溯性。",
  "协议语义、事实流、调度边界、执行隔离必须同时成立，少一个就无法日用。"
], 0.82, 5.45, 10.8, 0.78, { fontSize: 12.5 });
notes(s, ["这一页讲目标和规模，避免一上来钻代码。", "数字口径来自论文摘要与总结。"]);

s = pptx.addSlide(); addBg(s); title(s, "03 · ARCHITECTURE", "总体架构：核心—通讯—编排—执行—观测", "A2A 是契约语义，EventBus 是事实流；两者正交。"); footer(s, n++);
addImageContain(s, path.join(FIG, "drawio/fig-2-1-overall-architecture.png"), 0.78, 1.55, 7.45, 4.95);
txt(s, "讲图顺序", 8.62, 1.58, 1.5, 0.24, { fontSize: 12, bold: true, color: C.teal });
bullets(s, [
  "底层：fuxi-core 定义 Agent / Task / Workspace / Event。",
  "通讯：fuxi-events 记录事实，fuxi-a2a 承担 Agent 间契约。",
  "编排：玄女维护门客注册表，所有协作经编排层显式记录。",
  "执行：Claude Code / Codex 门客适配 + git worktree 沙箱。",
  "观测：Firehose、WebSocket、SSE、IM 共看同一事件流。"
], 8.64, 2.0, 3.85, 3.75, { fontSize: 11.2, paraSpaceAfterPt: 7 });
pill(s, "核心观点：协议不保存事实，事件流不替代协议", 8.62, 6.05, 3.78, C.copper);
notes(s, ["答辩时用这一页解释系统全貌。", "强调依赖方向自下而上，A2A 和 EventBus 的边界是论文差异点。"]);

s = pptx.addSlide(); addBg(s); title(s, "04 · CONTRIBUTIONS", "五项贡献：围绕一个命题展开", "把 Agent 间通讯的契约、事实流、调度边界与执行隔离同时做对。"); footer(s, n++);
const cons = [
  ["A2A 风格协议适配层", "五条核心路径；input-required 提升为平台级人工介入"],
  ["非阻塞事件总线", "Tokio broadcast + SQLite WAL；try_send + 后台转交 + lag 哨兵"],
  ["玄女—门客分层", "用户入口与执行 Agent 解耦；编排层不可绕过"],
  ["三层沙箱 × WS 反连", "L1/L2/L3 worktree 隔离；编排层可主动控制门客"],
  ["跨节点扩展钩子", "DistEnqueuer / NodeLoadProvider 等 trait 注入；HMAC 验签"]
];
cons.forEach((c, i) => {
  const y = 1.55 + i * 0.95;
  txt(s, `0${i + 1}`, 0.82, y + 0.02, 0.42, 0.28, { fontSize: 12, bold: true, color: [C.teal, C.copper, C.green, C.violet, C.teal][i], align: "center" });
  s.addShape(pptx.ShapeType.line, { x: 1.48, y: y + 0.18, w: 0.75, h: 0, line: { color: C.line, width: 1 } });
  txt(s, c[0], 2.45, y, 3.05, 0.34, { fontSize: 15.5, bold: true, color: C.white });
  txt(s, c[1], 5.6, y + 0.02, 6.35, 0.34, { fontSize: 11.2, color: C.soft });
});
quote(s, "每一项贡献都能被单独评审，但它们最终服务于同一个通讯系统闭环。", 1.25, 6.45, 10.2, 0.42);
notes(s, ["本页是答辩主目录。", "后面几页不用把每项都讲成源码讲解，只讲最有区分度的三四个机制。"]);

s = pptx.addSlide(); addBg(s); title(s, "05 · A2A", "A2A 适配：协议语义进入平台状态机", "`input-required` 不只是 wire 字段，而是可观测、可调度的人工介入状态。"); footer(s, n++);
addImageContain(s, path.join(IMG, "代码3-3-TaskState枚举.png"), 0.82, 1.55, 5.65, 1.85);
addImageContain(s, path.join(IMG, "算法3-1-SSE帧解析.png"), 0.82, 3.75, 5.65, 2.2);
txt(s, "实现闭环", 7.05, 1.62, 1.2, 0.22, { fontSize: 12, bold: true, color: C.teal });
bullets(s, [
  "覆盖 agent discovery、task send、task stream、task query、task cancel 五条主路径。",
  "沿用早期 A2A JSON-RPC binding；论文中明确说明与当前官方 v1.0 尚未完全对齐。",
  "将 TASK_STATE_INPUT_REQUIRED 映射到 Task::PendingApproval 与 ShelfStatus::AwaitingInput。",
  "采用单端点 HTTP + SSE，降低客户端连接管理复杂度。"
], 7.05, 2.0, 5.15, 2.2, { fontSize: 11.4 });
metric(s, "1321 行", "fuxi-a2a crate：核心 1063 + 测试 258", 7.08, 4.78, 2.45, C.copper);
metric(s, "5 路径", "发现、发送、流式订阅、查询、取消", 9.83, 4.78, 2.45, C.teal);
notes(s, ["老师可能会问：既然已有 a2a-rs，为什么还做？回答：不是生态空白，而是与本地编排、事件总线、人工介入语义深度集成。", "主动承认：当前是早期 binding，不主张完整 v1.0 兼容。"]);

s = pptx.addSlide(); addBg(s); title(s, "06 · EVENT BUS", "事件总线：实时推送与历史回放共用一套抽象", "publish 路径不阻塞调用方，同时让背压变成可见告警。"); footer(s, n++);
addImageContain(s, path.join(IMG, "代码3-1-EventBus非阻塞publish.png"), 0.75, 1.52, 6.15, 4.72);
txt(s, "三步组合", 7.35, 1.52, 1.4, 0.25, { fontSize: 12, bold: true, color: C.teal });
[
  ["1", "broadcast", "零拷贝扇出给实时订阅者"],
  ["2", "try_send", "事件投递给异步 writer 落 SQLite WAL"],
  ["3", "lag sentinel", "队列超过阈值 512 时发出可观测告警"]
].forEach((r, i) => {
  const y = 2.0 + i * 1.05;
  s.addShape(pptx.ShapeType.roundRect, { x: 7.36, y, w: 4.95, h: 0.72, rectRadius: 0.08, fill: { color: C.panel }, line: { color: C.line, width: 0.8 } });
  txt(s, r[0], 7.55, y + 0.17, 0.32, 0.2, { fontSize: 13, bold: true, color: C.copper, align: "center" });
  txt(s, r[1], 8.05, y + 0.12, 1.25, 0.22, { fontSize: 11.5, bold: true, color: C.white });
  txt(s, r[2], 9.2, y + 0.14, 2.85, 0.2, { fontSize: 9.5, color: C.soft });
});
quote(s, "关键取舍：宁可暴露 lag，也不让调用方在 publish() 上被持久化路径拖住。", 7.36, 5.65, 4.9, 0.54);
notes(s, ["这一页讲清楚非阻塞事件总线。", "如果被问是否丢消息：说明持久化路径拥塞时后台等待，lag sentinel 负责让背压可见；广播路径只负责实时观测。"]);

s = pptx.addSlide(); addBg(s); title(s, "07 · ORCHESTRATION", "玄女—门客：编排层不可绕过", "顶层 Agent 负责对话与调度，工作 Agent 负责执行；所有协作都留下事件。"); footer(s, n++);
addImageContain(s, path.join(FIG, "drawio/fig-3-1-execution-loop.png"), 0.78, 1.45, 6.2, 4.85);
txt(s, "工程边界", 7.35, 1.5, 1.4, 0.24, { fontSize: 12, bold: true, color: C.teal });
bullets(s, [
  "玄女是用户对话面唯一入口；门客之间不直接绕过编排层。",
  "publish-then-pump：门客私有事件补齐 task_id / agent_id 后再进入全局 EventBus。",
  "claim_idle_by_role 原子化避免并发派活撞同一门客。",
  "shutdown_agent 对玄女做硬豁免，避免 idle GC 误杀顶层调度 Agent。"
], 7.35, 1.93, 4.85, 2.1, { fontSize: 11.2 });
addImageContain(s, path.join(IMG, "代码3-4-claim_idle原子化.png"), 7.35, 4.4, 4.95, 1.12);
pill(s, "答辩句：编排层既不是聊天路由，也不是业务逻辑，而是事实流的边界。", 7.35, 6.0, 4.96, C.copper);
notes(s, ["这一页解释为什么叫玄女—门客。", "避免讲成拟人设定，重点是架构分层和事件可审计。"]);

s = pptx.addSlide(); addBg(s); title(s, "08 · EXECUTION", "执行隔离：三层沙箱 × WebSocket 反连", "把 headless Agent CLI 接进平台，而不是把平台绑死在某一个 CLI 上。"); footer(s, n++);
addImageContain(s, path.join(IMG, "代码3-5-CcAgent反连握手.png"), 0.78, 1.45, 5.62, 2.1);
addImageContain(s, path.join(IMG, "附录代码A-10-L2_L3沙箱差异.png"), 0.78, 3.86, 5.62, 2.05);
[
  ["L1", "只读视图", "轻量查询，不创建任务 worktree"],
  ["L2", "任务级 worktree", "并发修改互不干扰，结束后合并"],
  ["L3", "角色级持久沙箱", "复用构建缓存，适合长期角色"]
].forEach((r, i) => {
  const y = 1.62 + i * 1.12;
  metric(s, r[0], `${r[1]}\n${r[2]}`, 7.0, y, 2.08, [C.teal, C.copper, C.green][i]);
});
quote(s, "WS 反连让编排层能主动 send_message / cancel，打破传统 stdio 只能等门客 poll 的限制。", 9.35, 2.05, 2.85, 1.78);
notes(s, ["这一页是工程亮点：多 Agent 并发为什么不会互相踩文件。", "说明支持 Claude Code 和 Codex 两套差异很大的 headless CLI。"]);

s = pptx.addSlide(); addBg(s); title(s, "09 · CROSS NODE", "跨节点扩展：单机优先，扩展点钩子化", "默认单节点完整运行；多节点能力通过 trait 注入，不污染核心编排层。"); footer(s, n++);
addImageContain(s, path.join(FIG, "drawio/fig-2-3-cross-node.png"), 0.74, 1.42, 5.45, 4.75);
txt(s, "关键机制", 6.75, 1.5, 1.4, 0.24, { fontSize: 12, bold: true, color: C.teal });
bullets(s, [
  "Project.host_nodes 标记可执行节点。",
  "auto_pin_from_project 尊重用户显式 @node，自动逻辑不得覆盖显式意图。",
  "节点选择按 inflight / max_concurrency 的饱和度最小化。",
  "HMAC-SHA256 + nonce cache + 常量时间比较，覆盖跨节点请求验签。"
], 6.76, 1.92, 5.65, 1.85, { fontSize: 11.5 });
addImageContain(s, path.join(IMG, "算法3-3-HMAC验签.png"), 6.78, 4.2, 5.52, 1.55);
pill(s, "当前边界：已跑通两节点 sandbox；10+ 节点与不可靠链路故障转移是后续工作。", 6.78, 6.2, 5.46, C.copper);
notes(s, ["这页不要夸大。强调单机优先，跨节点是已验证扩展点。", "主动说局限：更大规模和不可靠链路留待后续。"]);

s = pptx.addSlide(); addBg(s); title(s, "10 · EVALUATION", "实验设计：先隔离 LLM，再测通讯层上限", "合成基准测的是编排与通讯路径，不把模型推理时间混进系统指标。"); footer(s, n++);
addImageContain(s, path.join(IMG, "表4-1-测试覆盖.png"), 0.75, 1.46, 5.6, 1.95);
addImageContain(s, path.join(IMG, "表4-2-吞吐基线.png"), 0.75, 3.82, 5.6, 1.78);
txt(s, "方法学", 6.92, 1.52, 1.15, 0.24, { fontSize: 12, bold: true, color: C.teal });
bullets(s, [
  "吞吐实验：每任务以 10 ms sleep 模拟，剥离 LLM 推理。",
  "吞吐采用 5-run median；延迟采用 500 样本统计 p50 / p99。",
  "覆盖正确性、实时性、吞吐扩展性、参数敏感性、事件总线压力。",
  "真实 LLM Agent 的端到端吞吐主要受单次推理 1–30 s 主导。"
], 6.94, 1.95, 5.25, 2.05, { fontSize: 11.5 });
metric(s, "cargo test", "workspace all-targets 通过", 6.95, 4.55, 2.25, C.green);
metric(s, "500", "延迟样本统计", 9.5, 4.55, 2.25, C.copper);
notes(s, ["本页帮你防御“为什么任务只有 10ms sleep”的问题。", "回答：这是为了测通讯/编排层上限，真实模型推理另算。"]);

s = pptx.addSlide(); addBg(s); title(s, "11 · RESULTS", "结果一：Worker 扩展性没有在 16 worker 前饱和", "8 worker 达 665 tasks/s；16 worker 达 1288.24 tasks/s。"); footer(s, n++);
addImageContain(s, path.join(FIG, "matplotlib/fig-5-1-scalability.png"), 0.7, 1.46, 7.15, 3.05);
addImageContain(s, path.join(IMG, "表4-3-Worker扩展性.png"), 0.7, 4.9, 7.15, 1.22);
metric(s, "665.00", "8 worker × 10 ms 合成任务\n调度路径吞吐 tasks/s", 8.32, 1.58, 2.25, C.copper);
metric(s, "1288.24", "16 worker 配置吞吐 tasks/s", 10.83, 1.58, 1.82, C.teal);
metric(s, "80.5–83.1%", "1→16 worker 扩展效率区间", 8.32, 3.0, 2.25, C.green);
metric(s, "未饱和", "dispatcher 在该区间不是主瓶颈", 10.83, 3.0, 1.82, C.violet);
quote(s, "结论：通讯与调度路径在目标规模内有足够余量，瓶颈主要会转移到真实 LLM 推理。", 8.32, 5.18, 4.32, 0.78);
notes(s, ["用数字讲，不要泛泛说高性能。", "强调这些不是端到端 LLM 吞吐，是剥离推理后的通讯层上限。"]);

s = pptx.addSlide(); addBg(s); title(s, "12 · RESULTS", "结果二：事件流延迟比派发链路低三个数量级", "观测端应当跟得上任务过程，而不是靠轮询追尾。"); footer(s, n++);
addImageContain(s, path.join(FIG, "matplotlib/fig-5-6-e2e-breakdown.png"), 0.68, 1.45, 6.55, 2.55);
addImageContain(s, path.join(FIG, "matplotlib/fig-5-5-event-flow-latency.png"), 0.68, 4.42, 3.1, 1.85);
addImageContain(s, path.join(FIG, "matplotlib/fig-5-4-dispatch-latency.png"), 4.12, 4.42, 3.1, 1.85);
metric(s, "32.93 ms", "任务派发 p50\n含 HTTP / SQLite / 反序列化等", 7.72, 1.6, 2.25, C.copper);
metric(s, "0.08 ms", "事件总线广播 p50", 10.25, 1.6, 2.25, C.teal);
metric(s, "0.26 ms", "事件总线广播 p99", 7.72, 3.02, 2.25, C.green);
metric(s, "410×", "event_flow p50 与 dispatch p50 差距", 10.25, 3.02, 2.25, C.violet);
quote(s, "通讯层低延迟的意义：让 Firehose、TUI、PWA 能近实时显示过程事件。", 7.72, 5.22, 4.78, 0.58);
notes(s, ["讲清楚 p50/p99 的含义。", "重点：事件流很快，所以观测端不需要轮询；dispatch 也在可接受范围。"]);

s = pptx.addSlide(); addBg(s); title(s, "13 · RESULTS", "结果三：事件总线压力测试给出容量边界", "64 subscriber × 10k events/s 零丢帧；16 subscriber × 100k events/s 可持续。"); footer(s, n++);
addImageContain(s, path.join(FIG, "matplotlib/fig-5-3-bus-stress.png"), 0.72, 1.46, 7.25, 3.05);
addImageContain(s, path.join(IMG, "表4-6-事件总线压测.png"), 0.72, 4.9, 7.25, 1.2);
metric(s, "64 × 10k/s", "持续负载下零丢帧", 8.45, 1.62, 2.25, C.teal);
metric(s, "1.6M events/s", "16 sub × 100k/s sustainable 边界", 8.45, 3.04, 2.25, C.copper);
metric(s, "3 个数量级", "高于实际工作负载估算余量", 8.45, 4.46, 2.25, C.green);
txt(s, "payload size 影响很小：small 256B 与 large 4096B 在零丢帧区间 p50/p99 差异不超过 0.1us，符合 Arc clone 扇出模型预期。", 10.95, 1.72, 1.42, 3.7, { fontSize: 10.5, color: C.soft, breakLine: true, valign: "mid" });
notes(s, ["这里回答系统容量问题。", "64x100k/s 出现丢帧是边界，不要隐去；主结论是实际负载远低于 sustainable 边界。"]);

s = pptx.addSlide(); addBg(s); title(s, "14 · LIMITS", "局限与后续工作", "答辩里主动承认边界，比被问住更好。"); footer(s, n++);
const limits = [
  ["A2A 规范跟进", "当前实现沿用早期 JSON-RPC binding；后续补齐 v1.0 PascalCase 与 one-of 互通测试。"],
  ["安全模型", "已有 worktree sandbox 与跨节点 HMAC；工具授权、数据访问控制、提示注入仍需统一策略。"],
  ["分布式一致性", "两节点 sandbox 已跑通；更大节点数、不可靠链路与故障转移尚未充分验证。"],
  ["Agent 适配生态", "已接 Claude Code / Codex；后续抽象更通用的 Agent CLI 适配框架。"]
];
limits.forEach((r, i) => {
  const x = 0.8 + (i % 2) * 6.15;
  const y = 1.62 + Math.floor(i / 2) * 2.05;
  slideBlock(s, r[0], [r[1]], x, y, 5.45, [C.copper, C.teal, C.green, C.violet][i], 1.42);
});
quote(s, "路线没有因答辩结束而停止：论文记录的是 Fuxi v1/v2 阶段的工程化证据。", 1.2, 6.22, 10.8, 0.48);
notes(s, ["这一页要坦诚。", "局限说完马上接后续路径，显示你知道边界在哪里。"]);

s = pptx.addSlide(); addBg(s);
txt(s, "Q&A", 0.75, 0.75, 2.25, 0.55, { fontSize: 34, bold: true, color: C.white });
txt(s, "谢谢各位老师", 0.78, 1.42, 2.6, 0.32, { fontSize: 15, color: C.teal, bold: true });
quote(s, "Fuxi 的差异化不在于“最快的 Agent 框架”，而在于把通讯底座从应用流程中显式剥离出来：事件流可观测可回放，协议层可独立部署，执行层可隔离扩展。", 0.8, 2.55, 5.2, 1.5);
addImageContain(s, path.join(FIG, "drawio/fig-1-1-overview.png"), 6.55, 1.25, 5.9, 4.95);
footer(s, n++);
notes(s, ["结束句：感谢老师，欢迎批评指正。", "如果追问创新点，回到五项贡献；如果追问性能，回到三页结果。"]);

function slideBlock(slide, heading, body, x, y, w, accent, h = 1.72) {
  slide.addShape(pptx.ShapeType.roundRect, {
    x, y, w, h,
    rectRadius: 0.08,
    fill: { color: C.panel, transparency: 0 },
    line: { color: accent, transparency: 35, width: 1.1 }
  });
  slide.addShape(pptx.ShapeType.rect, {
    x, y, w: 0.08, h,
    fill: { color: accent },
    line: { color: accent }
  });
  txt(slide, heading, x + 0.28, y + 0.22, w - 0.5, 0.28, { fontSize: 15, bold: true, color: C.white });
  txt(slide, body.join("\n"), x + 0.28, y + 0.68, w - 0.52, h - 0.82, { fontSize: 10.4, color: C.soft, breakLine: true, valign: "top" });
}

pptx.writeFile({ fileName: path.join(OUT, "fuxi-thesis-defense-2026-05-09.pptx") });

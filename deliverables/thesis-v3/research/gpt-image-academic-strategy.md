# gpt-image-2 学术图使用策略（v3）

## 背景

用户反馈 v2 fig-1-1（玄女门客萌系水墨）方向走偏，要求 gpt-image-2 服务**学术化** figure 而非 cute concept art。

调研报告 figure-design-conventions.md 没覆盖 gpt-image-2 用法，本文补全。

## gpt-image-2 vs draw.io vs matplotlib 三者分工

| 工具 | 强项 | 弱项 | 适合 figure 类型 |
|---|---|---|---|
| **matplotlib** | 数据驱动、可复现、PDF vector | 抽象图能力差 | 实验图（CDF / scatter / bar / breakdown） |
| **draw.io / TikZ** | 结构图清晰、可编辑、统一风格 | 视觉冲击力弱 | 模块图、状态机、依赖 DAG、序列图 |
| **gpt-image-2** | **视觉表达力强、立体感、illustration** | 文字易乱码、不可复现、不严格 | **概念示意图**、hero 图、抽象隐喻图 |

## v3 中 gpt-image-2 的合理用法

### ✅ 该用 gpt-image-2 的位置（学术风格）

按 Aegaeon / OSDI / SIGCOMM 论文实例，"learning systems / agent" 类论文常在以下位置用 illustrative figure：

1. **第 1 章 motivation / overview hero**：替代当前 fig-1-1。
   - 风格：扁平 isometric / vector / 工业设计感
   - 内容：自治 agent + 工具调用 + 通信箭头组成的概念图
   - 例：DeepMind/OpenAI 论文常见的「层级架构 + 数据流 + 立体小图标」组合
   - 参照：DeepMind Gato / Anthropic Claude blog 的 hero illustrations

2. **第 6 章 future work hero**（可选）：分布式 fuxi 集群俯视图。
   - 风格：科技感 isometric topology
   - 内容：多节点世界地图 + 数据流光线 + agent icon

### ❌ 不该用 gpt-image-2 的位置

- 系统架构图（fig 2.1, 3.x, 4.x）→ draw.io / TikZ（必须可编辑、文字必须准确）
- 实验数据图（fig 5.x）→ matplotlib（必须可复现、数据必须准确）
- 状态机 / 时序图 → draw.io / TikZ
- 任何带具体技术术语 label 的图 → 不用（gpt-image-2 文字基本会乱）

## v3 fig-1-1 重做 prompt 模板

废弃 v2 萌系水墨 prompt，改为如下学术 illustration 风格：

```
Editorial-style flat vector illustration for a top-tier systems research paper
(NeurIPS / SOSP aesthetic). Subject: a hierarchical multi-agent system.

Composition: isometric perspective, central orchestrator agent at top depicted
as an abstract geometric figure (no human face), surrounded by 4-5 worker
agents below as smaller geometric shapes. Light beams or data-flow lines
connect them, showing message passing and task dispatch.

Style: minimalist, clean, professional. Off-white background. Limited palette
of soft blue, teal, soft yellow, and gray (3-4 colors max). Subtle gradients
allowed but no 3D shadows. Vector-quality lines.

NO text labels. NO cute / kawaii style. NO faces or characters. Strictly
abstract geometric agents.

Aspect ratio: 16:9. Suitable for a paper figure caption "Architectural
overview of the Fuxi multi-agent communication platform".
```

## 风险与缓解

- **风险**：gpt-image-2 仍可能加文字（即使 prompt 说 NO text）
  - 缓解：生成后用 Photoshop / Pixelmator 擦除残留文字
- **风险**：风格与论文整体不统一
  - 缓解：限制只用 1-2 张 hero 图，其他图用 draw.io / matplotlib
- **风险**：评委觉得 generative 图不严肃
  - 缓解：放在 motivation / future work 而非核心论证段，且必须有 caption 解释「示意图，仅作概念辅助」

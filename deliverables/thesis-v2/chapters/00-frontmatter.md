<!--
本文件包含：封面 / 扉页 / 诚信声明 / 中文摘要 / 英文摘要 / 目录占位
学校格式约束（用户用 Word 手调）：封面、扉页、摘要、目录不编页码

mustache 占位符 {{...}} 用户最后手填：
- {{学号}}
- {{班级}}
- {{专业}}（建议：软件工程 / 计算机科学与技术）
- {{所在学院}}（建议：信息科学与工程学院）
- {{指导教师}}
- {{完成日期}}
- {{答辩之后日期}}

摘要里的实验数字占位符（待 bench-worker 完成后由 Phase 2 写第 5 章时填）：
- {{TPS_8W_10MS}}     · 8 worker × 10ms 任务的吞吐
- {{TPS_16W_10MS}}    · scalability 实验 16 worker 上限
- {{P50_TASK_DISP}}   · 任务派发 p50 延迟（ms）
- {{P99_EVENT_FLOW}}  · 跨节点事件流 p99 延迟（ms）
- {{TPS_BUS_64SUB}}   · 事件总线压测 64 subscriber 时 publish_tps
-->

\newpage

::: {.cover-page}

**毕业论文（设计）**

\vspace{2cm}

**题目：基于 AI Agent 的高性能分布式通讯系统**

\vspace{3cm}

学生姓名：以琳

学    号：{{学号}}

班    级：{{班级}}

专    业：{{专业}}

指导教师：{{指导教师}}

\vspace{4cm}

**2026 年 5 月**

:::

\newpage

::: {.title-page}

**基于 AI Agent 的高性能分布式通讯系统**

\vspace{2cm}

学生姓名：以琳

学     号：{{学号}}

班     级：{{班级}}

所在学院：{{所在学院}}

指导教师：{{指导教师}}

完成日期：{{完成日期}}

:::

\newpage

# 诚信声明

本人声明：

1、本人所呈交的毕业设计（论文）是在老师指导下进行的研究（设计）工作及取得的研究（设计）成果；

2、据查证，除了文中特别加以标注和致谢的地方外，毕业设计（论文）中不包含其他人已经公开发表过的研究成果，也不包含为获得其他教育机构的学位而使用过材料；

3、我承诺，本人提交的毕业设计（论文）中的所有内容均真实、可信。

\vspace{3cm}

作者签名：{{电子签名图片}}        日期：{{答辩之后日期}}

\newpage

# 摘要

随着大语言模型与工具调用技术的发展，AI Agent 已从单轮问答工具逐步演进为能够理解目标、分解任务、调用外部工具并持续反馈的自治执行单元。当多个 Agent 共同工作时，系统不仅要解决模型能力问题，还须处理任务编排、跨进程通信、状态一致性、执行隔离、实时可观测性与故障恢复等工程问题。现有多智能体框架多侧重角色协作与提示词流程，在本地化、高性能、可审计的分布式通信底座方面仍有较大优化空间。

针对上述问题，本文设计并实现一套名为 Fuxi 的 Rust 平台。系统采用「玄女—门客」分层协作模式，将面向用户的顶层 Agent 与执行任务的工作 Agent 解耦；以 A2A 语义统一描述 Agent 间任务、消息与能力发现；以 Tokio broadcast 与 SQLite WAL 构建实时事件总线，使实时推送与历史回放并存；以 Firehose、WebSocket、SSE 与 IM API 提供多端观测；以 Git worktree 与分层 sandbox 保证任务执行隔离；并提供长期记忆、定时触发、角色加载与跨节点任务调度等支撑模块。在通信层进一步给出端到端延迟分解、广播事件延迟下界与 Worker 选择函数的形式化定义，为系统性能分析提供可度量基础。

实验环节在 Apple Silicon 平台上对吞吐量、延迟、参数敏感性与事件总线压力进行了多维测量。结果表明，Fuxi 在 8 worker、10 ms 模拟任务条件下达到 {{TPS_8W_10MS}} tasks/s 的吞吐量，且 Worker 数量扩展至 16 时吞吐保持近线性增长；任务派发 p50 延迟为 {{P50_TASK_DISP}} ms，跨节点事件流 p99 延迟低至 {{P99_EVENT_FLOW}} ms；事件总线在 64 subscriber 并发订阅下仍可维持 {{TPS_BUS_64SUB}} events/s 的发布吞吐。上述数据说明，事件驱动与追加式日志结合的通信架构能够在本地优先的 AI Agent 协作场景中兼顾吞吐、实时性与可追溯性，为后续多 Agent 系统的工程化提供了一套可借鉴的基础设施。

**关键词**：AI Agent；分布式通信；多智能体系统；事件总线；A2A 协议

\newpage

# ABSTRACT

With the rapid progress of large language models and tool-use techniques, AI agents are evolving from single-turn assistants into autonomous execution units that understand goals, decompose tasks, invoke external tools and continuously report progress. When multiple agents collaborate, a platform must address not only model capability, but also task orchestration, inter-process communication, state consistency, execution isolation, real-time observability and fault recovery. Existing multi-agent frameworks primarily focus on role assignment and prompt workflows, leaving room for improvement in local-first, high-performance and auditable distributed communication infrastructure.

This thesis designs and implements Fuxi, a Rust-based platform that addresses these gaps. Fuxi adopts a hierarchical Xuannv-Menke collaboration pattern, separating the user-facing orchestrator agent from worker agents that execute concrete tasks. It unifies inter-agent task, message and capability semantics through an A2A-style protocol; builds a real-time event bus on top of Tokio broadcast and SQLite WAL so that live subscription and historical replay coexist; exposes observability through Firehose, WebSocket, SSE and IM APIs; and isolates task execution through Git worktrees and layered sandboxes. The system further supports persistent memory, scheduled triggers, role loading and cross-node task dispatch. End-to-end latency decomposition, broadcast event latency lower bounds and a worker selection function are formalized to make performance analysis quantifiable.

Experimental evaluation on an Apple Silicon platform measures throughput, latency, parameter sensitivity and event bus stress along multiple dimensions. Fuxi sustains {{TPS_8W_10MS}} tasks per second with 8 workers under 10 ms simulated jobs, and throughput scales near-linearly up to 16 workers. The p50 task dispatch latency is {{P50_TASK_DISP}} ms and the p99 cross-node event flow latency is as low as {{P99_EVENT_FLOW}} ms. Even with 64 concurrent subscribers, the event bus retains {{TPS_BUS_64SUB}} events per second of publish throughput. These results demonstrate that combining event-driven semantics with append-only logging delivers a communication substrate that simultaneously achieves throughput, real-time responsiveness and traceability for local-first AI agent collaboration, providing a reusable foundation for future multi-agent system engineering.

**Key words**: AI Agent; Distributed Communication; Multi-Agent System; Event Bus; A2A Protocol

\newpage

# 目录

[此处由 Word 自动生成目录，pandoc 输出后请手动插入「引用 → 目录」三级目录]

\newpage

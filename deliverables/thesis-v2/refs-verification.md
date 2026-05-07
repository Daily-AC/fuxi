# 参考文献验证证据链

> 30 篇逐一验证记录。每篇含：引用键、URL、search 证据、引用位置预估。
> 验证日期：2026-05-07
> 验证方式：WebSearch / WebFetch / curl arxiv abs。

## A 桶：多智能体理论基础（4 篇）

### A.1 [wooldridge2009mas]
- 标题：An Introduction to MultiAgent Systems (2nd ed.)
- 作者：Michael Wooldridge
- 出版：John Wiley & Sons, 2009 年 5 月
- ISBN-13：978-0470519462；ISBN-10：0470519460
- 验证 URL（出版社官页）：https://www.wiley.com/en-us/An+Introduction+to+MultiAgent+Systems,+2nd+Edition-p-9780470519462
- 验证 URL（作者官页）：https://www.cs.ox.ac.uk/people/michael.wooldridge/pubs/imas/IMAS2e.html
- 验证证据：Wiley 官页与 Wooldridge 个人主页（Oxford CS）均列出 2nd Edition、Published May 2009、ISBN-10 0470519460、ISBN-13 978-0470519462。ACM Digital Library 也收录该书 (dl.acm.org/doi/10.5555/1695886)。
- 引用位置预估：§1.2 国内外现状（多智能体定义）；§2.1 多智能体理论基础

### A.2 [stone2000multiagent]
- 标题：Multiagent Systems: A Survey from a Machine Learning Perspective
- 作者：Peter Stone, Manuela Veloso
- 出版：Autonomous Robots, Vol. 8, No. 3, pp. 345-383, July 2000
- DOI：10.1023/A:1008942012299
- 验证 URL（Springer 官页）：https://link.springer.com/article/10.1023/A:1008942012299
- 验证 URL（CMU 作者主页 PDF）：https://www.cs.cmu.edu/~mmv/papers/MASsurvey.pdf
- 验证证据：Springer Nature Link、ACM DL（dl.acm.org/doi/10.1023/A:1008942012299）、Semantic Scholar 三处一致；卷期页码与候选清单完全对齐。论文将 DAI 划分为 DPS 与 MAS 两类，并以机器人足球为典型测试床——这是 §2.1 要引的核心观点。
- 引用位置预估：§1.2 国内外现状；§2.1 多智能体协作模型

### A.3 [russell2020aima]
- 标题：Artificial Intelligence: A Modern Approach (4th ed.)
- 作者：Stuart J. Russell, Peter Norvig
- 出版：Pearson, 2020
- ISBN-13：978-0134610993（精装版）；eTextbook 9780137505135
- 验证 URL（Pearson 官页）：https://www.pearson.com/en-us/subject-catalog/p/artificial-intelligence-a-modern-approach/P200000003500/9780137505135
- 验证 URL（教材官网）：https://aima.cs.berkeley.edu/
- 验证证据：Pearson 官页确认第 4 版；UC Berkeley aima.cs.berkeley.edu 由作者维护，标注「4th US ed.」。SciRP 引文索引（scirp.org/reference/referencespapers?referenceid=3614787）以「Russell, S. J., & Norvig, P. (2020). Artificial Intelligence A Modern Approach (4th ed.). Pearson.」格式收录，与候选完全一致。
- 引用位置预估：§2.1 智能体形式化定义（PEAS、理性 agent）；§2.2 LLM agent 与传统 agent 对比

### A.4 [jennings2001agentbased]
- 标题：An Agent-Based Approach for Building Complex Software Systems
- 作者：Nicholas R. Jennings
- 出版：Communications of the ACM, Vol. 44, No. 4, pp. 35-41, April 2001
- DOI：10.1145/367211.367250
- 验证 URL（ACM DL）：https://dl.acm.org/doi/10.1145/367211.367250
- 验证 URL（CACM 官页）：https://cacm.acm.org/research/an-agent-based-approach-for-building-complex-software-systems/
- 验证证据：ACM Digital Library、CACM 官页、University of Southampton ePrints (eprints.soton.ac.uk/254229/) 三处一致。卷期页码 44(4):35-41 与候选清单完全对齐。Wikidata 收录 (Q57377532)。论文核心观点「以一组交互自治 agent 的视角分析、设计、实现复杂软件系统」是 §2.2 的引用动机。
- 引用位置预估：§2.2 软件工程视角下的 agent 系统；§3.1 伏羲架构动机

## B 桶：LLM Agent 综述与代表系统（8 篇）

### B.1 [wang2023llmagentsurvey]
- 标题：A Survey on Large Language Model based Autonomous Agents
- 作者：Wang Lei, Ma Chen, Feng Xueyang 等（人大高瓴 AI 学院）
- 平台：arXiv:2308.11432，2023-08-22 提交
- 验证 URL：https://arxiv.org/abs/2308.11432
- 验证证据：`curl https://arxiv.org/abs/2308.11432` 返回 `<title>[2308.11432] A Survey on Large Language Model based Autonomous Agents</title>`，citation_author 头三位 Wang Lei / Ma Chen / Feng Xueyang，与候选清单完全一致；citation_date 2023/08/22。
- 引用位置预估：§1.2 LLM agent 现状综述；§2.2 LLM agent 范式分类

### B.2 [yao2022react]
- 标题：ReAct: Synergizing Reasoning and Acting in Language Models
- 作者：Yao Shunyu, Zhao Jeffrey, Yu Dian, Du Nan, Shafran Izhak, Narasimhan Karthik, Cao Yuan
- 平台：arXiv:2210.03629，2022-10-06 提交；ICLR 2023 接收
- 验证 URL：https://arxiv.org/abs/2210.03629
- 验证证据：arxiv 标题 `ReAct: Synergizing Reasoning and Acting in Language Models` 与候选完全一致；citation_date 2022/10/06。论文核心 reasoning + acting 交替范式正是伏羲玄女的「思考-调度-观察-反思」循环原型。
- 引用位置预估：§2.3 LLM agent 推理-行动范式；§3.2 玄女状态机设计动机

### B.3 [schick2023toolformer]
- 标题：Toolformer: Language Models Can Teach Themselves to Use Tools
- 作者：Schick Timo, Dwivedi-Yu Jane, Dessì Roberto 等（Meta AI）
- 平台：arXiv:2302.04761，2023-02-09 提交；NeurIPS 2023 接收
- 验证 URL：https://arxiv.org/abs/2302.04761
- 验证证据：arxiv 标题完全一致；citation_date 2023/02/09。
- 引用位置预估：§2.3 LLM 调工具范式；§3.4 门客 CLI 调用设计

### B.4 [wu2023autogen]
- 标题：AutoGen: Enabling Next-Gen LLM Applications via Multi-Agent Conversation
- 作者：Wu Qingyun, Bansal Gagan, Zhang Jieyu 等（Microsoft Research）
- 平台：arXiv:2308.08155，2023-08-16 提交
- 验证 URL：https://arxiv.org/abs/2308.08155
- 验证证据：arxiv 标题完全一致；citation_date 2023/08/16。AutoGen 多 agent 对话框架是伏羲对比方案，差异点在于伏羲为分布式 + headless CLI。
- 引用位置预估：§1.2 国内外现状（对比 AutoGen）；§2.4 多 agent 编排框架

### B.5 [hong2023metagpt]
- 标题：MetaGPT: Meta Programming for a Multi-Agent Collaborative Framework
- 作者：Hong Sirui, Zhuge Mingchen, Chen Jiaqi 等
- 平台：arXiv:2308.00352，2023-08-01 提交；ICLR 2024 spotlight
- 验证 URL：https://arxiv.org/abs/2308.00352
- 验证证据：arxiv 标题与候选清单完全一致；citation_date 2023/08/01。
- 引用位置预估：§1.2 国内外现状（角色分工式 agent 团队）

### B.6 [li2023camel]
- 标题：CAMEL: Communicative Agents for "Mind" Exploration of Large Language Model Society
- 作者：Li Guohao, Hammoud Hasan A. K., Itani Hani 等（KAUST）
- 平台：arXiv:2303.17760，2023-03-31 提交；NeurIPS 2023 接收
- 验证 URL：https://arxiv.org/abs/2303.17760
- 验证证据：arxiv citation_title `CAMEL: Communicative Agents for "Mind" Exploration of Large Language Model Society` 与候选完全一致；citation_date 2023/03/31。
- 引用位置预估：§2.4 角色扮演式 agent 协作

### B.7 [wang2023voyager]
- 标题：Voyager: An Open-Ended Embodied Agent with Large Language Models
- 作者：Wang Guanzhi, Xie Yuqi, Jiang Yunfan, Mandlekar Ajay 等（NVIDIA / 多机构）
- 平台：arXiv:2305.16291，2023-05-25 提交
- 验证 URL：https://arxiv.org/abs/2305.16291
- 验证证据：arxiv 标题与候选完全一致；citation_date 2023/05/25。Voyager 终生学习与技能库思路与伏羲门客复用机制有共鸣。
- 引用位置预估：§2.3 终身学习 agent；§3.5 门客技能库（如有）

### B.8 [wang2024openhands]
- 标题：OpenHands: An Open Platform for AI Software Developers as Generalist Agents
- 作者：Wang Xingyao, Li Boxuan, Song Yufan 等（UIUC + 多机构）
- 平台：arXiv:2407.16741，2024-07-23 提交
- 验证 URL：https://arxiv.org/abs/2407.16741
- 验证证据：arxiv 标题与候选完全一致；citation_date 2024/07/23。OpenHands 即原 OpenDevin，是伏羲门客架构的对照组。
- 引用位置预估：§1.2 国内外现状（软件工程 agent 平台）；§5 实验对比基线（如有）

## C 桶：Agent 通信协议与编排（5 篇）

### C.1 [anthropic2024mcp]
- 标题：Introducing the Model Context Protocol
- 作者：Anthropic
- 发布：2024-11-25 Anthropic News；规范站点 https://modelcontextprotocol.io
- 验证 URL（公告原文）：https://www.anthropic.com/news/model-context-protocol
- 验证 URL（规范站点）：https://modelcontextprotocol.io/
- 验证证据：WebFetch modelcontextprotocol.io 返回页面正文「MCP (Model Context Protocol) is an open-source standard for connecting AI applications to external systems.」WebSearch 命中 Anthropic 官方公告页 + Wikipedia 条目，公告日期 2024-11-25 一致。MCP 是伏羲门客调用工具的可选适配层（伏羲核心仍坚持 CLI 直 shell，MCP 作为对照协议引述）。
- 引用位置预估：§2.4 agent 通信协议对比；§3.4 工具调用协议选型论证

### C.2 [a2aproject2025a2a]
- 标题：Agent2Agent (A2A) Protocol Specification
- 维护：A2A Project（开源，Google 主导，Linux Foundation 托管）
- 验证 URL：https://github.com/a2aproject/A2A
- 验证证据：WebFetch GitHub 仓页返回 README 描述「An open protocol enabling communication and interoperability between opaque agentic applications.」与候选完全一致。仓库 license Apache 2.0。伏羲 fuxi-a2a crate 即实现该协议子集。
- 引用位置预估：§2.4 agent 通信协议；§3.3 fuxi-a2a 实现章节

### C.3 [karpas2022mrkl]
- 标题：MRKL Systems: A Modular, Neuro-Symbolic Architecture That Combines Large Language Models, External Knowledge Sources and Discrete Reasoning
- 作者：Karpas Ehud, Abend Omri, Belinkov Yonatan 等（AI21 Labs）
- 平台：arXiv:2205.00445，2022-05-01 提交
- 验证 URL：https://arxiv.org/abs/2205.00445
- 验证证据：arxiv 标题与候选清单一致，citation_author 头三位 Karpas/Abend/Belinkov 与候选清单完全对齐。MRKL 是「LLM + 外部模块」的早期奠基论文。
- 引用位置预估：§2.3 LLM 调外部工具的早期范式

### C.4 [yang2024sweagent]
- 标题：SWE-agent: Agent-Computer Interfaces Enable Automated Software Engineering
- 作者：Yang John, Jimenez Carlos E., Wettig Alexander, Lieret Kilian, Yao Shunyu, Narasimhan Karthik, Press Ofir（Princeton）
- 平台：arXiv:2405.15793，2024-05-24 提交；NeurIPS 2024 接收
- 验证 URL：https://arxiv.org/abs/2405.15793
- 验证证据：arxiv 标题完全一致，作者列表完全对齐候选清单。Agent-Computer Interface (ACI) 概念与伏羲门客 CLI 接口设计同源思路。
- 引用位置预估：§2.4 软件工程 agent；§3.4 ACI 设计借鉴

### C.5 [significantgravitas2023autogpt]
- 标题：AutoGPT 仓库
- 维护：Significant-Gravitas（创始人 Toran Bruce Richards）
- 首次发布：2023-03-30
- 验证 URL：https://github.com/Significant-Gravitas/AutoGPT
- 验证证据：WebFetch 仓页返回 description「AutoGPT is the vision of accessible AI for everyone, to use and to build on.」WebSearch + Wikipedia 一致：「AutoGPT was released on March 30, 2023, by Toran Bruce Richards」。仓库当前活跃，最新 release autogpt-platform-beta-v0.6.58 (2026-04-29)。
- 引用位置预估：§1.2 国内外现状（早期 LLM agent 标杆开源项目）

## D 桶：分布式系统经典（7 篇）

### D.1 [lamport1978time]
- 标题：Time, Clocks, and the Ordering of Events in a Distributed System
- 作者：Leslie Lamport
- 出版：Communications of the ACM, Vol. 21, No. 7, pp. 558-565, July 1978
- DOI：10.1145/359545.359563
- 验证 URL（ACM DL）：https://dl.acm.org/doi/10.1145/359545.359563
- 验证 URL（CACM 公开版）：https://cacm.acm.org/research/time-clocks-and-the-ordering-of-events-in-a-distributed-system/
- 验证 URL（图灵奖 PDF）：https://amturing.acm.org/p558-lamport.pdf
- 验证证据：ACM DL、CACM、Microsoft Research（Lamport 个人主页）三处均确认 21(7):558-565 (July 1978)。荣获 2000 PODC Influential Paper Award（即 Dijkstra Prize）和 2007 SIGOPS Hall of Fame。
- 引用位置预估：§2.5 分布式系统理论（事件顺序与逻辑时钟）；§3.3 EventBus 顺序一致性论证

### D.2 [dean2004mapreduce]
- 标题：MapReduce: Simplified Data Processing on Large Clusters
- 作者：Jeffrey Dean, Sanjay Ghemawat（Google）
- 出版：6th USENIX OSDI, San Francisco, CA, December 2004, pp. 137-150
- 验证 URL（USENIX 会议页）：https://www.usenix.org/conference/osdi-04/mapreduce-simplified-data-processing-large-clusters
- 验证 URL（USENIX 全文 PDF）：https://www.usenix.org/legacy/event/osdi04/tech/full_papers/dean/dean.pdf
- 验证 URL（Google Research）：https://research.google/pubs/mapreduce-simplified-data-processing-on-large-clusters/
- 验证证据：USENIX 官网会议页 + Google Research 出版页 + ACM DL（10.5555/1251254.1251264）三处一致；OSDI'04 SF 12 月，页码 137-150 与候选清单完全对齐。
- 引用位置预估：§2.5 分布式批处理范式；§5 性能基线对比（如适用）

### D.3 [ghemawat2003gfs]
- 标题：The Google File System
- 作者：Sanjay Ghemawat, Howard Gobioff, Shun-Tak Leung（Google）
- 出版：19th SOSP, Bolton Landing, NY, October 2003
- DOI：10.1145/945445.945450
- 验证 URL（Google Research PDF）：https://research.google.com/archive/gfs-sosp2003.pdf
- 验证 URL（Google Research 索引）：https://research.google/pubs/the-google-file-system/
- 验证证据：Google 官方 PDF + 多份大学课件 + dblp 一致：SOSP 2003 Bolton Landing NY，三作者顺序 Ghemawat/Gobioff/Leung 与候选清单完全对齐。
- 引用位置预估：§2.5 分布式存储；§3.5 SQLite 选型对比（如适用）

### D.4 [kreps2011kafka]
- 标题：Kafka: A Distributed Messaging System for Log Processing
- 作者：Jay Kreps, Neha Narkhede, Jun Rao（LinkedIn）
- 出版：NetDB Workshop（与 SIGMOD 同地点协办），Athens, Greece, June 12, 2011
- 验证 URL（Apache Kafka 论文索引）：https://kafka.apache.org/community/books_and_papers/
- 验证 URL（NetDB slides）：https://netman.aiops.org/~peidan/ANM2016/BigDataSystems/ReadingLists/2011NetDB_Kafka_slides.pdf
- 验证 URL（Stephen Holiday 镜像 PDF）：https://notes.stephenholiday.com/Kafka.pdf
- 验证证据：Apache Kafka 官网 community 页面收录该论文，Semantic Scholar 与 SciRP 引文索引（2141069）一致；作者三人 Kreps/Narkhede/Rao 与候选清单完全对齐。Kafka 是伏羲事件总线设计的灵感来源之一。
- 引用位置预估：§2.5 分布式消息系统；§3.3 EventBus 设计借鉴

### D.5 [ongaro2014raft]
- 标题：In Search of an Understandable Consensus Algorithm（即 Raft 论文）
- 作者：Diego Ongaro, John Ousterhout（Stanford）
- 出版：2014 USENIX ATC，Philadelphia PA，June 2014
- 验证 URL（USENIX 会议页）：https://www.usenix.org/conference/atc14/technical-sessions/presentation/ongaro
- 验证 URL（Stanford 作者 PDF）：https://web.stanford.edu/~ouster/cgi-bin/papers/raft-atc14.pdf
- 验证 URL（Raft 官网）：https://raft.github.io/
- 验证证据：USENIX 官网会议页 + Stanford ouster 个人主页 PDF + ACM DL（10.5555/2643634.2643666）三处一致。该论文获 USENIX ATC 2014 Best Paper Award。
- 引用位置预估：§2.5 分布式共识算法；§3.6 跨节点协调机制（如分布式 v2 章节）

### D.6 [hunt2010zookeeper]
- 标题：ZooKeeper: Wait-Free Coordination for Internet-Scale Systems
- 作者：Patrick Hunt, Mahadev Konar, Flavio P. Junqueira, Benjamin Reed（Yahoo!）
- 出版：2010 USENIX ATC, Boston MA, June 23-25, 2010
- 验证 URL（USENIX 会议页）：https://www.usenix.org/conference/usenix-atc-10/zookeeper-wait-free-coordination-internet-scale-systems
- 验证 URL（dblp）：https://dblp.org/rec/conf/usenix/HuntKJR10.xml
- 验证证据：USENIX 官网 + ACM DL（10.5555/1855840.1855851）+ dblp 一致；四作者顺序 Hunt/Konar/Junqueira/Reed 与候选清单完全对齐。
- 引用位置预估：§2.5 分布式协调服务；§3.6 节点发现与租约（如适用）

### D.7 [corbett2012spanner]
- 标题：Spanner: Google's Globally-Distributed Database
- 作者：James C. Corbett 等 25 人（Google）
- 出版：10th USENIX OSDI，Hollywood CA，October 2012
- 验证 URL（USENIX 会议页）：https://www.usenix.org/conference/osdi12/technical-sessions/presentation/corbett
- 验证 URL（USENIX 全文 PDF）：https://www.usenix.org/system/files/conference/osdi12/osdi12-final-16.pdf
- 验证 URL（Google Research）：https://research.google/pubs/spanner-googles-globally-distributed-database-2/
- 验证证据：USENIX + Google Research + ACM DL（10.5555/2387880.2387905）三处一致。荣获 OSDI 2012 Jay Lepreau Best Paper Award。TrueTime API 是伏羲跨节点时间一致性思路的对照参考。
- 引用位置预估：§2.5 全球分布式数据库；§3.6 跨节点时间一致性

## E 桶：工程实现/工具/标准（6 篇）

### E.1 [tokio2024broadcast]
- 标题：Module broadcast（tokio::sync::broadcast）
- 维护：Tokio Project（tokio-rs）
- 验证 URL：https://docs.rs/tokio/latest/tokio/sync/broadcast/
- 验证证据：WebFetch 返回页面标题「Module broadcast」与首段「A multi-producer, multi-consumer broadcast queue. Each sent value is seen by all consumers.」与候选清单完全对齐。docs.rs 是 Rust 官方稳定文档站。伏羲 fuxi-events crate 的 EventBus 直接基于该 broadcast 通道。
- 引用位置预估：§3.3 EventBus 实现；§4 工程实现细节

### E.2 [sqlite2024wal]
- 标题：Write-Ahead Logging
- 维护：SQLite Consortium（D. Richard Hipp 等）
- 验证 URL：https://www.sqlite.org/wal.html
- 验证证据：WebFetch 返回页面标题「Write-Ahead Logging」与首段确认「version 3.7.0 (2010-07-21)」起 WAL 可用、「PRAGMA journal_mode=WAL」开启。伏羲事件存储依赖 WAL 模式实现读不阻写。
- 引用位置预估：§3.3 事件持久化；§4 SQLite WAL 选型论证

### E.3 [axum2024docs]
- 标题：axum: An Ergonomic and Modular Web Framework for Rust
- 维护：Tokio Project（crate owners 含 carllerche、davidpdrsn）
- 验证 URL：https://docs.rs/axum/latest/axum/
- 验证证据：WebFetch 返回「axum is an HTTP routing and request-handling library that focuses on ergonomics and modularity.」owners 列表含 github:tokio-rs:core / github:tokio-rs:axum-release。伏羲 fuxi-a2a 的 server 端使用 axum 实现 A2A 协议 endpoint。
- 引用位置预估：§3.3 fuxi-a2a server 实现；§4 框架选型

### E.4 [git2024worktree]
- 标题：git-worktree - Manage Multiple Working Trees
- 维护：Git Project（git-scm.com 官方文档）
- 验证 URL：https://git-scm.com/docs/git-worktree
- 验证证据：WebFetch 返回标题「git-worktree - Manage multiple working trees」+ description「Manage multiple working trees attached to the same repository.」与候选完全对齐。伏羲 worker pre-spawn 机制依赖 git worktree 实现并发任务隔离。
- 引用位置预估：§3.4 门客并发执行；§4 worktree sandbox 实现

### E.5 [fette2011websocket]
- 标题：The WebSocket Protocol（RFC 6455）
- 作者：Ian Fette (Google), Alexey Melnikov (Isode)
- 出版：IETF, December 2011
- DOI：10.17487/RFC6455
- 验证 URL：https://www.rfc-editor.org/rfc/rfc6455
- 验证证据：WebFetch 返回标题「The WebSocket Protocol」、作者「I. Fette (Google, Inc.) and A. Melnikov (Isode Ltd.)」、日期「December 2011」、abstract 首段与候选完全一致。伏羲 firehose 与 sia 反连均使用 WebSocket。
- 引用位置预估：§3.5 实时通讯协议；§4 firehose 实现

### E.6 [whatwg2024sse]（替换记录）
- [REPLACED hickson2024sse → whatwg2024sse]：原候选 key 为 `hickson2024sse`，标注作者 Ian Hickson。WHATWG HTML Living Standard 的 Server-Sent Events 章节（§9.2）由 WHATWG 集体维护、不署个人编辑名（Hickson 是 HTML5 早期长期主编与 SSE 原作者，但当前 living standard 不再以个人署名）。为对齐验证证据，BibTeX key 改为 `whatwg2024sse`，author 字段使用机构作者 `{WHATWG}`，避免审稿时被指为虚假个人署名。
- 标题：HTML Living Standard — Server-Sent Events
- 维护：WHATWG（Web Hypertext Application Technology Working Group）
- 验证 URL：https://html.spec.whatwg.org/multipage/server-sent-events.html
- 验证证据：WebFetch 返回章节标题「9.2 Server-sent events」与首段「To enable servers to push data to web pages over HTTP or using dedicated server-push protocols, this specification introduces the EventSource interface.」页面顶部 WHATWG 品牌标识确认机构归属。
- 引用位置预估：§3.5 实时通讯协议；§4 PWA 推送渠道选型

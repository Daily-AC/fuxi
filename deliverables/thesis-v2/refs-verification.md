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

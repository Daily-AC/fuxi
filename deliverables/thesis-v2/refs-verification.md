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

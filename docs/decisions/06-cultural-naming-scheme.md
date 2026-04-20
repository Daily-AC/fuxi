# Decision 06 · 文化底蕴命名统一方案

**日期**：2026-04-19  
**状态**：已采纳；贯穿 v1 所有 crate / 模块

## 背景

用户原话（2026-04-19）：

> 关于一些核心 role 的设定和 soul 的书写，要跟伏羲、玄女同文化底蕴，你明白我意思吧？**还记得为啥平台叫伏羲了吗？**

伏羲（三皇之首，画八卦、造六书、驯六畜 = **秩序之源**）和玄女（九天玄女，**授兵策总参**）立住了平台的文化基调。下面的 role / 模块 / 概念要**同轴同底蕴**，不是随意混搭。

## 决策 · 命名总表

| 英文（crate / 表 / 变量） | 文化名 | 语义 |
|---|---|---|
| platform | 伏羲 | 画卦造字，秩序之源 |
| top orchestrator | 玄女 | 九天玄女，授兵策 |
| worker agent | 门客 | 战国四公子蓄士 |
| `fuxi-skills` crate / `~/.fuxi/skills/` | 点将台 | 玄女点将派门客 |
| `SKILL.md` bundle | 玉牒 | 身份谱系玉册 |
| `skills/<role>.staging/` | 榜文 | 招贤暂挂榜示众 |
| `skill_smith` role | 铸牒司 | 专职铸造玉牒的门客 |
| `hetu_patterns` 表 | 河图洛书 | 上古秘传图文，= 学到的模式/技能 |
| `oracle_facts` 表 | 甲骨 | 上古刻字，最早的"写下来" |
| `events` 表（已有） | 简册 | 策简编联成册 |
| `fuxi-memory` crate | 策府 | 枢机之府，记忆总库 |
| `fuxi-scheduler` crate | 更漏 | 古之计时器 |
| `scheduler/watcher.rs` | 候吏 | 守候 trigger 的小吏 |
| `triggers` 表 | 候簿 | 候吏的值日簿 |
| `trigger_fires` 表 | 应期 | 到期应召记录 |
| role (dev) | 鲁班 `luban` | 工匠鼻祖 |
| role (pm) | 张良 `zhangliang` | 运筹帷幄，黄石公授策 |
| role (research) | 仓颉 `cangjie` | 造字典藏 |
| role (test) | 皋陶 `gaoyao` | 司法断狱 |
| role (ops) | 造父 `zaofu` | 御马驾车 |
| role (comm) | 苏秦 `suqin` | 合纵外交 |
| role (skill smith) | 铸牒司 `zhudiesi` | 招贤生成玉牒 |
| ConversationSwitch | 让贤 | 贤人代言主对话权 |
| OrchestratorCcReceived | 呈报 | 下对上的抄送 |

## 命名规则

**代码标识符**（struct / function / crate / 表名 / 变量）用**英文或拼音**——agentskills 规范要求 `name` 字段 ASCII lowercase，且跨 crate 兼容。

**文档 / 注释 / UI 显示 / EventKind 摘要**用**中文名**——让用户看到的面都是古文底蕴。

## 理由

### 门客 role 选取标准（为啥不随便）
每个 role 名必须：
1. 是**具体历史人物 / 神话人格**（不是"匠"/"策士"这种泛称）
2. 与玄女有**师传 / 辅佐关系** 或是**上古秩序源头**
3. 认知度足够（普通人能联想到其专长）

比如：
- 鲁班（公输班）= 工匠鼻祖 → dev
- 张良（受黄石公授策）= 运筹帷幄 → pm；**和玄女授黄帝兵符同构**
- 皋陶（虞舜司法）= 断狱公正 → test/QA
- 造父（周穆王驭者）= 御马驾车 → ops（**驭**是关键意象）
- 仓颉（造字始祖）= 典藏之源 → research/knowledge

### 名字的可扩展性
- 招贤（玄女为新需求造新 role）= 点将台加新玉牒 —— 名字本身就说明"动态 role 生成"
- 门客有三千 —— 命名空间无限

### 反面禁止
- **不能**用"agent"直接作 UI 名（太工程化）
- **不能**混搭朝代（仓颉上古 + 苏秦战国 OK 因为都是"历史人物"；但不能突然来个"张三"）
- **不能**用日文/洋名（伏羲是华夏创世神，文化轴要稳）

## 参考

- 原始对话：2026-04-19 关于 "每个人物都设计成 skill 的好处" + "核心 role 具体名字"
- 全表实装：`docs/architecture-v1.md` §0

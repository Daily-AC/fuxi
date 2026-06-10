# Handoff · v1-session21（2026-06-10 下午→晚）

> 本会话从「按 v1-session20 继续」起，自驱 home 实测抓到 1 个 P0 严重 bug 并修复，
> 完整重做+验证 FU-2（本地+跨节点），破解 WebSearch 谜团，最后定位玄女分身 Phase 2 缺口。
> 细节见 memory（文末指针）。

## 已 ship 上线（全 push origin/main，home+mac 部署，每个带 TDD + home 实测）

> 注：我的 commit 之后 main 又前进到 `51ead92`（别的线做的 PWA 语音/唤醒/安卓常听，
> 跟下列改动不冲突）。**下个 session 先 `git fetch origin && git pull --ff-only`**。

1. **`9151f54`（P0 严重）idle_gc 豁免 general 分身** — 块5 池化漏了 general 豁免，general
   10min idle 被 dormant 回收→`xuannv_id=None`→无 respawn 入口→玄女永久 503 到重启。home
   实锤今早 04:25 死、9.5h 全黑。修：tick_once 池分支命中 `TopicId::general()` 直接 continue。
   home 实测（TTL 45s 等 100s general 存活）。issue `29e75d2e`（awaiting_test）。
2. **`3cedcab` FU-2 worker-dispatch topic（本地）** — cherry-pick 回 `b9278fe`（守 session_id
   红线），home 实测 env 注入/topic stamp/完工路由全通。
3. **`30eb2a1` always-nudge 完工兜底信号带 topic** — pump fallback 的 AgentRequestReview 漏
   stamp topic，补上，home 验过。
4. **`0acadaf`+`7b016f0` dist 跨节点 topic** — DistJob/DistEnqueueReq 加 topic_id（serde
   default 兼容旧库），两条 enqueue 路径透传 + worker cc/codex stamp。**home↔mac 2 节点实测通过**。
   踩穿 bug：`TopicId::to_string()` 是 `topic-<uuid>` Display 形，worker 须 strip 前缀（教训：
   跨节点字段过 wire 是 Display 形不是裸值）。

**全 P0/块4/块5 路径 home 实测过**：general 存活 / 串味隔离（本地+跨节点）/ pending 队列落库+drain
补发 / FU-2 / always-nudge。

## 头号待办：玄女分身 Phase 2（玄女自己提的）

**玄女反馈属实**：切 topic「还是注入回顾，没改成独立 cc 进程」。代码（`topic_switch.rs`）坐实——
切 topic 还是 **kill 当前 cc + spawn 新 cc + 灌"回顾"prelude**（Phase 1，spec 第 9 行要替换掉的老行为）。
本会话只兑现了门客完工路由 + 持久队列那半，**用户对话侧切 topic 没变独立常驻进程**。
**详细方案 + 难点（路由要从 xuannv_id 改 current_topic、跟 general 镜像 reconciler 的张力）见 memory
`project_xuannv_topic_phase2_independent_process`。** 实打实架构活，先设计后动手。

## 其他开口

- **WebSearch**：是 cc 2.1.114 版本 bug（**非订阅**，我早先诊断错已纠正），home 因 --sdk-url 墙升不了
  cc。现行：搜网派 `@mac`（cc 2.1.170，已通）。根治=玄女改 stdio 流式输入（不用 --sdk-url），Fable 5
  挖出+实测可行，~1-2 天重构，用户定暂缓。见 memory `reference_cc_websearch_blocked` /
  `reference_cc_stdio_streaming_path`。issue `9aea3fcc` 已纠正关闭。
- **P1 以琳 PWA 实测**：`fuxi issue list` 里 6 条 awaiting_test（含本会话 `29e75d2e`）等以琳逐个验。
  另 `c8c9239d`（PWA 历史只显 10/46 条）是 open 新 bug。
- **Fable 5 模型**：硬逆向/二进制深挖/协议逆推默认派它（`cc --model claude-fable-5`），实战很强。
  见 memory `reference_fable5_for_hard_reversing`。
- **部署残留**：mac worker 旧 binary 备份 `~/.local/bin/fuxi.bak-pre-disttopic`（确认稳了可删）。
  mac dist worker 已升 cc 2.1.170 + 最新 fuxi binary。

## 文档/memory 指针
- spec：`docs/superpowers/specs/2026-06-10-玄女分身-持久队列-design.md`（Phase 2 看第 9/22-27/50 行）
- follow-up：`docs/superpowers/plans/2026-06-10-玄女分身-FOLLOWUP.md`（FU-2 已标 ship）
- 新增 memory：`project_xuannv_topic_phase2_independent_process` · `reference_cc_websearch_blocked`(纠正)
  · `reference_cc_stdio_streaming_path` · `reference_fable5_for_hard_reversing` ·
  `project_general_clone_never_gc_2026_06_10`

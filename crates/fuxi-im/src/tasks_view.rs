//! `GET /api/tasks` 数据聚合（Task #21）。
//!
//! 重建 PWA 需要的"task sheet"视图——从 EventBus 历史事件 + Fuxi shelf
//! 实时状态聚合而成。**不**改动 orchestrator 或 events schema：
//! - 历史事件（events.db）告诉我们：哪些 task 存在过 / 谁干 / 什么时候 /
//!   终态是不是 Done
//! - shelf（Fuxi 内存）告诉我们：当前哪些 agent 还活着、忙不忙
//!
//! 两路数据 outer join：每条 task 取自身事件流推 `status` + `members` +
//! `last_active`，再用 shelf 校 member 当下的 busy/idle/thinking。
//!
//! ## 性能
//!
//! 全量扫 events——v1 task 数 O(几十)、events O(几千)，单次 list_tasks 在
//! ~10ms 量级。后续上限 50 task 是 task list 视图天花板；超过靠归档。

#![allow(dead_code)]

use chrono::{DateTime, Utc};
use fuxi_core::{AgentId, Event, EventKind, TaskId, task::TaskState};
use fuxi_events::EventBus;
use fuxi_orchestrator::{Fuxi, ShelfStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberCard {
    pub agent_id: String,
    pub role: String,
    pub role_display: String,
    /// 当前活动短描述——最近一次 ToolCall 概括（`Bash: cargo test`）或最近 AgentText
    /// 头 30 字。**保留旧字段**（前 ε 任务 sheet `#26` 在用）—— 新设计的"任务树页"
    /// 用 `last_tool_call`，旧 sheet 还在过渡，不删 activity 避免破契约。
    pub activity: Option<String>,
    /// 该 agent 累计 token 用量。v1 没有该数据源——预留 None。
    pub tokens: Option<u64>,
    /// **重设计 spec**（#N2 任务树页）三态：`"busy" | "idle" | "dead"`。
    /// thinking 现归属 busy 子态——任务树页不区分（只在私聊页气泡里展示 thinking）。
    pub status: String,
    /// 重设计 #25：最近一次工具调用的概括，如 `"Bash · grep server/api/v1.go"`。
    /// **只**取 ToolCall 事件（不 fallback 到 text）—— 任务树用它直观显示门客在干啥。
    /// `None` = 该 member 还没发过工具调用。
    pub last_tool_call: Option<String>,
    /// 重设计 #25：task 派给 worker 时附的 description。从 `TaskCreated.description` 拿。
    /// 任务级 description 同 task 内所有 member 共用。
    pub description: Option<String>,
    /// β · #55：worker 当前所在节点的 `node_id`。v1 默认 `"home"`（本地 spawn）；
    /// #57 dispatch routing 加 pinned_node 后，dispatch 到远端节点的 worker
    /// 这里会变 "mac-local" / 自定义 node 名。前端 ε #59 用此字段在 member 行
    /// 渲染 `· @home` / `· @mac-local` 后缀。
    pub node_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCard {
    pub id: String,
    pub title: String,
    /// "running" | "completed" | "failed"
    pub status: String,
    /// ISO 8601
    pub created_at: String,
    pub last_active_at: String,
    pub duration_ms: u64,
    pub members: Vec<MemberCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListTasksResponse {
    pub running: Vec<TaskCard>,
    pub completed: Vec<TaskCard>,
}

/// 中间数据——单条 task 在事件流里能扒到的信息。
#[derive(Debug, Default, Clone)]
struct TaskAccumulator {
    title: Option<String>,
    /// 重设计 #25：task description（派活时附的诉求）—— member 卡片继承同一份。
    description: Option<String>,
    created_at: Option<DateTime<Utc>>,
    last_active_at: Option<DateTime<Utc>>,
    /// task 终态：None = 仍 running；Some(Done) = completed；Some(其它) = failed/cancelled
    terminal_state: Option<TaskState>,
    /// 派给的 agent ids（保序去重）
    member_ids: Vec<AgentId>,
    /// 每个 agent 的最近 tool call 简述（"Bash: cmd"，旧 activity 字段用）
    agent_activity: HashMap<AgentId, String>,
    /// 每个 agent 的最近文本响应（fallback activity）
    agent_last_text: HashMap<AgentId, String>,
    /// 重设计 #25：每个 agent 的最近 tool call（"Bash · cmd" 格式，spec 严格分隔符）
    agent_last_tool_call: HashMap<AgentId, String>,
    /// thinking 状态：True 表示正在 thinking；False/None 表示 idle 或 busy 工具中
    agent_thinking: HashMap<AgentId, bool>,
    /// #78：dist 路径下事件源节点 id（取自 `meta.source_node_id`）。home 端
    /// 本地 spawn 的事件无此字段，保持 None → MemberCard.node_id 回落 "home"。
    source_node_id: Option<String>,
}

impl TaskAccumulator {
    fn ingest(&mut self, ev: &Event) {
        // 时间戳追踪：每条事件的 meta.at 都更新 last_active_at
        let at = ev.meta.at;
        self.last_active_at = Some(self.last_active_at.map_or(at, |existing| existing.max(at)));

        // 任何带 meta.agent + meta.task 的事件都视为 member 候选——dist 路径
        // 远端 worker 不发 TaskDispatched（home 端 dispatch 已发 TaskCreated 但
        // agent_id 那时还不知道；远端 cc spawn 后只发 agent_spawning/agent_ready/
        // tool_call/agent_responded 等带 meta.agent+meta.task 的事件，靠这些累积
        // member_ids fallback。本地路径仍由 TaskDispatched 直接命中——`contains`
        // 去重保证不重复加）。
        if let (Some(agent), Some(_)) = (ev.meta.agent, ev.meta.task)
            && !self.member_ids.contains(&agent)
        {
            self.member_ids.push(agent);
        }

        // #78：远端 dist worker republish 的事件带 source_node_id；任一条命中
        // 就给整 task 标节点（不区分 agent，因 dist 路径 1 task = 1 节点的 cc）。
        if let Some(node) = ev.meta.source_node_id.as_deref()
            && self.source_node_id.is_none()
        {
            self.source_node_id = Some(node.to_string());
        }

        match &ev.kind {
            EventKind::TaskCreated { title, description } => {
                self.title = Some(title.clone());
                // description 可能为空字符串（玄女派活时偶尔不附详情）—— 转 None 让前端
                // 直接判 nullable 而非 ""
                if !description.is_empty() {
                    self.description = Some(description.clone());
                }
                if self.created_at.is_none() {
                    self.created_at = Some(at);
                }
            }
            EventKind::TaskDispatched { to } => {
                if !self.member_ids.contains(to) {
                    self.member_ids.push(*to);
                }
            }
            EventKind::TaskStateChanged { to, .. } => {
                if matches!(
                    to,
                    TaskState::Done | TaskState::Cancelled | TaskState::Delivering
                ) {
                    self.terminal_state = Some(*to);
                }
            }
            EventKind::ToolCallStarted { tool, args } => {
                if let Some(agent) = ev.meta.agent {
                    self.agent_activity
                        .insert(agent, summarize_tool_call(tool, args));
                    // 重设计 #25：last_tool_call 用 "·" 分隔（spec 严格格式）
                    self.agent_last_tool_call
                        .insert(agent, summarize_tool_call_dot(tool, args));
                    self.agent_thinking.insert(agent, false);
                }
            }
            EventKind::AgentResponded { text } => {
                if let Some(agent) = ev.meta.agent {
                    self.agent_last_text.insert(agent, summarize_text(text));
                    self.agent_thinking.insert(agent, false);
                }
            }
            EventKind::ThinkingStarted => {
                if let Some(agent) = ev.meta.agent {
                    self.agent_thinking.insert(agent, true);
                }
            }
            EventKind::ThinkingFinished => {
                if let Some(agent) = ev.meta.agent {
                    self.agent_thinking.insert(agent, false);
                }
            }
            _ => {}
        }
    }
}

/// 把 tool 调用 condense 成 ~40 char 短描述（旧 activity 字段格式：`Bash: cmd`）。
fn summarize_tool_call(tool: &str, args: &serde_json::Value) -> String {
    summarize_tool_call_with_sep(tool, args, ": ")
}

/// 重设计 #25 spec 格式：`Bash · cmd`（中间是 U+00B7 middle dot）。
/// 任务树页 last_tool_call 用此格式。
fn summarize_tool_call_dot(tool: &str, args: &serde_json::Value) -> String {
    summarize_tool_call_with_sep(tool, args, " · ")
}

fn summarize_tool_call_with_sep(tool: &str, args: &serde_json::Value, sep: &str) -> String {
    // 尝试取常见参数：command / cmd / file_path / pattern / url
    let hint = args
        .get("command")
        .or_else(|| args.get("cmd"))
        .or_else(|| args.get("file_path"))
        .or_else(|| args.get("path"))
        .or_else(|| args.get("pattern"))
        .or_else(|| args.get("url"))
        .and_then(|v| v.as_str());
    match hint {
        Some(h) => {
            let mut s = format!("{tool}{sep}{h}");
            truncate_chars(&mut s, 60);
            s
        }
        None => tool.to_string(),
    }
}

/// 把文本响应 condense 成 ~30 char 概括（按 char 计避免切割多字节字符）。
fn summarize_text(text: &str) -> String {
    let mut s = text.trim().replace('\n', " ");
    truncate_chars(&mut s, 30);
    s
}

/// **按 char** 截断；超过 limit 加省略号。CJK 字符按 1 char 算（UTF-8 三字节
/// 但视觉就是一个字）——避免按 byte 切错位。
fn truncate_chars(s: &mut String, limit: usize) {
    if s.chars().count() <= limit {
        return;
    }
    let truncated: String = s.chars().take(limit).collect();
    *s = format!("{truncated}…");
}

/// 重设计 #25 spec：成员状态严格三态 `"busy" | "idle" | "dead"`。
/// thinking 归 busy 子态——任务树页不区分（私聊页气泡里另渲染 thinking 卡）。
///
/// `thinking` 仍走 busy；只有 ShelfStatus::Dead 才映射 "dead"，
/// 让前端能区分"门客挂了"vs"门客闲着"，否则之前 dead 当 idle 显示就是用户
/// 反馈"门客没法 ping"也看不到红点的由来。
fn member_status(shelf: Option<ShelfStatus>, thinking: bool) -> String {
    match shelf {
        Some(ShelfStatus::Dead) => "dead".into(),
        Some(ShelfStatus::Idle) => "idle".into(),
        Some(ShelfStatus::Busy) => "busy".into(),
        // 不在 shelf——可能 GC 后还在事件流里有历史；视为 dead（前端可灰化）
        None => {
            if thinking {
                "busy".into()
            } else {
                "dead".into()
            }
        }
    }
}

/// role_display 中文映射——v1 hardcode 主要门客 + xuannv；其它 fallback role 原文。
/// 后续若加 role 走 ROLE.md `display_name` frontmatter 字段。
fn role_display(role: &str) -> String {
    match role {
        "xuannv" => "玄女".into(),
        "luban" => "鲁班".into(),
        "pusong" => "蒲松".into(),
        "moshu" => "墨术".into(),
        "shennong" => "神农".into(),
        other => other.to_string(),
    }
}

/// 主入口：聚合 events history + Fuxi shelf 给 PWA 的 ListTasksResponse。
///
/// `xuannv_id`：用于过滤掉玄女自己（玄女不算"任务的 member"，她是派活的人）；
/// 给 None 表示不过滤。
pub async fn aggregate(fuxi: &Fuxi, bus: &EventBus) -> crate::Result<ListTasksResponse> {
    let xuannv_id = fuxi.xuannv_id().await;

    // 1. 拿全部 task ids
    let task_ids_str = bus.list_task_ids().await.map_err(crate::Error::Events)?;
    // TaskId Display 形式是 `task-<uuid>`——store 把 to_string 落库，所以这里
    // 拿到的是带前缀的字符串。`strip_prefix` 兼容裸 uuid 形态（旧数据 / 不同 caller）。
    let mut task_ids: Vec<TaskId> = Vec::new();
    for s in &task_ids_str {
        let trimmed = s.strip_prefix("task-").unwrap_or(s);
        if let Ok(uuid) = uuid::Uuid::parse_str(trimmed) {
            task_ids.push(TaskId::from(uuid));
        }
    }

    // 2. 收集 agent role 映射——live shelf + 历史 AgentSpawning 兜底
    //    （#51 修：worker GC / shutdown 后 shelf 拿不到，role 拿空 fallback "unknown"
    //    显在 PWA 任务卡上很难看。AgentSpawning 事件持久化在 EventStore，从历史
    //    回扫一次能补上 role，让已完成 task 的 member 卡仍然显示正确角色名。
    //    扫 recent(2000) 远大于 fuxi v1 50-task × ~30 events 的天花板，O(2000) 单调
    //    序列遍历 ~ms 级，可接受。后续若 task 量级上去要换 indexed query）
    let cards = fuxi.list_workers().await;
    let mut role_by_agent: HashMap<AgentId, String> = cards
        .iter()
        .map(|c| (c.id, c.profile.role.clone()))
        .collect();
    let recent = bus
        .store()
        .recent(2000)
        .await
        .map_err(crate::Error::Events)?;
    for ev in &recent {
        if let (Some(agent), EventKind::AgentSpawning { role, .. }) = (ev.meta.agent, &ev.kind) {
            // shelf 优先（live worker 的 role 最权威）；历史只填 shelf 缺失的
            role_by_agent.entry(agent).or_insert_with(|| role.clone());
        }
    }

    // 3. 每条 task 拉历史事件聚合
    let mut running: Vec<TaskCard> = Vec::new();
    let mut completed: Vec<TaskCard> = Vec::new();
    for task_id in task_ids {
        let history = bus
            .history_for_task(task_id)
            .await
            .map_err(crate::Error::Events)?;
        if history.is_empty() {
            continue;
        }
        let mut acc = TaskAccumulator::default();
        for ev in &history {
            acc.ingest(ev);
        }

        // **重设计 #25** filter user-turn：两条都加更稳
        // 1. 直接 title 字面 == "user-turn"——玄女对话内部 task，不是真活
        //    （bridge.rs intervene-degrade-dispatch 路径产生的）
        // 2. member_ids 全是 xuannv（无门客 dispatch）—— 同样视为内部 task
        let is_user_turn_title = acc
            .title
            .as_deref()
            .map(|t| t == "user-turn")
            .unwrap_or(false);
        let is_xuannv_self = match xuannv_id {
            Some(xn) => acc.member_ids.iter().all(|a| *a == xn) && !acc.member_ids.is_empty(),
            None => false,
        };
        // 完全没 dispatch（没派给任何门客）也跳——空壳 task 用户看不出意义
        let no_real_member = acc.member_ids.is_empty()
            || match xuannv_id {
                Some(xn) => acc.member_ids.iter().all(|a| *a == xn),
                None => false,
            };
        if is_user_turn_title || is_xuannv_self || no_real_member {
            continue;
        }

        let title = acc.title.clone().unwrap_or_else(|| "（无标题）".into());
        let created = acc.created_at.unwrap_or_else(Utc::now);
        let last = acc.last_active_at.unwrap_or(created);
        let duration_ms = (last - created).num_milliseconds().max(0) as u64;

        // task 终态 snapshot——决定 member.status 显 live shelf 还是 task 完结状态。
        // #51 修：task done 后 member.status 不应再读 live shelf——worker 可能已被
        // 派到下一个 task 显示 busy，但用户看的是"这个 task 收尾了"应该 idle/done。
        // task 内 task-thread banner 也用本字段判"运行中"vs"已完成"。
        let task_terminated = matches!(
            acc.terminal_state,
            Some(TaskState::Done | TaskState::Cancelled | TaskState::Delivering)
        );

        // 把 member 列表转成 MemberCard，过滤玄女
        let mut members: Vec<MemberCard> = Vec::new();
        for agent_id in &acc.member_ids {
            if Some(*agent_id) == xuannv_id {
                continue;
            }
            // #51 修：role 取 shelf 优先，shelf 缺失再走 AgentSpawning 历史兜底
            // （已在上面 step 2 拼好），最后 fallback "unknown"——大多数路径都不会
            // 到 fallback 这一步了。
            let role = role_by_agent
                .get(agent_id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());
            let thinking = acc.agent_thinking.get(agent_id).copied().unwrap_or(false);
            let activity = acc
                .agent_activity
                .get(agent_id)
                .or_else(|| acc.agent_last_text.get(agent_id))
                .cloned();
            let last_tool_call = acc.agent_last_tool_call.get(agent_id).cloned();
            // #51 修：task 已终结时强制 idle——前端 banner / member 卡显"已完成"
            // 不再误把 worker 后续派活的 busy 状态算到本 task 头上。task 仍 running
            // 时正常读 live shelf。
            let status = if task_terminated {
                "idle".into()
            } else {
                let shelf_status = fuxi.status_of(*agent_id).await;
                member_status(shelf_status, thinking)
            };
            members.push(MemberCard {
                agent_id: agent_id.to_string(),
                role: role.clone(),
                role_display: role_display(&role),
                activity,
                tokens: None,
                status,
                last_tool_call,
                description: acc.description.clone(),
                // #78：dist 路径用 acc.source_node_id（远端事件 republish 带的
                // node_id）；本地 spawn 路径默认 "home"。修了用户实测「mac 远端
                // 任务卡显 @home」错显。
                node_id: acc.source_node_id.clone().unwrap_or_else(|| "home".into()),
            });
        }

        let status = match acc.terminal_state {
            Some(TaskState::Done) | Some(TaskState::Delivering) => "completed",
            Some(TaskState::Cancelled) => "failed",
            Some(_) => "completed",
            None => "running",
        };

        let card = TaskCard {
            id: task_id.to_string(),
            title,
            status: status.into(),
            created_at: created.to_rfc3339(),
            last_active_at: last.to_rfc3339(),
            duration_ms,
            members,
        };
        if status == "running" {
            running.push(card);
        } else {
            completed.push(card);
        }
    }

    // 4. 排序：running 按 created_at 倒序（最新派的在前）；completed 按 last_active_at 倒序
    running.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    completed.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));

    Ok(ListTasksResponse { running, completed })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_chars_handles_cjk() {
        let mut s = "你好世界一二三四五".to_string(); // 9 chars
        truncate_chars(&mut s, 5);
        assert_eq!(s, "你好世界一…");
        let mut s2 = "短".to_string();
        truncate_chars(&mut s2, 5);
        assert_eq!(s2, "短");
    }

    #[test]
    fn role_display_known_and_fallback() {
        assert_eq!(role_display("luban"), "鲁班");
        assert_eq!(role_display("xuannv"), "玄女");
        assert_eq!(role_display("custom"), "custom");
    }

    #[test]
    fn member_status_three_states() {
        // 重设计 #25：spec 严格三态 busy/idle/dead，thinking 归 busy
        assert_eq!(member_status(Some(ShelfStatus::Idle), false), "idle");
        assert_eq!(member_status(Some(ShelfStatus::Idle), true), "idle"); // shelf 说 idle 优先
        assert_eq!(member_status(Some(ShelfStatus::Busy), false), "busy");
        assert_eq!(member_status(Some(ShelfStatus::Busy), true), "busy"); // thinking 不再独立
        assert_eq!(member_status(Some(ShelfStatus::Dead), false), "dead");
        assert_eq!(member_status(Some(ShelfStatus::Dead), true), "dead");
        // shelf 不在（GC 后）—— thinking=true 视为 busy；否则 dead
        assert_eq!(member_status(None, true), "busy");
        assert_eq!(member_status(None, false), "dead");
    }

    #[test]
    fn summarize_tool_call_picks_command_arg() {
        let s = summarize_tool_call("Bash", &serde_json::json!({"command": "cargo test --lib"}));
        assert!(s.starts_with("Bash: cargo test"), "got: {s}");
        let s2 = summarize_tool_call("Read", &serde_json::json!({"file_path": "/tmp/x.rs"}));
        assert!(s2.contains("/tmp/x.rs"));
        let s3 = summarize_tool_call("WebSearch", &serde_json::json!({"unknown_arg": "x"}));
        assert_eq!(s3, "WebSearch");
    }

    #[test]
    fn summarize_text_truncates_at_30_chars() {
        let s = summarize_text(
            "这是一段很长的文本超过三十个字符所以应当被截断成省略号结尾的简短描述方便前端展示",
        );
        assert!(s.chars().count() <= 31, "got len {}", s.chars().count());
        assert!(s.ends_with('…'));
    }

    #[test]
    fn summarize_text_strips_newlines() {
        let s = summarize_text("hi\nthere");
        assert_eq!(s, "hi there");
    }

    /// TaskAccumulator 的核心契约：吃一串事件后能正确推出 title/created/members/terminal。
    #[test]
    fn accumulator_aggregates_dispatch_then_done() {
        use chrono::Duration;
        use fuxi_core::EventMeta;
        let task = TaskId::new();
        let agent = AgentId::new();
        let t0 = Utc::now();

        let mut meta1 = EventMeta::now();
        meta1.task = Some(task);
        meta1.at = t0;
        let created = Event {
            meta: meta1,
            kind: EventKind::TaskCreated {
                title: "修 ERP-1066".into(),
                description: "复现 + 修".into(),
            },
        };

        let mut meta2 = EventMeta::now();
        meta2.task = Some(task);
        meta2.at = t0 + Duration::seconds(1);
        let dispatched = Event {
            meta: meta2,
            kind: EventKind::TaskDispatched { to: agent },
        };

        let mut meta3 = EventMeta::now();
        meta3.task = Some(task);
        meta3.agent = Some(agent);
        meta3.at = t0 + Duration::seconds(5);
        let tool = Event {
            meta: meta3,
            kind: EventKind::ToolCallStarted {
                tool: "Bash".into(),
                args: serde_json::json!({"command": "cargo test"}),
            },
        };

        let mut meta4 = EventMeta::now();
        meta4.task = Some(task);
        meta4.at = t0 + Duration::seconds(60);
        let done = Event {
            meta: meta4,
            kind: EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Done,
            },
        };

        let mut acc = TaskAccumulator::default();
        for ev in [&created, &dispatched, &tool, &done] {
            acc.ingest(ev);
        }

        assert_eq!(acc.title.as_deref(), Some("修 ERP-1066"));
        assert_eq!(acc.created_at, Some(t0));
        assert_eq!(acc.last_active_at, Some(t0 + Duration::seconds(60)));
        assert_eq!(acc.member_ids, vec![agent]);
        assert_eq!(acc.terminal_state, Some(TaskState::Done));
        assert!(
            acc.agent_activity
                .get(&agent)
                .unwrap()
                .contains("cargo test")
        );
    }

    /// 重设计 #25：summarize_tool_call_dot 用 `·` 分隔，spec 严格格式。
    #[test]
    fn summarize_tool_call_dot_format() {
        let s = summarize_tool_call_dot(
            "Bash",
            &serde_json::json!({"command": "grep server/api/v1.go"}),
        );
        assert_eq!(s, "Bash · grep server/api/v1.go");
        // 无 hint 走 fallback
        let s2 = summarize_tool_call_dot("WebSearch", &serde_json::json!({}));
        assert_eq!(s2, "WebSearch");
    }

    /// 重设计 #25：accumulator description 从 TaskCreated 拿；空字符串转 None。
    #[test]
    fn accumulator_description_from_task_created() {
        use fuxi_core::EventMeta;
        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.task = Some(task);
        let with_desc = Event {
            meta: meta.clone(),
            kind: EventKind::TaskCreated {
                title: "T".into(),
                description: "复现 + 修".into(),
            },
        };
        let mut acc = TaskAccumulator::default();
        acc.ingest(&with_desc);
        assert_eq!(acc.description.as_deref(), Some("复现 + 修"));

        // 空字符串 → None（避免前端拿到 "" 当合法值）
        let empty_desc = Event {
            meta,
            kind: EventKind::TaskCreated {
                title: "T".into(),
                description: "".into(),
            },
        };
        let mut acc2 = TaskAccumulator::default();
        acc2.ingest(&empty_desc);
        assert!(acc2.description.is_none());
    }

    /// 重设计 #25：last_tool_call 字段独立累积，跟 activity（旧字段）格式不同。
    #[test]
    fn accumulator_tracks_last_tool_call_separately() {
        use fuxi_core::EventMeta;
        let agent = AgentId::new();
        let task = TaskId::new();
        let mut meta = EventMeta::now();
        meta.task = Some(task);
        meta.agent = Some(agent);
        let tool = Event {
            meta,
            kind: EventKind::ToolCallStarted {
                tool: "Bash".into(),
                args: serde_json::json!({"command": "cargo test"}),
            },
        };
        let mut acc = TaskAccumulator::default();
        acc.ingest(&tool);
        // 旧 activity 用 ":"
        assert!(
            acc.agent_activity
                .get(&agent)
                .unwrap()
                .contains("Bash: cargo")
        );
        // 新 last_tool_call 用 "·"
        assert!(
            acc.agent_last_tool_call
                .get(&agent)
                .unwrap()
                .contains("Bash · cargo")
        );
    }

    #[test]
    fn accumulator_thinking_toggle() {
        use fuxi_core::EventMeta;
        let agent = AgentId::new();
        let task = TaskId::new();

        let mut meta1 = EventMeta::now();
        meta1.agent = Some(agent);
        meta1.task = Some(task);
        let started = Event {
            meta: meta1,
            kind: EventKind::ThinkingStarted,
        };

        let mut meta2 = EventMeta::now();
        meta2.agent = Some(agent);
        meta2.task = Some(task);
        let finished = Event {
            meta: meta2,
            kind: EventKind::ThinkingFinished,
        };

        let mut acc = TaskAccumulator::default();
        acc.ingest(&started);
        assert_eq!(acc.agent_thinking.get(&agent).copied(), Some(true));
        acc.ingest(&finished);
        assert_eq!(acc.agent_thinking.get(&agent).copied(), Some(false));
    }
}

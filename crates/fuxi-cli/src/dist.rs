//! 分布式 worker 最小闭环（80 分测试版）。
//!
//! 目标：让远端机器主动连接 controller 拉任务并回传结果，不依赖 controller 入站到家宽。

use crate::dist_event_client::NetworkBusClient;
use anyhow::{Context, Result, anyhow};
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::{Args as ClapArgs, Subcommand};
use fuxi_agent_codex::CodexEvent;
use fuxi_agent_codex::parser::ItemPhase;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::{AgentId, TaskId};
use fuxi_events::EventBus;
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, Opts,
    Registry, TextEncoder,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub const DIST_TOKEN_ENV: &str = "FUXI_DIST_TOKEN";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistJob {
    pub id: String,
    /// 保留字段但语义弱化——仅作 enqueue 时留下的 "requester hint"（日志/审计可读）。
    /// 真正的路由由 `pinned_node` + `required_tags` + worker capacity 三元决定。
    /// 3a 之前这个字段是 per-node queue key，3b 之后派工不再看它。
    pub node_id: String,
    pub title: String,
    pub body: String,
    pub created_at: i64,
    /// 从 controller 端 resolve 的 role 系统提示；worker 会 prepend 到 codex
    /// prompt 头部来赋予 role 心智。老版 worker 不认识这个字段会直接忽略
    /// （`#[serde(default)]`），两端不强耦合升级节奏。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// 派工过滤：worker 必须满足 `required_tags ⊆ worker.tags` 才能取此 job。
    /// 空集 = 任意 worker 都可取（兜底路径，行为上等同 round-robin）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_tags: Vec<String>,
    /// 硬 pin 到指定 worker node。Some(x) 时只有 `node_id == x` 的 worker 能取；
    /// None 则仅靠 tags 过滤。用户显式 `fuxi spawn --node home` 走这条。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_node: Option<String>,
    /// 指定 worker 用哪个 CLI adapter（`"codex"` / `"claude-code"` / 未来
    /// `"gemini"` 等）。空串 = 默认 codex（向后兼容老版 gateway，不填就 codex）。
    /// worker 端不认识的值直接 fail job，避免 panic / 无限 retry。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cli: String,
    /// cc 专属——role 声明的 allowed_tools（`--allowed-tools` flag 的内容）。
    /// codex 忽略。老版不填 → 空 Vec → cc adapter 不加 flag。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// home 端真相 task_id（`task-<uuid>` 全形式）——worker 端用此 id 给 cc/codex
    /// agent 跑出来的所有 events 填 meta.task。否则 worker 自生成 TaskId 跟 home
    /// 完全不同 → /api/tasks aggregate 永远拼不起来（#76 实测踩坑）。
    /// 老版 worker 没此字段 → fallback `TaskId::new()` 保留旧行为。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// home 端 dispatch 时的目标 role（`"luban"` / `"pusong"` 等）——worker 端
    /// spawn cc 前用此 publish AgentSpawning，让 home aggregate 能查到 role 不
    /// fallback "unknown"（#77）。老版无此字段 → fallback `"unknown"` 旧行为。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Decision 21 phase 3 跨节点 sandbox · 项目 slug——worker 端会用本字段查
    /// 自己的 ProjectRegistry 拿到 canonical_path 并把 cc/codex 起在该 sandbox 里。
    /// `None` = 不绑项目（默认 cwd 跑），保留旧行为。
    /// `Some("erp")` + ephemeral_task=None → 走 L3 持久 sandbox（`<root>/erp/sandboxes/<role>/`）。
    /// `Some("erp")` + ephemeral_task=Some(...) → 走 L2 一次性 worktree。
    /// **要求**：worker 节点必须先 `fuxi project add <path>` 注册同名 slug，否则 fail job。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// 配 `project` 用——L2 一次性活的 task 显示形（`task-<uuid>`）。
    /// 见 `project` 字段说明。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_task: Option<String>,
    /// FU-2 跨节点收尾（2026-06-10）：home 端发起 task 的 `topic_id`（`Task.topic_id`
    /// 的 UUID 形）——worker 端用本字段给跑出来的 events.meta.topic_id 归位发起
    /// topic，让 home bridge 把跨节点门客的完工/里程碑精确路由回归属 topic 分身（不
    /// 兜底串 general）。`None` = 不绑 topic（兜底 general，同本地 task.topic_id=None）。
    /// 老版 worker 无此字段 → `#[serde(default)]` → None 旧行为。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistEnqueueReq {
    /// requester hint——见 `DistJob.node_id`。老版 gateway 会传自己的 --node 值，
    /// 新版会置空或当 `pinned_node` 用。
    pub node_id: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_node: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cli: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Decision 21 phase 3 · 项目 slug。见 `DistJob.project`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Decision 21 phase 3 · L2 一次性 task 显示形。见 `DistJob.project`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_task: Option<String>,
    /// FU-2 跨节点收尾 · 发起 task 的 topic（UUID 形）。见 `DistJob.topic_id`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistEnqueueResp {
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistPullResp {
    pub job: Option<DistJob>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistReportReq {
    pub node_id: String,
    pub job_id: String,
    pub ok: bool,
    pub output: String,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistReportResp {
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistJobStatusQuery {
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistJobStatusResp {
    pub done: bool,
    pub ok: Option<bool>,
    pub output: Option<String>,
    pub duration_ms: Option<u128>,
    pub node_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistRegisterReq {
    pub node_id: String,
    /// Worker 自报能力。gateway enqueue 若带 `required_tags` 必须是本集合的
    /// 子集才能被派给此 worker。典型：`["home", "codex", "gpu"]`。空集 =
    /// 只接受无要求的 job。向后兼容：老版 worker 不带字段 → `Vec::new()`。
    #[serde(default)]
    pub tags: Vec<String>,
    /// 并发 job 上限。默认 1（codex exec 是 one-shot 单进程，多并发要求
    /// 本机器真有冗余 ChatGPT session / API key）。
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: u32,
}

fn default_max_concurrency() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistRegisterResp {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistPullQuery {
    pub node_id: String,
}

/// 流式回传的语义分类。Gateway 把不同 kind 翻成不同 `EventKind`
/// （AssistantText → AgentResponded，Thinking → AgentThinking，
/// ToolCall → ToolInvoked-like，Error → AgentResponded 带标签），
/// 让 TUI 能按语义上色 / 折叠。
///
/// 故意不和 `fuxi_agent_codex::CodexEvent` 直接耦合：后者是 codex 的 wire 格式，
/// 本枚举是分布式层独立的语义层，将来换 wire（比如 gemini）也好改。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressKind {
    AssistantText,
    Thinking,
    ToolCall,
    Error,
}

/// controller 存储态 + pull 返回给 gateway 的块。
///
/// `seq` 由 controller 分配（per-job 单调递增 from 1），worker 上报时不填。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressChunk {
    pub seq: u64,
    pub kind: ProgressKind,
    pub text: String,
}

/// worker → controller 的单块。seq 在此**不出现**——controller 分配。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressPush {
    pub kind: ProgressKind,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistProgressReq {
    pub node_id: String,
    pub job_id: String,
    pub chunks: Vec<ProgressPush>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistProgressAck {
    /// 接受几条；job 未登记时为 0（worker 应停 push 并等 final report）。
    pub accepted: usize,
    /// 本批结束后 job 在 controller 端的最大 seq——worker 可以对账。
    pub last_seq: u64,
    /// controller 侧已收到对该 job 的 cancel 指令。worker 看到 true 就
    /// 杀 codex 子进程并以 `ok=false, output="cancelled"` 走 final report。
    ///
    /// 用 ack 捎带而非独立 endpoint：worker 每次 flush 自然到 controller，
    /// 不需要额外轮询循环。代价是"无输出时段的 cancel"要等到下一次 flush
    /// 才生效——Phase 3 再视需要加独立 cancel-poll 通道。
    #[serde(default)]
    pub should_cancel: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistCancelReq {
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistCancelResp {
    /// 成功记下 cancel flag（job 不存在也返回 true，无状态幂等）。
    pub accepted: bool,
}

/// Worker 周期心跳——把**自己真实的 inflight 列表**报给 controller。
///
/// 为什么带 inflight 不只是 ping：worker 可能因为意外重启丢失 in-memory 状态，
/// controller 这边的 `NodeRuntimeInfo.inflight` 可能比实际多。让 worker 权威
/// 声明 "我现在真的在跑这些 job"，controller 以 worker 为准，自动修复漂移。
///
/// 频率约定：worker 每 10s 发一次；controller 30s 未收到视作 dead。
///
/// **idempotent metadata（PR-B）**：`tags` + `max_concurrency` 让每次心跳都重申
/// worker 身份——controller 重启 in-memory `nodes` 表清零时，下次心跳即恢复，无需
/// worker 端独立 re-register RPC 兜底。老版 worker 不带这俩字段（`#[serde(default)]`
/// 解成 None），controller 沿用既有 entry 值（或 NodeRuntimeInfo::default 的空值）。
///
/// 缘起：home fuxi-im 5/12 00:39 重启后 mac worker 一直 heartbeat 但 `fuxi nodes`
/// 看到 tags=[] / max_concurrency=1 / registered_at_ms_ago=null——心跳走 `or_default()`
/// 新建 entry 时 NodeRuntimeInfo 字段全 default，原 register 上报的元数据丢光。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistHeartbeatReq {
    pub node_id: String,
    /// worker 自身视角的 inflight job_ids。空 = 当前空闲。
    #[serde(default)]
    pub inflight: Vec<String>,
    /// worker 重申的 tags——填充新建 entry 或刷新既有 entry。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// worker 重申的 max_concurrency。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistHeartbeatResp {
    pub ok: bool,
    /// controller 汇报的"你应该 cancel 的 job_ids"——worker 对账后杀相应 child。
    /// 当前只填 `cancelled` 集合与 worker.inflight 的交集。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cancel_pending: Vec<String>,
}

/// 远端 worker → controller 的事件转发批量。
///
/// 让运行在远端机器的子门客（cc/codex/...）产生的事件能流回 controller 主 bus，
/// TUI/IPC/firehose 才看得见。worker 内部 buffer + retry + drop policy 由 [β]
/// 的 NetworkBusClient 负责，本 endpoint 只做最小服务端：auth + 节点白名单 +
/// 原样 republish。
///
/// **空批合法**——worker 心跳式打开 keepalive 时也会触发空批；当心跳行为
/// 共用本通道，将来若真要分开再加 keepalive 字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistEventReq {
    pub node_id: String,
    #[serde(default)]
    pub events: Vec<Event>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistEventResp {
    /// controller 实际成功 publish 的条数。worker 据此对账（< len 时进入降级）。
    pub accepted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistProgressQuery {
    pub job_id: String,
    /// 只返回 seq > after 的 chunks。首次轮询传 0。
    pub after: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistProgressResp {
    pub chunks: Vec<ProgressChunk>,
    /// job 已收到终态 report，后续不会有新 chunks。
    pub done: bool,
    /// done=true 时填；否则 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_output: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NodeRuntimeInfo {
    pub last_seen: Option<Instant>,
    /// 首次 register 时间——重连不覆盖，让 `nodes_snapshot` 能算"在线时长"。
    pub registered_at: Option<Instant>,
    /// Worker 上次 register 声明的 tags。register 重连会覆盖。
    pub tags: Vec<String>,
    /// 同一 worker 允许的最大并发 job 数（≥1）。
    pub max_concurrency: u32,
    /// 当前 inflight 的 job_ids——pull 添加，report/heartbeat/sweep_stale
    /// 维护。worker 心跳的 inflight 是对账权威（自愈 controller-side 漂移）。
    pub inflight: Vec<String>,
}

impl Default for NodeRuntimeInfo {
    fn default() -> Self {
        Self {
            last_seen: None,
            registered_at: None,
            tags: Vec::new(),
            max_concurrency: 1,
            inflight: Vec::new(),
        }
    }
}

/// 心跳带的身份元数据——worker 每次心跳重申，让 controller 自愈重启丢内存态。
/// 见 [`DistHeartbeatReq`] doc 解释为何要这条。
#[derive(Debug, Clone)]
pub struct NodeHeartbeatMetadata {
    pub tags: Vec<String>,
    pub max_concurrency: u32,
}

#[derive(Default)]
struct DistInner {
    /// 全局派工队列——3b 以前是 per-node HashMap；改成全局后派工不再与 enqueuer
    /// 声明的 `node_id` 耦合，真正的路由走 pull 时的 matcher（pinned / tags / capacity）。
    global_queue: VecDeque<DistJob>,
    inflight: HashMap<String, DistJob>,
    finished: HashMap<String, DistReportReq>,
    nodes: HashMap<String, NodeRuntimeInfo>,
    /// job_id → 按 seq 有序的 progress chunks。
    progress: HashMap<String, Vec<ProgressChunk>>,
    /// job_id → 下一个要分配的 seq（从 1 开始）。
    progress_next_seq: HashMap<String, u64>,
    /// 已收到 cancel 指令的 job。worker 下一次 push 时从 ack 得知。
    cancelled: HashSet<String>,
    /// P6: 上次 publish 的 inflight 数量——心跳采样判 diff 用。
    /// heartbeat 200ms × N worker × 小时 = 百万级噪声，必须采样。
    last_published_inflight: HashMap<String, u32>,
    /// P6: 上次 publish 的 worker 状态（`"alive"` / `"stale"`）。
    /// stale→alive（sweep 后 worker 重连心跳）翻转才发；alive→alive 不发。
    last_published_status: HashMap<String, &'static str>,
}

/// 分布式 controller 的 prometheus 指标集合。
///
/// 用私有 `Registry`（不挂 default global）——一是避免跨测试串扰，二是若同
/// 进程内将来再起第二个 controller（比如 multi-tenant）也不会 panic 在
/// "Duplicate metrics collector registration"。`/metrics` handler 只 encode
/// 这一份 registry。
pub struct Metrics {
    pub registry: Registry,
    pub jobs_enqueued_total: IntCounterVec,
    pub jobs_dispatched_total: IntCounterVec,
    pub jobs_completed_total: IntCounterVec,
    pub job_duration_ms: HistogramVec,
    pub workers_registered: IntGauge,
    pub queue_depth: IntGauge,
    pub inflight_jobs: IntGaugeVec,
    pub workers_swept_total: IntCounter,
    pub workers_max_concurrency: IntGaugeVec,
    /// 远端 worker 通过 `/dist/event` 转发进来的事件总条数（按 source node 拆）。
    pub remote_events_received_total: IntCounterVec,
    /// `/dist/event` 收到事件但 publish 到 bus 失败的条数（writer 关闭等极端 case）。
    pub remote_events_publish_failed_total: IntCounterVec,
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let jobs_enqueued_total = IntCounterVec::new(
            Opts::new("fuxi_dist_jobs_enqueued_total", "分布式队列已入队 job 总数"),
            &["cli"],
        )
        .expect("metric definition is well-formed");
        let jobs_dispatched_total = IntCounterVec::new(
            Opts::new(
                "fuxi_dist_jobs_dispatched_total",
                "已 pull 到 worker 的 job 总数",
            ),
            &["cli", "node_id"],
        )
        .expect("metric definition is well-formed");
        let jobs_completed_total = IntCounterVec::new(
            Opts::new(
                "fuxi_dist_jobs_completed_total",
                "已收到 final report 的 job 总数（按 ok/失败拆）",
            ),
            &["cli", "ok"],
        )
        .expect("metric definition is well-formed");
        let job_duration_ms = HistogramVec::new(
            HistogramOpts::new(
                "fuxi_dist_job_duration_ms",
                "job 执行耗时（worker 上报，毫秒）",
            )
            .buckets(vec![
                10.0, 50.0, 100.0, 500.0, 1000.0, 5000.0, 30000.0, 120000.0,
            ]),
            &["cli"],
        )
        .expect("metric definition is well-formed");
        let workers_registered = IntGauge::new(
            "fuxi_dist_workers_registered",
            "controller 已知 worker 节点数（含 stale 未 sweep 的）",
        )
        .expect("metric definition is well-formed");
        let queue_depth = IntGauge::new(
            "fuxi_dist_queue_depth",
            "当前 global_queue 中等待派发的 job 数",
        )
        .expect("metric definition is well-formed");
        let inflight_jobs = IntGaugeVec::new(
            Opts::new(
                "fuxi_dist_inflight_jobs",
                "每个 worker 当前 inflight 的 job 数",
            ),
            &["node_id"],
        )
        .expect("metric definition is well-formed");
        let workers_swept_total = IntCounter::new(
            "fuxi_dist_workers_swept_total",
            "sweep_stale 回收掉 inflight 的 worker 次数（不是 job 数）",
        )
        .expect("metric definition is well-formed");
        let workers_max_concurrency = IntGaugeVec::new(
            Opts::new(
                "fuxi_dist_workers_max_concurrency",
                "每个 worker 注册时声明的并发上限——配合 inflight_jobs 算 saturation",
            ),
            &["node_id"],
        )
        .expect("metric definition is well-formed");
        let remote_events_received_total = IntCounterVec::new(
            Opts::new(
                "fuxi_dist_remote_events_received_total",
                "通过 /dist/event 从远端 worker 收到的事件总条数",
            ),
            &["node_id"],
        )
        .expect("metric definition is well-formed");
        let remote_events_publish_failed_total = IntCounterVec::new(
            Opts::new(
                "fuxi_dist_remote_events_publish_failed_total",
                "/dist/event 收到但 publish 到 bus 失败的事件条数（极端 case）",
            ),
            &["node_id"],
        )
        .expect("metric definition is well-formed");

        // 全部 register 一遍——任一失败 = bug，构造期 panic 比 silent drop 强
        registry
            .register(Box::new(jobs_enqueued_total.clone()))
            .expect("register jobs_enqueued_total");
        registry
            .register(Box::new(jobs_dispatched_total.clone()))
            .expect("register jobs_dispatched_total");
        registry
            .register(Box::new(jobs_completed_total.clone()))
            .expect("register jobs_completed_total");
        registry
            .register(Box::new(job_duration_ms.clone()))
            .expect("register job_duration_ms");
        registry
            .register(Box::new(workers_registered.clone()))
            .expect("register workers_registered");
        registry
            .register(Box::new(queue_depth.clone()))
            .expect("register queue_depth");
        registry
            .register(Box::new(inflight_jobs.clone()))
            .expect("register inflight_jobs");
        registry
            .register(Box::new(workers_swept_total.clone()))
            .expect("register workers_swept_total");
        registry
            .register(Box::new(workers_max_concurrency.clone()))
            .expect("register workers_max_concurrency");
        registry
            .register(Box::new(remote_events_received_total.clone()))
            .expect("register remote_events_received_total");
        registry
            .register(Box::new(remote_events_publish_failed_total.clone()))
            .expect("register remote_events_publish_failed_total");

        Self {
            registry,
            jobs_enqueued_total,
            jobs_dispatched_total,
            jobs_completed_total,
            job_duration_ms,
            workers_registered,
            queue_depth,
            inflight_jobs,
            workers_swept_total,
            workers_max_concurrency,
            remote_events_received_total,
            remote_events_publish_failed_total,
        }
    }

    /// Prometheus exposition 文本格式（text/plain; version=0.0.4）。
    pub fn encode_text(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        let encoder = TextEncoder::new();
        // encode 失败只可能是 io 错（写 Vec 不会 io 错）——unwrap 安全
        encoder
            .encode(&self.registry.gather(), &mut buf)
            .expect("encode metrics into Vec<u8> never fails");
        buf
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// controller 进程内状态。
pub struct DistController {
    /// 旧裸 token——path 3 起鉴权改走 HMAC（`dist_auth::HmacGate`），本字段
    /// 仅作 `DistController::new` 旧 ctor 兼容；新代码不要读它做鉴权。
    #[allow(dead_code)]
    token: String,
    bus: EventBus,
    inner: Mutex<DistInner>,
    pub metrics: Arc<Metrics>,
    /// 可选 SQLite 持久化（path 4 α）——`None` = 纯 in-memory 老路径，
    /// 重启丢 in-flight。生产装一个 `JobPersistence` 后 enqueue/pull/report/cancel/sweep
    /// 全自动 dual-write，restart 后调 `restore_from_persistence` 重建 queue。
    persistence: Option<Arc<crate::dist_persistence::JobPersistence>>,
}

impl DistController {
    pub fn new(token: String, bus: EventBus) -> Self {
        Self {
            token,
            bus,
            inner: Mutex::new(DistInner::default()),
            metrics: Arc::new(Metrics::new()),
            persistence: None,
        }
    }

    /// 生产路径推荐 ctor——直接绑定 persistence，等价 `Self::new(token, bus).with_persistence(p)`。
    /// 单独签名让生产入口 grep 起来一目了然；老 `new()` 留给 in-memory 测试。
    pub fn new_with_persistence(
        token: String,
        bus: EventBus,
        persistence: Arc<crate::dist_persistence::JobPersistence>,
    ) -> Self {
        Self {
            token,
            bus,
            inner: Mutex::new(DistInner::default()),
            metrics: Arc::new(Metrics::new()),
            persistence: Some(persistence),
        }
    }

    /// 注入 SQLite 持久化层（builder 形态）。让现有 in-memory-only 测试不破：
    /// `DistController::new(t, b).with_persistence(p)`。
    /// 生产路径也可直接用 `Self::new_with_persistence(t, b, p)`。
    pub fn with_persistence(
        mut self,
        persistence: Arc<crate::dist_persistence::JobPersistence>,
    ) -> Self {
        self.persistence = Some(persistence);
        self
    }

    /// γ #3 (gateway restart e2e) 的入口契约：
    ///
    /// 调用方拿到一个**新** controller（可能装了 persistence 也可能没；没装时此方法 noop）。
    /// 读 SQLite 重建 in-memory queue：
    /// - 'queued' 行按 enqueued_at 升序 **push_back**（保留原派工次序）
    /// - 'inflight' 行视作 stale orphans **push_front**（与 sweep_stale 既有语义对齐——
    ///   `dist.rs` 注释 "push_front 让被回收的 job 优先派发"；orphan 已等过一轮 controller crash，
    ///   公平起见优先派；新 enqueue 的 queued 排在 orphan 后）
    /// - 同步把 SQLite 行从 inflight 翻回 queued，避免下次重启又被当 orphan
    ///
    /// 返回 `(queued_n, orphan_n)` 给调用方做 metric / 日志。无 persistence 时返回 `(0, 0)`。
    ///
    /// **必须在 controller 接受任何 enqueue/pull 之前调用**——否则会与正常 enqueue
    /// race，job 顺序乱（虽然不丢，但派发次序与 enqueued_at 不一致）。
    pub async fn restore_from_persistence(&self) -> (usize, usize) {
        let Some(persistence) = self.persistence.as_ref() else {
            return (0, 0);
        };
        let restored = match persistence.restore().await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "dist_jobs restore 失败，跳过——此次启动相当于无 persistence");
                return (0, 0);
            }
        };
        let queued_n = restored.queued.len();
        let orphan_n = restored.orphans.len();
        let orphan_ids: Vec<String> = restored.orphans.iter().map(|j| j.id.clone()).collect();
        let mut g = self.inner.lock().await;
        // 先 push_back queued（保派工次序）；再 orphan 倒序 push_front，最终次序：
        // [orphan_0, orphan_1, ..., queued_0, queued_1, ...]
        for job in restored.queued {
            g.global_queue.push_back(job);
        }
        for job in restored.orphans.into_iter().rev() {
            g.global_queue.push_front(job);
        }
        let depth = g.global_queue.len() as i64;
        drop(g);
        self.metrics.queue_depth.set(depth);
        // SQLite 同步——orphan 翻回 queued，否则下次重启它们仍是 inflight 又被当 orphan
        for id in orphan_ids {
            if let Err(e) = persistence.record_sweep_to_queued(&id).await {
                tracing::warn!(job_id = %id, error = %e, "orphan 翻回 queued 失败——下次重启会再当 orphan");
            }
        }
        (queued_n, orphan_n)
    }

    /// 旧 token accessor——HMAC 取代后无生产 caller，保留供未迁移的 binary
    /// 入口（up.rs / repl.rs 老路径）传透传字段。
    #[allow(dead_code)]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// 测试用：拿 EventBus 句柄 subscribe，断言 publish 行为。
    /// 也允许其它子系统（TUI 拓扑面板）直接监听拓扑事件。
    /// non-test 构建里若 δ #4 还没接，clippy 会报 dead_code，先 allow。
    #[allow(dead_code)]
    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// 记录 / 更新一个 worker 的能力声明。
    ///
    /// 同一 `node_id` 再次 register（worker 重连）时：
    /// - 覆盖 `tags` 和 `max_concurrency`
    /// - 刷新 `last_seen`
    /// - **保留** `inflight`——那是 runtime state，重连不应该清空（Phase 3c
    ///   的 heartbeat/timeout 才是 inflight 的清理边界）。
    ///
    /// `max_concurrency=0` 被归一到 1，防止 worker 传 0 把自己锁死在永远不
    /// 接任务的状态。
    pub async fn register(&self, node_id: String, tags: Vec<String>, max_concurrency: u32) {
        let normalized_concurrency = max_concurrency.max(1);
        let mut g = self.inner.lock().await;
        let entry = g.nodes.entry(node_id.clone()).or_default();
        let now = Instant::now();
        entry.last_seen = Some(now);
        // 首次 register（重连不覆盖）——`registered_at` 给 `nodes_snapshot`
        // 算"在线时长"。重连后这个值不变。
        if entry.registered_at.is_none() {
            entry.registered_at = Some(now);
        }
        entry.tags = tags.clone();
        entry.max_concurrency = normalized_concurrency;
        // 重连场景：register 也算"翻转回 alive"。先清掉 last_published_status，
        // 让下一次 heartbeat 必发一条 alive（status diff 触发）；inflight diff
        // 仍由 heartbeat 自身处理。
        g.last_published_status.remove(&node_id);
        let nodes_len = g.nodes.len() as i64;
        drop(g);
        self.metrics.workers_registered.set(nodes_len);
        self.metrics
            .workers_max_concurrency
            .with_label_values(&[&node_id])
            .set(normalized_concurrency as i64);
        let _ = self.bus.publish(Event {
            meta: EventMeta::now(),
            kind: EventKind::WorkerRegistered {
                node_id,
                tags,
                max_concurrency: normalized_concurrency,
            },
        });
    }

    /// 快照查询：返回 `node_id` 当前的 runtime 信息（`None` 表示从未 register）。
    /// Phase 3b 的 tag-based 派工匹配会消费 `tags` 和 `max_concurrency`。
    // Phase 3a 只在测试里有 caller；3b 的派工算法会正式消费。
    #[allow(dead_code)]
    pub async fn node_info(&self, node_id: &str) -> Option<NodeRuntimeInfo> {
        self.inner.lock().await.nodes.get(node_id).cloned()
    }

    /// 全量节点快照——供 IPC `Command::Nodes` / TUI 拓扑 panel 用。
    ///
    /// - 按 `node_id` 字典序排序，输出稳定（测试可断言、TUI 不抖）
    /// - **不暴露 `Instant`**——折成 `last_seen_ms_ago` / `registered_at_ms_ago`，
    ///   让 wire 类型保持可序列化、跨进程无歧义
    /// - `status` 字段：`alive` / `stale` / `unknown`，按 `last_seen` 与
    ///   `STALE_THRESHOLD` (60s) 比较，与 `sweep_stale` 默认阈值对齐——TUI 标
    ///   红的边界和 controller 自我回收的边界是同一根线
    pub async fn nodes_snapshot(&self) -> Vec<crate::ipc::NodeSnapshot> {
        const STALE_THRESHOLD: Duration = Duration::from_secs(60);
        let now = Instant::now();
        let snapshot = {
            let g = self.inner.lock().await;
            // 在锁内只拷数据，不计算时间——锁外再算 ms_ago
            let mut entries: Vec<(String, NodeRuntimeInfo)> = g
                .nodes
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            entries
        };
        snapshot
            .into_iter()
            .map(|(node_id, info)| {
                let last_seen_ms_ago = info
                    .last_seen
                    .map(|ts| now.saturating_duration_since(ts).as_millis() as u64);
                let registered_at_ms_ago = info
                    .registered_at
                    .map(|ts| now.saturating_duration_since(ts).as_millis() as u64);
                let status = match info.last_seen {
                    None => "unknown",
                    Some(ts) if now.saturating_duration_since(ts) > STALE_THRESHOLD => "stale",
                    Some(_) => "alive",
                }
                .to_string();
                crate::ipc::NodeSnapshot {
                    node_id,
                    tags: info.tags,
                    max_concurrency: info.max_concurrency,
                    inflight_count: info.inflight.len(),
                    inflight: info.inflight,
                    last_seen_ms_ago,
                    registered_at_ms_ago,
                    status,
                }
            })
            .collect()
    }

    /// 旧 wrapper——保留 10 参形参兼容已有大量测试 callsite。
    /// 新代码请用 `enqueue_with_project` 直接传 project / ephemeral_task。
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue(
        &self,
        node_id_hint: String,
        title: String,
        body: String,
        system_prompt: Option<String>,
        required_tags: Vec<String>,
        pinned_node: Option<String>,
        cli: String,
        allowed_tools: Vec<String>,
        task_id: Option<String>,
        role: Option<String>,
    ) -> String {
        self.enqueue_with_project(
            node_id_hint,
            title,
            body,
            system_prompt,
            required_tags,
            pinned_node,
            cli,
            allowed_tools,
            task_id,
            role,
            None,
            None,
            None,
        )
        .await
    }

    /// Decision 21 phase 3 全形参 enqueue——支持 project / ephemeral_task 透传。
    #[allow(clippy::too_many_arguments)]
    pub async fn enqueue_with_project(
        &self,
        node_id_hint: String,
        title: String,
        body: String,
        system_prompt: Option<String>,
        required_tags: Vec<String>,
        pinned_node: Option<String>,
        cli: String,
        allowed_tools: Vec<String>,
        task_id: Option<String>,
        role: Option<String>,
        project: Option<String>,
        ephemeral_task: Option<String>,
        topic_id: Option<String>,
    ) -> String {
        let id = format!("job-{}", Uuid::new_v4());
        let job = DistJob {
            id: id.clone(),
            node_id: node_id_hint.clone(),
            title: title.clone(),
            body,
            created_at: chrono::Utc::now().timestamp(),
            system_prompt,
            required_tags: required_tags.clone(),
            pinned_node: pinned_node.clone(),
            cli,
            allowed_tools,
            task_id,
            role,
            project,
            ephemeral_task,
            topic_id,
        };
        let cli_label = cli_label_of(&job.cli);
        // dual-write：先持久化（SQLite 是真相源）再 push 到 in-memory queue。
        // 顺序很重要——反过来的话，controller 死在两步之间会让 in-memory 有 job 而
        // SQLite 没有，restart 后丢。即使 SQLite 写失败（极端 case 比如磁盘满），
        // 我们仍 push in-memory 让 hot path 能跑——只是失去 restart-safety，运维通过
        // 监控 dist_jobs 写失败率发现。
        if let Some(persistence) = self.persistence.as_ref()
            && let Err(e) = persistence.record_enqueue(&job).await
        {
            tracing::warn!(job_id = %job.id, error = %e, "dist_jobs enqueue 写入失败——in-memory 仍接，重启会丢");
        }
        let mut g = self.inner.lock().await;
        g.global_queue.push_back(job);
        let depth = g.global_queue.len() as i64;
        drop(g);
        self.metrics
            .jobs_enqueued_total
            .with_label_values(&[&cli_label])
            .inc();
        self.metrics.queue_depth.set(depth);
        let _ = self.bus.publish(Event {
            meta: EventMeta::now(),
            kind: EventKind::Custom {
                label: "dist_job_enqueued".into(),
                payload: serde_json::json!({
                    "job_id": id,
                    "node_id_hint": node_id_hint,
                    "required_tags": required_tags,
                    "pinned_node": pinned_node,
                    "title": title
                }),
            },
        });
        id
    }

    /// 派工匹配：worker `node_id` 想取任务，扫全局 queue 找**第一个**满足以下
    /// 三条的 job：
    /// 1. `pinned_node.is_none() || pinned_node == Some(node_id)` ——未 pin 或 pin 到我
    /// 2. `required_tags ⊆ worker.tags` ——能力是超集
    /// 3. worker 还有并发额度（`inflight.len() < max_concurrency`）
    ///
    /// 匹配后 job 从 queue 移走、记入 controller.inflight、push 进 worker.inflight。
    /// 未注册（从未 register）的 worker 视为空 tags + 默认 1 并发。
    pub async fn pull(&self, node_id: &str) -> Option<DistJob> {
        let mut g = self.inner.lock().await;
        // 先刷新 last_seen 并拿 capacity/tags 快照；3a 的 register 没跑过也兜底建默认
        let (worker_tags, capacity_left) = {
            let node = g.nodes.entry(node_id.to_string()).or_default();
            node.last_seen = Some(Instant::now());
            let left = (node.max_concurrency as usize).saturating_sub(node.inflight.len());
            (node.tags.clone(), left)
        };
        if capacity_left == 0 {
            return None;
        }
        let idx = g.global_queue.iter().position(|job| {
            if let Some(pin) = &job.pinned_node
                && pin != node_id
            {
                return false;
            }
            // required_tags ⊆ worker_tags
            job.required_tags.iter().all(|t| worker_tags.contains(t))
        })?;
        let job = g
            .global_queue
            .remove(idx)
            .expect("position just returned a valid index");
        g.inflight.insert(job.id.clone(), job.clone());
        // 写 worker.inflight——pull 是唯一入口，report/timeout 是出口
        let worker_inflight_len = {
            let node = g.nodes.get_mut(node_id).expect("entry was just touched");
            node.inflight.push(job.id.clone());
            node.inflight.len() as i64
        };
        let depth = g.global_queue.len() as i64;
        let cli_label = cli_label_of(&job.cli);
        let job_id_for_persist = job.id.clone();
        drop(g);
        // dual-write：in-memory 已 commit，SQLite 同步翻 inflight。失败也不退回——
        // 退回会让上层"已经派发"的承诺失效，引发更糟的双派；只 warn。
        if let Some(persistence) = self.persistence.as_ref()
            && let Err(e) = persistence.record_pull(&job_id_for_persist, node_id).await
        {
            tracing::warn!(job_id = %job_id_for_persist, error = %e, "dist_jobs pull 写入失败——重启可能误当 queued 重发");
        }
        self.metrics
            .jobs_dispatched_total
            .with_label_values(&[&cli_label, node_id])
            .inc();
        self.metrics.queue_depth.set(depth);
        self.metrics
            .inflight_jobs
            .with_label_values(&[node_id])
            .set(worker_inflight_len);
        let _ = self.bus.publish(Event {
            meta: EventMeta::now(),
            kind: EventKind::Custom {
                label: "dist_job_dispatched".into(),
                payload: serde_json::json!({
                    "job_id": job.id,
                    "node_id": node_id,
                    "title": job.title,
                    "required_tags": job.required_tags,
                    "pinned_node": job.pinned_node,
                }),
            },
        });
        Some(job)
    }

    pub async fn report(&self, req: DistReportReq) -> bool {
        let mut g = self.inner.lock().await;
        g.nodes.entry(req.node_id.clone()).or_default().last_seen = Some(Instant::now());
        let removed_job = g.inflight.remove(&req.job_id);
        let existed = removed_job.is_some();
        // 从 controller 端 inflight 拿 cli 标签；controller 重启后 race
        // 收 report 时可能拿不到，回退到 "unknown" 不丢指标
        let cli_label = removed_job
            .as_ref()
            .map(|j| cli_label_of(&j.cli))
            .unwrap_or_else(|| "unknown".to_string());
        g.finished.insert(req.job_id.clone(), req.clone());
        // Phase 3b: 从 worker 的 inflight list 释放——否则 capacity 永远 0
        let worker_inflight_len = if let Some(worker) = g.nodes.get_mut(&req.node_id) {
            worker.inflight.retain(|id| id != &req.job_id);
            worker.inflight.len() as i64
        } else {
            0
        };
        drop(g);
        // dual-write：终态写盘——重启后该 job 不会再被 restore 到 queue。
        if let Some(persistence) = self.persistence.as_ref()
            && let Err(e) = persistence.record_report(&req.job_id, req.ok).await
        {
            tracing::warn!(job_id = %req.job_id, error = %e, "dist_jobs report 写入失败——重启可能误当 inflight 重派");
        }
        let ok_label = if req.ok { "true" } else { "false" };
        self.metrics
            .jobs_completed_total
            .with_label_values(&[&cli_label, ok_label])
            .inc();
        self.metrics
            .job_duration_ms
            .with_label_values(&[&cli_label])
            .observe(req.duration_ms as f64);
        self.metrics
            .inflight_jobs
            .with_label_values(&[&req.node_id])
            .set(worker_inflight_len);
        let _ = self.bus.publish(Event {
            meta: EventMeta::now(),
            kind: EventKind::Custom {
                label: if req.ok {
                    "dist_job_succeeded".into()
                } else {
                    "dist_job_failed".into()
                },
                payload: serde_json::json!({
                    "job_id": req.job_id,
                    "node_id": req.node_id,
                    "duration_ms": req.duration_ms,
                    "output": req.output
                }),
            },
        });
        // Bug 修（v1-session15+）：dist 路径 task 没 lifecycle 终态 emit，task 永远
        // 卡 running——worker 只 emit AgentSpawning/AgentDead 不动 Task lifecycle，
        // home pump 在 dist 路径根本不跑（dispatch 走 enqueue 直接 return）。
        // controller 收到 worker report 是把"dist 视图终结"翻译成"task 视图终结"
        // 的唯一时机。task_id 缺失（老 job / 测试）跳过。
        if let Some(task_str) = removed_job.as_ref().and_then(|j| j.task_id.as_deref()) {
            let trimmed = task_str.strip_prefix("task-").unwrap_or(task_str);
            if let Ok(task_uuid) = uuid::Uuid::parse_str(trimmed) {
                let task_id = fuxi_core::TaskId::from(task_uuid);
                let mut meta = EventMeta::now();
                meta.task = Some(task_id);
                let to = if req.ok {
                    fuxi_core::task::TaskState::Done
                } else {
                    fuxi_core::task::TaskState::Cancelled
                };
                let _ = self.bus.publish(Event {
                    meta,
                    kind: EventKind::TaskStateChanged {
                        from: fuxi_core::task::TaskState::InProgress,
                        to,
                    },
                });
            }
        }
        existed
    }

    pub async fn job_status(&self, job_id: &str) -> DistJobStatusResp {
        let g = self.inner.lock().await;
        if let Some(done) = g.finished.get(job_id) {
            return DistJobStatusResp {
                done: true,
                ok: Some(done.ok),
                output: Some(done.output.clone()),
                duration_ms: Some(done.duration_ms),
                node_id: Some(done.node_id.clone()),
            };
        }
        DistJobStatusResp {
            done: false,
            ok: None,
            output: None,
            duration_ms: None,
            node_id: None,
        }
    }

    /// worker 上报一批 progress chunks。controller 分配 per-job 单调 seq。
    ///
    /// 若 job 从未 pull 过（inflight 不存在）也不拒——允许 worker 在 pull
    /// 与首次 push 之间有 race。返回 `(accepted, last_seq)`：accepted 总是
    /// 等于 chunks.len()（当前不做拒收），last_seq 用于 worker 对账。
    pub async fn push_progress(
        &self,
        node_id: &str,
        job_id: &str,
        pushes: Vec<ProgressPush>,
    ) -> (usize, u64, bool) {
        let mut g = self.inner.lock().await;
        g.nodes.entry(node_id.to_string()).or_default().last_seen = Some(Instant::now());
        // 先把 next_seq 拷出来释放 progress_next_seq 的借用，避免 progress 那边的
        // second mutable borrow 冲突。
        let mut next_seq = *g.progress_next_seq.entry(job_id.to_string()).or_insert(1);
        let mut accepted = 0usize;
        let mut last_seq = next_seq.saturating_sub(1);
        let bucket = g.progress.entry(job_id.to_string()).or_default();
        for p in pushes {
            let chunk = ProgressChunk {
                seq: next_seq,
                kind: p.kind,
                text: p.text,
            };
            last_seq = chunk.seq;
            bucket.push(chunk);
            next_seq += 1;
            accepted += 1;
        }
        g.progress_next_seq.insert(job_id.to_string(), next_seq);
        let should_cancel = g.cancelled.contains(job_id);
        (accepted, last_seq, should_cancel)
    }

    /// 标记 job 已被取消。worker 下一次 push_progress 的 ack 里会看到
    /// should_cancel=true。幂等——对不存在或已 finished 的 job 也返回 true。
    pub async fn cancel_job(&self, job_id: &str) {
        let mut g = self.inner.lock().await;
        g.cancelled.insert(job_id.to_string());
        drop(g);
        // dual-write：cancel 是"调度意图"——SQLite 标 cancelled 让重启后不再 restore；
        // runtime 杀进程仍走 heartbeat ack 路径（worker 看到 should_cancel=true）。
        if let Some(persistence) = self.persistence.as_ref()
            && let Err(e) = persistence.record_cancel(job_id).await
        {
            tracing::warn!(job_id = %job_id, error = %e, "dist_jobs cancel 写入失败——重启会丢 cancel 意图");
        }
    }

    /// 心跳：worker 以自身为权威声明当前真实 inflight。
    ///
    /// - 刷新 `last_seen`
    /// - 把 controller 这边 `NodeRuntimeInfo.inflight` **替换**成 worker 报的
    ///   列表（自动对账，worker 重启丢 state 的 case 会自然修复）
    /// - 返回 `cancel_pending` = worker.inflight ∩ controller.cancelled，让
    ///   worker 看到该杀哪些 child（补救"push 无输出时段 cancel 不生效"）
    ///
    /// 注意：不从 controller.inflight (全局 job 表) 里移除那些 worker 声明已
    /// 不在的 job——移除权归 report（job 结束）和 sweep_stale（worker 死亡）。
    pub async fn heartbeat(
        &self,
        node_id: &str,
        worker_inflight: Vec<String>,
        metadata: Option<NodeHeartbeatMetadata>,
    ) -> Vec<String> {
        let mut g = self.inner.lock().await;
        let cancelled = g.cancelled.clone();
        let node = g.nodes.entry(node_id.to_string()).or_default();
        let now = Instant::now();
        node.last_seen = Some(now);
        node.inflight = worker_inflight.clone();
        // PR-B：心跳 idempotent metadata。新 entry（register 未跑过 / controller 重启
        // 丢内存态）经此自愈：tags/cap 在心跳后立即正确，无需 worker 端独立 re-register。
        // 已有 entry 也会被刷新——register 跟心跳一致时无副作用；不一致时心跳是后到
        // 的"worker 真当前认知"，让它赢。
        if let Some(meta) = metadata {
            let normalized = meta.max_concurrency.max(1);
            node.tags = meta.tags;
            node.max_concurrency = normalized;
            if node.registered_at.is_none() {
                node.registered_at = Some(now);
            }
        }
        let inflight_len = node.inflight.len() as i64;
        let inflight_count = node.inflight.len() as u32;
        // P6 采样：仅在 inflight_count 与上次发布不同 OR 状态翻转 (stale→alive)
        // 才 publish——心跳 200ms × N worker 全发会百万级噪声。
        let prev_count = g.last_published_inflight.get(node_id).copied();
        let prev_status = g.last_published_status.get(node_id).copied();
        let count_changed = prev_count != Some(inflight_count);
        let status_flipped = prev_status != Some("alive");
        let should_publish = count_changed || status_flipped;
        if should_publish {
            g.last_published_inflight
                .insert(node_id.to_string(), inflight_count);
            g.last_published_status.insert(node_id.to_string(), "alive");
        }
        drop(g);
        // worker 是 inflight 权威——心跳到了就把 gauge 对齐
        self.metrics
            .inflight_jobs
            .with_label_values(&[node_id])
            .set(inflight_len);
        if should_publish {
            let _ = self.bus.publish(Event {
                meta: EventMeta::now(),
                kind: EventKind::WorkerHeartbeatStateChanged {
                    node_id: node_id.to_string(),
                    inflight_count,
                    status: fuxi_core::WorkerStatus::Alive,
                },
            });
        }
        worker_inflight
            .into_iter()
            .filter(|jid| cancelled.contains(jid))
            .collect()
    }

    /// Sweep：回收 `now - last_seen > stale_after` 的 worker 占用的 job。
    ///
    /// 回收策略：
    /// - 从该 worker 的 `inflight` 列表逐个回收 job_id
    /// - 从 controller 全局 `inflight` 移除，`global_queue` push_front 让它优先
    ///   被下一个 live worker 取（避免队尾打转）
    /// - 清空 node.inflight
    ///
    /// 返回被回收的 `(node_id, job_ids)`，调用方可发事件 / 日志。
    ///
    /// **未做**：retry_count 上限保护。第一版假设 dead worker 重抢一次就行，
    /// 真需要防 poison job 循环，在 DistJob 加计数字段再做。
    // 3c 只落协议 + 算法，up/repl 侧的 periodic sweep task 留 3c-2；
    // clippy 会把 sweep_stale 视作无 binary caller——测试里用到但 bin target 看不到。
    #[allow(dead_code)]
    pub async fn sweep_stale(
        &self,
        now: Instant,
        stale_after: Duration,
    ) -> Vec<(String, Vec<String>)> {
        let mut g = self.inner.lock().await;
        let dead: Vec<String> = g
            .nodes
            .iter()
            .filter_map(|(nid, info)| match info.last_seen {
                Some(ts) if now.saturating_duration_since(ts) > stale_after => Some(nid.clone()),
                _ => None,
            })
            .collect();
        let mut recycled = Vec::new();
        let mut swept_nodes_with_jobs = 0u64;
        // 先收集 (node_id, recycled_jobs) 全集——含空 jobs 的 dead worker，
        // 因为 WorkerStaleSwept 事件需要给"worker 失联"信号面板，不只看 job 视角。
        let mut publish_targets: Vec<(String, Vec<String>)> = Vec::with_capacity(dead.len());
        for nid in dead {
            let Some(node) = g.nodes.get_mut(&nid) else {
                continue;
            };
            let jobs = std::mem::take(&mut node.inflight);
            let mut jobs_to_push = Vec::with_capacity(jobs.len());
            for jid in &jobs {
                if let Some(job) = g.inflight.remove(jid) {
                    jobs_to_push.push(job);
                }
            }
            // push_front 让被回收的 job 优先派发（老 job 等待久了，公平）
            for job in jobs_to_push.into_iter().rev() {
                g.global_queue.push_front(job);
            }
            if !jobs.is_empty() {
                recycled.push((nid.clone(), jobs.clone()));
                swept_nodes_with_jobs += 1;
            }
            // 节点 inflight 已清——gauge 也跟着清
            self.metrics
                .inflight_jobs
                .with_label_values(&[nid.as_str()])
                .set(0);
            // 标记 last_published_status=stale，让 worker 重连时下次心跳的
            // status_flipped (stale→alive) 必发一条 WorkerHeartbeatStateChanged。
            g.last_published_status.insert(nid.clone(), "stale");
            publish_targets.push((nid, jobs));
        }
        let depth = g.global_queue.len() as i64;
        drop(g);
        self.metrics.queue_depth.set(depth);
        if swept_nodes_with_jobs > 0 {
            self.metrics
                .workers_swept_total
                .inc_by(swept_nodes_with_jobs);
        }
        // dual-write：sweep 把 in-memory 的 inflight job 翻回 queue（push_front），
        // 这里同步把 SQLite 行从 inflight 翻回 queued，避免重启后又被当 orphan。
        if let Some(persistence) = self.persistence.as_ref() {
            for (_nid, jobs) in &publish_targets {
                for jid in jobs {
                    if let Err(e) = persistence.record_sweep_to_queued(jid).await {
                        tracing::warn!(job_id = %jid, error = %e, "dist_jobs sweep 写入失败");
                    }
                }
            }
        }
        for (nid, jobs) in publish_targets {
            let _ = self.bus.publish(Event {
                meta: EventMeta::now(),
                kind: EventKind::WorkerStaleSwept {
                    node_id: nid,
                    recycled_jobs: jobs,
                },
            });
        }
        recycled
    }

    /// Gateway 短轮询：返回 `seq > after` 的所有 chunks + job 终态。
    ///
    /// `done=true` 意味着 controller 已收到 final report，**且** 所有之前上报
    /// 的 progress 都已包含在内（worker 承诺 report 前 flush 干净 progress）。
    pub async fn pull_progress_after(&self, job_id: &str, after: u64) -> DistProgressResp {
        let g = self.inner.lock().await;
        let chunks = g
            .progress
            .get(job_id)
            .map(|v| v.iter().filter(|c| c.seq > after).cloned().collect())
            .unwrap_or_default();
        if let Some(done) = g.finished.get(job_id) {
            DistProgressResp {
                chunks,
                done: true,
                final_ok: Some(done.ok),
                final_output: Some(done.output.clone()),
            }
        } else {
            DistProgressResp {
                chunks,
                done: false,
                final_ok: None,
                final_output: None,
            }
        }
    }

    /// 远端 worker 转发的一批事件 → controller 主 bus 原样 publish。
    ///
    /// 调用前 endpoint 已校验 token 与 node 注册；本方法只关心 publish + 计数。
    /// 入口处 stamp `EventMeta.source_node_id = Some(node_id)`——
    /// 让 TUI/firehose 从 meta 区分本地 vs 远端事件，无需依赖外部上下文。
    /// **覆盖而非保留** worker 自己塞的值：信任域以 controller 边界为准，
    /// 防止任何中间环节伪造 source_node_id。
    pub fn republish_remote_events(&self, node_id: &str, events: Vec<Event>) -> usize {
        let total = events.len() as u64;
        let mut accepted = 0usize;
        let mut failed = 0u64;
        for mut ev in events {
            ev.meta.source_node_id = Some(node_id.to_string());
            match self.bus.publish(ev) {
                Ok(()) => accepted += 1,
                Err(_) => failed += 1,
            }
        }
        if total > 0 {
            self.metrics
                .remote_events_received_total
                .with_label_values(&[node_id])
                .inc_by(total);
        }
        if failed > 0 {
            self.metrics
                .remote_events_publish_failed_total
                .with_label_values(&[node_id])
                .inc_by(failed);
        }
        accepted
    }

    /// 仅给 endpoint 用：判断 node_id 是否曾 register。未注册 → 403 拒收陌生流量。
    pub async fn has_node(&self, node_id: &str) -> bool {
        self.inner.lock().await.nodes.contains_key(node_id)
    }
}

fn forbidden_unknown_node() -> (StatusCode, String) {
    (StatusCode::FORBIDDEN, "node not registered".to_string())
}

// 注：所有 /dist/* 入口的 401 鉴权已下沉到 `crate::dist_auth::hmac_layer`
// （path 3 α）；handler 自身只关心业务校验。如果 router 没挂 HMAC layer
// （单测/旧路径直接 mount router 的场景），handler 行为退化到无鉴权——
// 生产路径必走 `router_with_hmac`。

async fn register_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistRegisterReq>,
) -> impl IntoResponse {
    ctrl.register(req.node_id, req.tags, req.max_concurrency)
        .await;
    Json(DistRegisterResp { ok: true }).into_response()
}

async fn enqueue_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistEnqueueReq>,
) -> impl IntoResponse {
    if req.title.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "title empty".to_string()).into_response();
    }
    let job_id = ctrl
        .enqueue_with_project(
            req.node_id,
            req.title,
            req.body,
            req.system_prompt,
            req.required_tags,
            req.pinned_node,
            req.cli,
            req.allowed_tools,
            req.task_id,
            req.role,
            req.project,
            req.ephemeral_task,
            req.topic_id,
        )
        .await;
    Json(DistEnqueueResp { job_id }).into_response()
}

async fn pull_handler(
    State(ctrl): State<Arc<DistController>>,
    Query(q): Query<DistPullQuery>,
) -> impl IntoResponse {
    let job = ctrl.pull(&q.node_id).await;
    Json(DistPullResp { job }).into_response()
}

async fn report_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistReportReq>,
) -> impl IntoResponse {
    let accepted = ctrl.report(req).await;
    Json(DistReportResp { accepted }).into_response()
}

async fn job_status_handler(
    State(ctrl): State<Arc<DistController>>,
    Query(q): Query<DistJobStatusQuery>,
) -> impl IntoResponse {
    Json(ctrl.job_status(&q.job_id).await).into_response()
}

async fn progress_post_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistProgressReq>,
) -> impl IntoResponse {
    let (accepted, last_seq, should_cancel) = ctrl
        .push_progress(&req.node_id, &req.job_id, req.chunks)
        .await;
    Json(DistProgressAck {
        accepted,
        last_seq,
        should_cancel,
    })
    .into_response()
}

async fn progress_get_handler(
    State(ctrl): State<Arc<DistController>>,
    Query(q): Query<DistProgressQuery>,
) -> impl IntoResponse {
    Json(ctrl.pull_progress_after(&q.job_id, q.after).await).into_response()
}

async fn cancel_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistCancelReq>,
) -> impl IntoResponse {
    ctrl.cancel_job(&req.job_id).await;
    Json(DistCancelResp { accepted: true }).into_response()
}

async fn event_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistEventReq>,
) -> impl IntoResponse {
    if !ctrl.has_node(&req.node_id).await {
        return forbidden_unknown_node().into_response();
    }
    let accepted = ctrl.republish_remote_events(&req.node_id, req.events);
    Json(DistEventResp { accepted }).into_response()
}

async fn heartbeat_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistHeartbeatReq>,
) -> impl IntoResponse {
    let metadata = match (req.tags, req.max_concurrency) {
        (Some(tags), Some(max_concurrency)) => Some(NodeHeartbeatMetadata {
            tags,
            max_concurrency,
        }),
        // 任一字段缺失就当老版 worker 不带 metadata——保留兜底，新版始终成对发。
        _ => None,
    };
    let cancel_pending = ctrl.heartbeat(&req.node_id, req.inflight, metadata).await;
    Json(DistHeartbeatResp {
        ok: true,
        cancel_pending,
    })
    .into_response()
}

/// 后台 sweep tick——每 30s 扫一次 `last_seen > STALE_SECS` 的 worker，把它们
/// 的 inflight 回滚到 global_queue 前端。`up`/`repl` 启动 controller 时调一次
/// 即可，返回 `JoinHandle` 供调用方持有（目前没关心 lifetime，`tokio::spawn`
/// 足矣；daemon 下线时 tokio runtime 一起终结）。
///
/// 阈值 60s = 两次 worker 心跳间隔的合理上限。worker 每 10s 心跳，5-6 次丢包
/// 才会超，通常意味着进程真死或 controller 侧丢数据，回收是对的。
pub fn spawn_sweep_task(ctrl: Arc<DistController>) {
    const TICK_SECS: u64 = 30;
    const STALE_SECS: u64 = 60;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(TICK_SECS));
        // 跳过 tokio interval 的 immediate first tick——controller 刚起，
        // 任何 worker 都还没 register，扫一遍无意义还制造启动噪音。
        tick.tick().await;
        loop {
            tick.tick().await;
            let recycled = ctrl
                .sweep_stale(Instant::now(), Duration::from_secs(STALE_SECS))
                .await;
            for (node_id, jobs) in recycled {
                tracing::warn!(
                    node_id = %node_id,
                    jobs = ?jobs,
                    "sweep: recycled inflight from stale worker back to queue front"
                );
            }
        }
    });
}

pub fn router(ctrl: Arc<DistController>) -> Router {
    Router::new()
        .route("/dist/register", post(register_handler))
        .route("/dist/enqueue", post(enqueue_handler))
        .route("/dist/pull", get(pull_handler))
        .route("/dist/report", post(report_handler))
        .route("/dist/job", get(job_status_handler))
        .route("/dist/progress", post(progress_post_handler))
        .route("/dist/progress", get(progress_get_handler))
        .route("/dist/cancel", post(cancel_handler))
        .route("/dist/heartbeat", post(heartbeat_handler))
        .route("/dist/event", post(event_handler))
        // Prometheus scrape 端点。无 token——和 /dist/* 不同，metrics 暴露面
        // 由部署侧（reverse proxy / firewall）控制。本地 dev 直接 curl 即可。
        .route("/metrics", get(metrics_handler))
        .with_state(ctrl)
}

/// 生产路径用的 router——同 `router` 但所有 `/dist/*` 走 HMAC middleware。
///
/// `/metrics` 不在 layer 内：scrape 工具一般不挂签名，部署侧靠 firewall /
/// reverse proxy 限制可见性。要把 metrics 也保护起来，调用方自行二次包一层。
pub fn router_with_hmac(ctrl: Arc<DistController>, gate: crate::dist_auth::HmacGate) -> Router {
    use axum::middleware::from_fn_with_state;
    let dist = Router::new()
        .route("/dist/register", post(register_handler))
        .route("/dist/enqueue", post(enqueue_handler))
        .route("/dist/pull", get(pull_handler))
        .route("/dist/report", post(report_handler))
        .route("/dist/job", get(job_status_handler))
        .route("/dist/progress", post(progress_post_handler))
        .route("/dist/progress", get(progress_get_handler))
        .route("/dist/cancel", post(cancel_handler))
        .route("/dist/heartbeat", post(heartbeat_handler))
        .route("/dist/event", post(event_handler))
        .with_state(ctrl.clone())
        .layer(from_fn_with_state(gate, crate::dist_auth::hmac_layer));
    Router::new()
        .merge(dist)
        .route("/metrics", get(metrics_handler))
        .with_state(ctrl)
}

async fn metrics_handler(State(ctrl): State<Arc<DistController>>) -> impl IntoResponse {
    let body = ctrl.metrics.encode_text();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
}

/// Job.cli 字段映射到 metric label——空串 = 老版默认走 codex，归一便于
/// PromQL 过滤。未来加 cc/gemini 时不需要改这里，原样透传即可。
fn cli_label_of(cli: &str) -> String {
    if cli.is_empty() {
        "codex".to_string()
    } else {
        cli.to_string()
    }
}

/// β · #69 防御性 controller URL 归一化（spec gap controller-url-bug）。
///
/// 干掉历史踩过的两类拼接错误：
/// - 末尾 `/`：`https://x/` → 拼成 `https://x//dist/register`（双斜杠）
/// - 末尾 `/dist`：`https://x/dist` → 拼成 `https://x/dist/dist/register`（双 /dist）
///
/// worker 端 5 个 endpoint 拼 URL（register/heartbeat/pull/report/progress）共用
/// 此 helper：调一次 `normalize_controller_base()` 拿干净 base，后续 `format!`
/// 直接 `{base}/dist/<endpoint>` 即可。
///
/// 选择 trim 顺序：先 `/` → 再 `/dist` → 再 `/`。第二个 `/` 处理 `https://x/dist/`
/// 这种斜杠+后缀+斜杠的组合（先去末 `/` 得 `https://x/dist`，再去 `/dist` 得
/// `https://x`）。
///
/// 不动 `https://x` 的 host 段——只剥**末尾**冗余，不解析 path/query。
pub(crate) fn normalize_controller_base(controller: &str) -> String {
    controller
        .trim_end_matches('/')
        .trim_end_matches("/dist")
        .trim_end_matches('/')
        .to_string()
}

#[derive(Debug, Subcommand)]
pub enum DistCmd {
    /// 往分布式队列派发一个 codex 任务（由远端 worker 拉取执行）
    Enqueue(DistEnqueueArgs),
    /// 在远端节点启动 worker 循环（主动向 controller 拉任务）
    Worker(DistWorkerArgs),
}

#[derive(Debug, ClapArgs)]
pub struct DistEnqueueArgs {
    #[arg(long, default_value = "http://127.0.0.1:4100")]
    pub controller: String,
    #[arg(long)]
    pub token: Option<String>,
    #[arg(long)]
    pub node: String,
    #[arg(long, default_value = "remote-codex-task")]
    pub title: String,
    #[arg(trailing_var_arg = true, required = true)]
    pub body: Vec<String>,
}

// Clone 给 worker 主循环 spawn job task 时用——args 要 move 进 'static task。
#[derive(Debug, Clone, ClapArgs)]
pub struct DistWorkerArgs {
    #[arg(long)]
    pub controller: String,
    #[arg(long)]
    pub node: String,
    #[arg(long)]
    pub token: Option<String>,
    #[arg(long, default_value = "codex")]
    pub codex_bin: String,
    /// claude-code CLI 路径。默认走 PATH 里的 `claude`。
    #[arg(long, default_value = "claude")]
    pub cc_bin: String,
    #[arg(long, default_value_t = 1000)]
    pub poll_ms: u64,
    /// 声明本节点能力（可重复），用于 tag-based 派工。示例：
    /// `--tag home --tag codex --tag gpu`。不传 = 空集（只接无要求的 job）。
    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
    /// 本 worker 允许的最大并发 job 数。默认 1。
    #[arg(long, default_value_t = 1)]
    pub max_concurrency: u32,
    /// Decision 21 phase 3 跨节点 sandbox · ProjectRegistry root 覆盖。
    /// 不传 → `$HOME/.fuxi/projects/`（与 home 端 fuxi-im / CLI 默认对齐）。
    /// 用户在 worker 节点须 pre-`fuxi project add <path>` 注册同名 slug，
    /// 否则 job 带 `project=...` 字段时 worker pull 后 fail job。
    #[arg(long)]
    pub projects_root: Option<PathBuf>,
}

fn resolve_token(token: Option<String>) -> Result<String> {
    if let Some(t) = token
        && !t.is_empty()
    {
        return Ok(t);
    }
    std::env::var(DIST_TOKEN_ENV).with_context(|| {
        format!("missing dist token: pass --token or set ${DIST_TOKEN_ENV} environment variable")
    })
}

pub async fn run_enqueue(args: DistEnqueueArgs) -> Result<()> {
    // CLI 入口也走 HMAC——controller 端 hmac_layer 不区分调用方身份，凡 /dist/*
    // 都过签。token resolve 仍调一次保留早失败：旧 --token / FUXI_DIST_TOKEN
    // 缺失时先报清晰错，避免飞到 controller 才 401。
    let _ = resolve_token(args.token);
    let secret = crate::dist_auth::HmacSecret::from_env()
        .map_err(|e| anyhow!("dist enqueue HMAC secret: {e}"))?;
    let body = args.body.join(" ");
    let client = Client::new();
    // β · #69 normalize（同 run_worker_with）
    let url = format!(
        "{}/dist/enqueue",
        normalize_controller_base(&args.controller)
    );
    let req = DistEnqueueReq {
        node_id: args.node,
        title: args.title,
        body,
        // CLI 入口裸派，不组装 role 心智——gateway agent 路径才会填。
        system_prompt: None,
        // CLI 同样不带 tags / pin——派工走全局 queue，谁空闲谁取。
        // 若真要定点派，用户用 `fuxi spawn --node` 走 gateway 路径。
        required_tags: Vec::new(),
        pinned_node: None,
        // CLI 入口不指定 cli——worker 按默认（codex）跑；若用户就想
        // 在分布式命令行直派 cc，Phase 4b 之后可扩 `fuxi dist enqueue --cli cc`。
        cli: String::new(),
        allowed_tools: Vec::new(),
        // CLI 入口走分布式裸派，无 home 真相 task / role 概念——dist worker 端
        // 自生成 TaskId / fallback role "unknown"。fuxi-im 走 gateway 时 Some。
        task_id: None,
        role: None,
        // CLI 入口不绑项目——`fuxi dist enqueue` 是裸派语义，需要项目 sandbox
        // 时走 gateway 路径（`fuxi spawn --node X --project erp`）。
        project: None,
        ephemeral_task: None,
        // CLI 裸派无 topic 概念——兜底 general（同 task.topic_id=None）。
        topic_id: None,
    };
    let resp = crate::dist_auth_client::signed_post(&client, &secret, &url, &req)
        .await
        .context("dist enqueue request failed")?
        .error_for_status()
        .context("dist enqueue non-2xx")?
        .json::<DistEnqueueResp>()
        .await
        .context("decode dist enqueue response failed")?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({ "job_id": resp.job_id }))?
    );
    Ok(())
}

/// adapter factory 类型——给定 cli 名 + 配置返回 trait object。生产路径
/// 用 `select_adapter` 默认实现；测试可注入快速 mock 验证 worker loop 自身
/// 的并发 / cancel 语义而不真起 codex/cc 子进程。
pub(crate) type AdapterFactory =
    Arc<dyn Fn(&str, &DistWorkerArgs) -> Result<Box<dyn CliAdapter>> + Send + Sync>;

/// 心跳间隔。Decision 12 决议后心跳兼任"静默期 cancel 派送"路径——
/// 抽成常量便于测试用更短间隔避免 1×10s 等待。
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

pub async fn run_worker(args: DistWorkerArgs) -> Result<()> {
    let token = resolve_token(args.token.clone())?;
    let secret = std::sync::Arc::new(
        crate::dist_auth::HmacSecret::from_env().map_err(|e| anyhow!("worker HMAC secret: {e}"))?,
    );
    let factory: AdapterFactory =
        Arc::new(|cli, args| select_adapter(cli, args).map(|a| a as Box<dyn CliAdapter>));
    run_worker_with(args, token, secret, factory, HEARTBEAT_INTERVAL).await
}

pub(crate) fn spawn_embedded_worker(
    ctrl: Arc<DistController>,
    args: DistWorkerArgs,
    token: String,
    secret: std::sync::Arc<crate::dist_auth::HmacSecret>,
) -> tokio::task::JoinHandle<()> {
    let factory: AdapterFactory =
        Arc::new(|cli, args| select_adapter(cli, args).map(|a| a as Box<dyn CliAdapter>));
    spawn_embedded_worker_with(ctrl, args, token, secret, factory, HEARTBEAT_INTERVAL)
}

pub(crate) fn spawn_embedded_worker_with(
    ctrl: Arc<DistController>,
    args: DistWorkerArgs,
    token: String,
    secret: std::sync::Arc<crate::dist_auth::HmacSecret>,
    adapter_factory: AdapterFactory,
    heartbeat_interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(e) = run_embedded_worker_with(
            ctrl,
            args,
            token,
            secret,
            adapter_factory,
            heartbeat_interval,
        )
        .await
        {
            tracing::error!(error = %e, "embedded dist worker exited");
        }
    })
}

async fn run_embedded_worker_with(
    ctrl: Arc<DistController>,
    args: DistWorkerArgs,
    token: String,
    secret: std::sync::Arc<crate::dist_auth::HmacSecret>,
    adapter_factory: AdapterFactory,
    heartbeat_interval: Duration,
) -> Result<()> {
    let controller = normalize_controller_base(&args.controller);
    let client = Client::new();
    ctrl.register(
        args.node.clone(),
        args.tags.clone(),
        args.max_concurrency.max(1),
    )
    .await;

    let inflight: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let bus_client = Arc::new(NetworkBusClient::new(
        client.clone(),
        controller.clone(),
        token.clone(),
        secret.clone(),
        args.node.clone(),
    ));
    let _bus_flush_handle = bus_client.clone().spawn_flush_loop();
    let bus_client = Some(bus_client);

    let mut jobs: JoinSet<()> = JoinSet::new();
    let max_concurrency = args.max_concurrency.max(1) as usize;

    loop {
        while jobs.len() >= max_concurrency {
            tokio::select! {
                _ = tokio::time::sleep(heartbeat_interval) => {
                    embedded_worker_heartbeat(&ctrl, &args.node, &args.tags, args.max_concurrency, &inflight).await;
                }
                _ = jobs.join_next() => {}
            }
        }

        let Some(job) = ctrl.pull(&args.node).await else {
            tokio::time::sleep(Duration::from_millis(args.poll_ms)).await;
            embedded_worker_heartbeat(
                &ctrl,
                &args.node,
                &args.tags,
                args.max_concurrency,
                &inflight,
            )
            .await;
            continue;
        };

        let job_id = job.id.clone();
        let cancel_tok = CancellationToken::new();
        inflight
            .lock()
            .await
            .insert(job_id.clone(), cancel_tok.clone());

        let client_c = client.clone();
        let controller_c = controller.clone();
        let token_c = token.clone();
        let secret_c = secret.clone();
        let node_c = args.node.clone();
        let inflight_c = inflight.clone();
        let factory_c = adapter_factory.clone();
        let args_for_factory = args.clone();
        let projects_root_c = args.projects_root.clone();
        let ctrl_c = ctrl.clone();
        let bus_c = bus_client.clone();
        let started = Instant::now();

        jobs.spawn(async move {
            let ctx = WorkerCtx {
                client: &client_c,
                controller: &controller_c,
                secret: &secret_c,
                token: &token_c,
                node_id: &node_c,
                bus_client: bus_c.as_ref(),
                projects_root: projects_root_c.as_deref(),
            };
            let run_result = match factory_c(&job.cli, &args_for_factory) {
                Ok(adapter) => {
                    tokio::select! {
                        biased;
                        _ = cancel_tok.cancelled() => {
                            Ok((false, "cancelled by controller (heartbeat)".to_string()))
                        }
                        r = adapter.run(&ctx, &job) => r,
                    }
                }
                Err(e) => Err(e),
            };
            let (ok, output) = match run_result {
                Ok(pair) => pair,
                Err(e) => (false, format!("worker run error: {e}")),
            };
            inflight_c.lock().await.remove(&job.id);
            ctrl_c
                .report(DistReportReq {
                    node_id: node_c,
                    job_id: job.id.clone(),
                    ok,
                    output,
                    duration_ms: started.elapsed().as_millis(),
                })
                .await;
        });
    }
}

async fn embedded_worker_heartbeat(
    ctrl: &DistController,
    node_id: &str,
    tags: &[String],
    max_concurrency: u32,
    inflight: &Arc<Mutex<HashMap<String, CancellationToken>>>,
) {
    let mut snapshot: Vec<String> = {
        let g = inflight.lock().await;
        g.keys().cloned().collect()
    };
    if let Some(node) = ctrl
        .nodes_snapshot()
        .await
        .into_iter()
        .find(|n| n.node_id == node_id)
    {
        for id in node.inflight {
            if !snapshot.contains(&id) {
                snapshot.push(id);
            }
        }
    }
    // PR-B：embedded worker 心跳也带 metadata 让 controller 自愈（与 run_worker_with 对称）。
    let metadata = Some(NodeHeartbeatMetadata {
        tags: tags.to_vec(),
        max_concurrency,
    });
    let cancel_pending = ctrl.heartbeat(node_id, snapshot, metadata).await;
    if cancel_pending.is_empty() {
        return;
    }
    let g = inflight.lock().await;
    for jid in cancel_pending {
        if let Some(tok) = g.get(&jid) {
            tok.cancel();
        }
    }
}

/// worker 主循环（Decision 12 真并发版）。
///
/// 与旧版本最大不同：每个 in-flight job 走 `tokio::spawn` + `JoinSet`，
/// 配合 per-job `CancellationToken` 接受心跳 ack 的 `cancel_pending`。
/// `inflight` map 同时承担两个职责：
///   1. 心跳 task 上报 worker 真实状态给 controller（heartbeat req 带的 inflight 列表）
///   2. heartbeat ack 拿到 cancel_pending 时按 job_id 找 token 触发 cancel
///
/// pull 节奏由本地 `jobs.len() < args.max_concurrency` 控制——controller 端
/// 也基于 `node.inflight.len()` 拦截 capacity，两侧冗余但对账靠 heartbeat。
pub(crate) async fn run_worker_with(
    args: DistWorkerArgs,
    token: String,
    secret: std::sync::Arc<crate::dist_auth::HmacSecret>,
    adapter_factory: AdapterFactory,
    heartbeat_interval: Duration,
) -> Result<()> {
    // β · #69 normalize：剥末尾 `/` 与 `/dist` 双重，避免 systemd env 误写
    // 含 `/dist` 后缀时 worker 拼成 `host/dist/dist/register`。
    let controller = normalize_controller_base(&args.controller);
    let client = Client::new();
    let register_url = format!("{controller}/dist/register");
    let register_req = DistRegisterReq {
        node_id: args.node.clone(),
        tags: args.tags.clone(),
        max_concurrency: args.max_concurrency,
    };
    crate::dist_auth_client::signed_post(&client, &secret, &register_url, &register_req)
        .await
        .context("dist register request failed")?
        .error_for_status()
        .context("dist register non-2xx")?;

    let inflight: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // γ: 跨节点 EventBus 桥接客户端。**per-worker 单实例**——所有 in-flight job
    // 共享一个攒批 / retry / drop 队列，避免 per-job spawn flush_loop 的开销和
    // batch 跨 job 边界被冲断（短 job 攒不满 batch_size 又 tick 没到 = 延迟塞家）。
    // flush_loop JoinHandle 当前不显式 shutdown：run_worker_with 是无限 `loop`，
    // 没有 graceful exit 路径；future 加 SIGTERM/CTRL-C 处理时再调
    // `bus_client.shutdown(handle).await`。当前 process exit 直接 abort flush task。
    let bus_client = Arc::new(NetworkBusClient::new(
        client.clone(),
        controller.clone(),
        token.clone(),
        secret.clone(),
        args.node.clone(),
    ));
    let _bus_flush_handle = bus_client.clone().spawn_flush_loop();
    let bus_client = Some(bus_client);

    // 心跳 task：除上报 inflight 外，**消费 ack 的 cancel_pending**——
    // worker 静默执行（无 progress push）时段也能 ~heartbeat interval 内拿到
    // cancel 信号，弥补只靠 push_progress.should_cancel 的盲区。
    {
        let hb_inflight = inflight.clone();
        let hb_node = args.node.clone();
        let hb_tags = args.tags.clone();
        let hb_max_concurrency = args.max_concurrency;
        let hb_controller = controller.clone();
        let hb_client = client.clone();
        let hb_secret = secret.clone();
        let hb_url = format!("{hb_controller}/dist/heartbeat");
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(heartbeat_interval);
            loop {
                tick.tick().await;
                let snapshot: Vec<String> = {
                    let g = hb_inflight.lock().await;
                    g.keys().cloned().collect()
                };
                let req = DistHeartbeatReq {
                    node_id: hb_node.clone(),
                    inflight: snapshot,
                    // PR-B：心跳带 metadata 让 controller 自愈重启——首次 register 后
                    // controller 重启会清掉 in-memory nodes 表，下次心跳即恢复。
                    tags: Some(hb_tags.clone()),
                    max_concurrency: Some(hb_max_concurrency),
                };
                let resp =
                    crate::dist_auth_client::signed_post(&hb_client, &hb_secret, &hb_url, &req)
                        .await;
                let Ok(resp) = resp else { continue };
                let Ok(ack) = resp.json::<DistHeartbeatResp>().await else {
                    continue;
                };
                if ack.cancel_pending.is_empty() {
                    continue;
                }
                // 命中本 worker inflight 的 token 直接 cancel——adapter 外层
                // `select!` 分支会退出，task 走"被取消"final report。未命中
                // 的 job_id 静默忽略（job 已结束 / 从未在本 worker 上跑）。
                let g = hb_inflight.lock().await;
                for jid in ack.cancel_pending {
                    if let Some(tok) = g.get(&jid) {
                        tok.cancel();
                    }
                }
            }
        });
    }

    let mut jobs: JoinSet<()> = JoinSet::new();
    let max_concurrency = args.max_concurrency.max(1) as usize;

    loop {
        // 容量满 → 阻塞在 join_next 直到任一 task 完成；这把"何时 pull 下一个"
        // 与"何时把上一批结果落 controller"自然耦合，不需要额外 throttle。
        while jobs.len() >= max_concurrency {
            let _ = jobs.join_next().await;
        }

        let pull_url = format!("{controller}/dist/pull");
        let pull = crate::dist_auth_client::signed_get(
            &client,
            &secret,
            &pull_url,
            &[("node_id", args.node.as_str())],
        )
        .await;
        let resp = match pull {
            Ok(r) => match r.error_for_status() {
                Ok(ok) => ok,
                Err(e) => {
                    eprintln!("dist worker pull status error: {e}");
                    tokio::time::sleep(Duration::from_millis(args.poll_ms)).await;
                    continue;
                }
            },
            Err(e) => {
                eprintln!("dist worker pull request error: {e}");
                tokio::time::sleep(Duration::from_millis(args.poll_ms)).await;
                continue;
            }
        };

        let payload = match resp.json::<DistPullResp>().await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("dist worker decode pull response error: {e}");
                tokio::time::sleep(Duration::from_millis(args.poll_ms)).await;
                continue;
            }
        };
        let Some(job) = payload.job else {
            tokio::time::sleep(Duration::from_millis(args.poll_ms)).await;
            continue;
        };

        let job_id = job.id.clone();
        let cancel_tok = CancellationToken::new();
        inflight
            .lock()
            .await
            .insert(job_id.clone(), cancel_tok.clone());

        // 把 ctx 字段在 spawn 前 owned 化——WorkerCtx 是借用 view，
        // 跨 task 传所有权才 'static 安全。
        let client_c = client.clone();
        let controller_c = controller.clone();
        let token_c = token.clone();
        let secret_c = secret.clone();
        let node_c = args.node.clone();
        let inflight_c = inflight.clone();
        let factory_c = adapter_factory.clone();
        let args_for_factory = args.clone();
        let projects_root_c = args.projects_root.clone();
        let started = Instant::now();

        let bus_c = bus_client.clone();
        jobs.spawn(async move {
            let ctx = WorkerCtx {
                client: &client_c,
                controller: &controller_c,
                secret: &secret_c,
                token: &token_c,
                node_id: &node_c,
                bus_client: bus_c.as_ref(),
                projects_root: projects_root_c.as_deref(),
            };
            // adapter 构造一旦 fail 就走失败 final report——和老路径行为一致。
            let run_result = match factory_c(&job.cli, &args_for_factory) {
                Ok(adapter) => {
                    tokio::select! {
                        biased;
                        // cancel 优先：避免 token 已 cancel 后还浪费一轮 adapter
                        // 启动开销（spawn child 之类）。
                        _ = cancel_tok.cancelled() => {
                            Ok((false, "cancelled by controller (heartbeat)".to_string()))
                        }
                        r = adapter.run(&ctx, &job) => r,
                    }
                }
                Err(e) => Err(e),
            };
            let (ok, output) = match run_result {
                Ok(pair) => pair,
                Err(e) => (false, format!("worker run error: {e}")),
            };
            // 先从本地 inflight 移除——下次心跳就不会再把 job_id 报给 controller。
            // 再发 final report 给 controller，对 capacity 释放权威。
            inflight_c.lock().await.remove(&job.id);
            let report_url = format!("{controller_c}/dist/report");
            let report_req = DistReportReq {
                node_id: node_c,
                job_id: job.id.clone(),
                ok,
                output,
                duration_ms: started.elapsed().as_millis(),
            };
            let _ = crate::dist_auth_client::signed_post(
                &client_c,
                &secret_c,
                &report_url,
                &report_req,
            )
            .await;
        });
    }
}

/// 根据 job payload 组 codex prompt——role 心智在此 prepend。
///
/// 抽成自由函数方便单测：prompt 组装规则必须与本地 `CodexAgent::task_to_prompt`
/// 对齐（同一 `compose_prompt` helper），否则远端 vs 本地行为分叉。
fn build_codex_prompt_from_job(job: &DistJob) -> String {
    let system = job.system_prompt.as_deref().unwrap_or("");
    fuxi_agent_codex::compose_prompt(system, &job.title, &job.body)
}

/// v2 跨节点 sandbox · best-effort `git fetch origin <branch>`。
///
/// 跨节点真协作的前提：worker 节点开 sandbox 前必须 fetch 一下 home 端 main
/// 的最新 commit，否则起出来的 worktree 是过期 base，跑完 push 回去也是脏 base。
///
/// **不 fail-fast**：fetch 失败（远端不可达 / 网络断 / no remote）只 warn，照常
/// 开 sandbox。理由：worker 可能短暂掉线但本地依旧能干活，硬挂会让"网卡飘"
/// 变成 dispatch 中断；让现有数据下能跑起来，由用户事后 git pull 修。
///
/// `FUXI_DISABLE_PRESPAWN_FETCH=1` 完全禁用（CI / 本地开发时偶尔需要）。
pub(crate) async fn try_fetch_default_branch(canonical: &std::path::Path, branch: &str) -> bool {
    if std::env::var_os("FUXI_DISABLE_PRESPAWN_FETCH").is_some() {
        tracing::debug!(
            path = %canonical.display(),
            branch,
            "FUXI_DISABLE_PRESPAWN_FETCH set, skip pre-spawn git fetch"
        );
        return false;
    }
    let out = match tokio::process::Command::new("git")
        .current_dir(canonical)
        .args(["fetch", "origin", branch])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(
                path = %canonical.display(),
                branch,
                error = %e,
                "pre-spawn git fetch 启动失败（git binary 缺？），继续打开 sandbox"
            );
            return false;
        }
    };
    if !out.status.success() {
        tracing::warn!(
            path = %canonical.display(),
            branch,
            stderr = %String::from_utf8_lossy(&out.stderr).trim(),
            "pre-spawn git fetch 失败（远端不可达 / no remote / 权限），继续打开 sandbox"
        );
        return false;
    }
    tracing::info!(
        path = %canonical.display(),
        branch,
        "pre-spawn git fetch origin/{} 完成", branch
    );
    true
}

/// 跨节点真协作 push back：worker 跑完 task 后把 worktree 当前 branch 推回 origin。
///
/// 没这一步 home 端永远看不到 mac 改的代码——L2 archive 只 rename + prune，
/// L3 sandbox 根本没 task-done hook，跨节点协作就成了"派出去就丢"。
///
/// **不 fail-fast**：push 失败只 warn 继续。理由跟 fetch 对称——worker 短暂掉线
/// （VPN 飘 / ssh tunnel 断）让 dispatch 整挂比"这次没 push 上"代价大；commit
/// 还在 worker 本地，用户可 `ssh worker 'git push'` 手动兜底。
///
/// `FUXI_DISABLE_PUSHBACK=1` 完全禁用（CI / 本地开发时偶尔需要）。
pub(crate) async fn try_push_back_branch(worktree_path: &std::path::Path) -> bool {
    if std::env::var_os("FUXI_DISABLE_PUSHBACK").is_some() {
        tracing::debug!(
            path = %worktree_path.display(),
            "FUXI_DISABLE_PUSHBACK set, skip post-job push back"
        );
        return false;
    }
    let head_out = match tokio::process::Command::new("git")
        .current_dir(worktree_path)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(
                path = %worktree_path.display(),
                error = %e,
                "post-job push back：git rev-parse 启动失败（git binary 缺？）"
            );
            return false;
        }
    };
    if !head_out.status.success() {
        tracing::warn!(
            path = %worktree_path.display(),
            stderr = %String::from_utf8_lossy(&head_out.stderr).trim(),
            "post-job push back：解 HEAD 分支失败，跳过 push"
        );
        return false;
    }
    let branch = String::from_utf8_lossy(&head_out.stdout).trim().to_string();
    if branch.is_empty() || branch == "HEAD" {
        tracing::warn!(
            path = %worktree_path.display(),
            branch = %branch,
            "post-job push back：detached HEAD 或空 branch，跳过 push"
        );
        return false;
    }
    let out = match tokio::process::Command::new("git")
        .current_dir(worktree_path)
        .args(["push", "origin", &branch])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(
                path = %worktree_path.display(),
                branch = %branch,
                error = %e,
                "post-job push back：git push 启动失败"
            );
            return false;
        }
    };
    if !out.status.success() {
        tracing::warn!(
            path = %worktree_path.display(),
            branch = %branch,
            stderr = %String::from_utf8_lossy(&out.stderr).trim(),
            "post-job push back：失败（远端不可达 / no remote / 权限），用户可手动 ssh + git push 兜底"
        );
        return false;
    }
    tracing::info!(
        path = %worktree_path.display(),
        branch = %branch,
        "post-job push back origin/{} 完成", branch
    );
    true
}

/// Decision 21 phase 3 跨节点 sandbox · worker 端按 `job.project` 解出
/// cc/codex 应该 spawn 进哪个目录。
///
/// 行为：
/// - `job.project.is_none()` → `Ok(None)` 不动 cwd（兼容老 job / 跨节点裸派）
/// - `job.project.is_some()` 但 worker 节点没注册同 slug → `Err(...)`
///   （明确报错让 home 用户知道要先在 worker 上 `fuxi project add`，
///   silent fallback 到默认 cwd 会让用户怎么也找不到为什么文件没落对地方）
/// - `job.ephemeral_task.is_some()` → 走 L2：若 worktree 已存在复用，否则 create
/// - 否则走 L3：`PersistentSandboxManager::get_or_create(role)` （幂等）
///
/// `role` 来自 `job.role`，缺则 fallback `"worker"`——L3 sandbox 索引会落到
/// `<root>/<project>/sandboxes/worker/`，仍能跑通但跟 home 端 role 不对齐；
/// 实际生产 home 端 spawn 路径必填 role（gateway 的 cfg.role），不会触发 fallback。
async fn resolve_project_sandbox_cwd(
    projects_root: Option<&std::path::Path>,
    job: &DistJob,
) -> Result<Option<PathBuf>> {
    let Some(project_slug) = job.project.as_deref() else {
        return Ok(None);
    };
    let registry = match projects_root {
        Some(p) => fuxi_workspace::FileSystemProjectRegistry::new(p),
        None => fuxi_workspace::FileSystemProjectRegistry::with_default_root().with_context(
            || "worker 解 project sandbox：FileSystemProjectRegistry default root 拿不到（$HOME 缺？）",
        )?,
    };
    let project_id = fuxi_core::ProjectId::new(project_slug.to_string())
        .with_context(|| format!("worker 解 project sandbox：非法 slug {project_slug:?}"))?;
    let project = registry
        .get(&project_id)
        .await
        .with_context(|| format!("worker registry 查 {project_slug} 失败"))?
        .ok_or_else(|| {
            anyhow!(
                "worker 节点未注册项目 {project_slug}——home 派的 job 带 project={project_slug}，\
                 请先在本机跑 `fuxi project add <path>` 注册同名 slug",
            )
        })?;
    // v2 跨节点 sandbox：在打开 worktree 前先 fetch origin main，让 sandbox 起在
    // home 端最新 base 上。fetch 失败 best-effort（log warn 继续）——worker 短暂
    // 掉线时不要把 dispatch 整挂；离线起的 sandbox 跑完 push 回去时若 base 已过期
    // 走 git refused，那是 git 本身的健全检查，比这里硬挂友好。
    let _ = try_fetch_default_branch(&project.canonical_path, &project.default_branch).await;

    if let Some(task_raw) = job.ephemeral_task.as_deref() {
        let trimmed = task_raw.strip_prefix("task-").unwrap_or(task_raw);
        let task_uuid = uuid::Uuid::parse_str(trimmed)
            .with_context(|| format!("worker 解 ephemeral_task：无效 uuid {task_raw}"))?;
        let task = fuxi_core::TaskId::from(task_uuid);
        let mgr = fuxi_workspace::EphemeralWorkspaceManager::new(project.clone(), registry.root());
        // L2 worktree 复用：同 task 重复 job 命中已有 worktree（home 端 spawn 是
        // per-task 一次，但 worker restart / job 重发 corner 仍可能撞）。先 list_active
        // 找已有；找不到再 create。
        let active = mgr.list_active().await.unwrap_or_default();
        if let Some(handle) = active.into_iter().find(|h| h.task == task) {
            return Ok(Some(handle.workspace_path));
        }
        let handle = mgr
            .create(task)
            .await
            .with_context(|| format!("worker create L2 ephemeral for task {task_raw} 失败"))?;
        Ok(Some(handle.workspace_path))
    } else {
        let role_for_sandbox = job
            .role
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("worker");
        let mgr = fuxi_workspace::PersistentSandboxManager::new(project.clone(), registry.root());
        let handle = mgr.get_or_create(role_for_sandbox).await.with_context(|| {
            format!("worker get_or_create L3 sandbox role={role_for_sandbox} 失败")
        })?;
        Ok(Some(handle.sandbox_path))
    }
}

/// worker 运行上下文——push progress 需要的 HTTP 目标 + （γ）跨节点 EventBus
/// 桥接客户端。
///
/// `bus_client` 是 Option：测试 / 老路径 / 暂未配 controller 跨节点 republish 时
/// 为 None，job 跑完不发任何 fuxi Event 到 home。生产 worker 主循环 Some。
pub(crate) struct WorkerCtx<'a> {
    client: &'a Client,
    controller: &'a str,
    /// HMAC 共享密钥——`flush_progress` 等出站调用经 `signed_post` 走签名。
    /// `Arc` 让跨 task 共享 0 拷贝。
    secret: &'a Arc<crate::dist_auth::HmacSecret>,
    /// 旧字段——生产 authn 已切 HMAC，仅 mock test fixture 兼容；新代码不要读。
    #[allow(dead_code)]
    token: &'a str,
    node_id: &'a str,
    bus_client: Option<&'a Arc<NetworkBusClient>>,
    /// Decision 21 phase 3：ProjectRegistry root 覆盖。`None` → 走默认
    /// `$HOME/.fuxi/projects/`。adapter 接到 `job.project=Some(...)` 时
    /// 用本字段 + `resolve_project_sandbox_cwd` 解出 cc/codex 应起的 cwd。
    projects_root: Option<&'a std::path::Path>,
}

/// 抽象 worker 端的 CLI 执行器——让 codex / claude-code / 未来 gemini 等都能
/// 平行实装同一个 trait，worker 主循环按 job.cli 字段选择具体 adapter。
///
/// 跨域协作 + 多 CLI 通用性是伏羲愿景的两支——这个 trait 是通用性的落点；
/// 跨域那条（HTTPS + token）通过现有 register/pull 协议已就绪。
///
/// `run` 合约：
/// - 期间可 push progress chunks 到 `ctx.controller`（通过 `flush_progress`）
/// - 返回 `(ok, final_output)`——`ok=false` 走失败 report，`final_output` 给
///   `/dist/job` 非流式消费者兜底（比如纯 curl 用户）
/// - 长耗时应定期 flush 不憋在本地 buffer，体现 progress
/// - 对 `ProgressAck.should_cancel=true` 要响应：杀 child、走 ok=false
///   "cancelled" 的终态
#[async_trait::async_trait]
pub(crate) trait CliAdapter: Send + Sync {
    /// 名称要和 `DistJob.cli` 字段对齐。当前支持 `"codex"`；`"claude-code"`
    /// 由 Phase 4c 的 `CcAdapter` 接入。
    // bin target 下 run_worker 只调 run(); name() 供测试 / 未来日志和 4b
    // 的 route-by-cli 用。
    #[allow(dead_code)]
    fn name(&self) -> &'static str;

    async fn run(&self, ctx: &WorkerCtx<'_>, job: &DistJob) -> Result<(bool, String)>;
}

/// codex CLI 的 adapter——包装原有 `run_codex_job` 实现。bin 字段支持非
/// PATH 定位，或用户提供替代 codex wrapper（比如 rustls 兼容的 fork）。
struct CodexAdapter {
    bin: String,
}

impl CodexAdapter {
    fn new(bin: String) -> Self {
        Self { bin }
    }
}

#[async_trait::async_trait]
impl CliAdapter for CodexAdapter {
    fn name(&self) -> &'static str {
        "codex"
    }

    async fn run(&self, ctx: &WorkerCtx<'_>, job: &DistJob) -> Result<(bool, String)> {
        run_codex_job(ctx, &self.bin, job).await
    }
}

/// Claude Code CLI 的 adapter（Phase 4c MVP）。
///
/// 走 `claude -p "<prompt>" --output-format stream-json --verbose` 模式——
/// **不**走 `--sdk-url` WS 反连（避 Clash TUN 把本机 loopback 吞了的坑）。
/// 代价是不支持 follow-up / resume（one-shot per job），但分布式场景下
/// 多轮对话优先在本机跑、分布式下更常见的是 rubber-duck / code review /
/// summarize 这类单轮任务——MVP 覆盖足够。
struct CcAdapter {
    bin: String,
}

impl CcAdapter {
    fn new(bin: String) -> Self {
        Self { bin }
    }
}

#[async_trait::async_trait]
impl CliAdapter for CcAdapter {
    fn name(&self) -> &'static str {
        "claude-code"
    }

    async fn run(&self, ctx: &WorkerCtx<'_>, job: &DistJob) -> Result<(bool, String)> {
        run_cc_job(ctx, &self.bin, job).await
    }
}

/// 按 job.cli / role metadata 选择 adapter。未知 CLI 直接报错——worker 会把
/// 这个当 job 失败走 report，避免无限 retry。
///
/// 当前只支持 `"codex"`；`""` 作 legacy 默认（老版 gateway 不填 `cli` 字段
/// 时 serde 会给空串），也走 codex。
fn select_adapter(cli: &str, args: &DistWorkerArgs) -> Result<Box<dyn CliAdapter>> {
    match cli {
        "codex" | "" => Ok(Box::new(CodexAdapter::new(args.codex_bin.clone()))),
        "claude-code" => Ok(Box::new(CcAdapter::new(args.cc_bin.clone()))),
        other => Err(anyhow!(
            "worker 未装载 CLI adapter: {other:?}（当前支持 codex、claude-code；gemini 等待扩）"
        )),
    }
}

/// 攒够这么多条就 flush 一次（无论时钟）。
const PROGRESS_FLUSH_BATCH: usize = 4;
/// 或者攒够这个时间就 flush——即使没满 batch。
const PROGRESS_FLUSH_INTERVAL_MS: u64 = 200;

/// Flush 一批 progress chunk 到 controller。返回 true 表示 controller 告知
/// 该 job 已被 cancel，worker 应立即杀 child 并走终止 report。
async fn flush_progress(ctx: &WorkerCtx<'_>, job_id: &str, chunks: Vec<ProgressPush>) -> bool {
    if chunks.is_empty() {
        return false;
    }
    let url = format!("{}/dist/progress", ctx.controller);
    let req = DistProgressReq {
        node_id: ctx.node_id.to_string(),
        job_id: job_id.to_string(),
        chunks,
    };
    let resp = crate::dist_auth_client::signed_post(ctx.client, ctx.secret, &url, &req).await;
    match resp {
        Ok(r) if r.status().is_success() => r
            .json::<DistProgressAck>()
            .await
            .map(|ack| ack.should_cancel)
            .unwrap_or(false),
        _ => false,
    }
}

/// codex wire event → progress push（`None` 表示此事件不该上报给 gateway）。
///
/// 映射策略：
/// - `AgentMessage` → `AssistantText`（模型对用户的回复文本）
/// - `CommandStarted` / `CommandCompleted` → `ToolCall`
/// - `Error` / `TurnFailed` → `Error`
/// - `ItemOther` 的 completed 阶段 → `Thinking`（reasoning 等）
/// - 其他协议级事件（ThreadStarted / TurnStarted / TurnCompleted / Unknown）静默
fn codex_event_to_push(ev: &CodexEvent) -> Option<ProgressPush> {
    match ev {
        CodexEvent::AgentMessage { text, .. } => Some(ProgressPush {
            kind: ProgressKind::AssistantText,
            text: text.clone(),
        }),
        CodexEvent::CommandStarted { command, .. } => Some(ProgressPush {
            kind: ProgressKind::ToolCall,
            text: format!("$ {command}"),
        }),
        CodexEvent::CommandCompleted {
            command,
            exit_code,
            output_preview,
            ..
        } => {
            let marker = match exit_code {
                Some(0) | None => String::new(),
                Some(n) => format!(" [exit={n}]"),
            };
            let preview: String = output_preview.chars().take(400).collect();
            let text = if preview.trim().is_empty() {
                format!("$ {command}{marker}")
            } else {
                format!("$ {command}{marker}\n{preview}")
            };
            Some(ProgressPush {
                kind: ProgressKind::ToolCall,
                text,
            })
        }
        CodexEvent::Error { message } => Some(ProgressPush {
            kind: ProgressKind::Error,
            text: message.clone(),
        }),
        CodexEvent::TurnFailed { reason } => Some(ProgressPush {
            kind: ProgressKind::Error,
            text: reason.clone(),
        }),
        CodexEvent::ItemOther {
            phase: ItemPhase::Completed,
            item_type,
            ..
        } => Some(ProgressPush {
            kind: ProgressKind::Thinking,
            text: format!("[{item_type}] completed"),
        }),
        _ => None,
    }
}

/// γ：测试 helper——封装 run_codex_job 内 splice 的 parse + translate + enqueue 三步，
/// 让 TDD 不必起 fake codex 子进程也能验证桥接行为。
///
/// 生产路径是 run_codex_job 循环里**内联**的同三步（不调本 fn——避免双消费同一行）。
/// 改桥接行为时同步改两处；clippy 警告"函数已存在两份近似实现"是预期。
#[cfg(test)]
async fn codex_publish_line(
    bus: &NetworkBusClient,
    line: &str,
    agent_id: AgentId,
    task_id: Option<TaskId>,
    state: &mut fuxi_agent_codex::TranslateState,
    pid_hint: Option<u32>,
) {
    let Ok(ev) = fuxi_agent_codex::parse_line(line) else {
        return;
    };
    for event in fuxi_agent_codex::translate(ev, agent_id, task_id, state, pid_hint) {
        let _ = bus.enqueue(event).await;
    }
}

/// γ：cc 路对称测试 helper——见 codex_publish_line 注释。
#[cfg(test)]
async fn cc_publish_line(
    bus: &NetworkBusClient,
    line: &str,
    agent_id: AgentId,
    task_id: Option<TaskId>,
    state: &mut fuxi_agent_cc::TranslateState,
    pid_hint: Option<u32>,
) {
    let Ok(ev) = fuxi_agent_cc::parse_line(line) else {
        return;
    };
    for event in fuxi_agent_cc::translate(ev, agent_id, task_id, state, pid_hint) {
        let _ = bus.enqueue(event).await;
    }
}

/// 流式执行 codex 任务：spawn + 按行读 stdout + 增量 POST progress。
///
/// 返回 `(ok, final_output)`——外层负责发 `/dist/report`。`final_output` 是
/// 整轮回复的文本汇总（给老 `/dist/job` 非流式消费者兜底），已做长度截断。
async fn run_codex_job(
    ctx: &WorkerCtx<'_>,
    codex_bin: &str,
    job: &DistJob,
) -> Result<(bool, String)> {
    let prompt = build_codex_prompt_from_job(job);
    let cfg = fuxi_agent_codex::CodexLaunchConfig {
        binary: codex_bin.to_string(),
        ..Default::default()
    };
    let mut args = cfg.build_args();
    args.push(prompt);

    // Decision 21 phase 3：项目 sandbox 解析——按 job.project 找 worker 节点
    // ProjectRegistry 中同 slug 的 sandbox/L2 worktree，把 codex spawn 进去。
    // 缺 project / 已注册 → cwd None / 真路径；查不到 / 解析失败 → fail job
    // 让 home 用户明确知道是 worker 节点配置问题（vs silent fallback 后用户找
    // 不到为什么文件没落对地方）。
    let project_cwd = resolve_project_sandbox_cwd(ctx.projects_root, job).await?;

    let mut cmd = Command::new(codex_bin);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 与 cc 路径对齐（run_cc_job）：外层 tokio::select! 在心跳 ack cancel
        // 时 drop 整个 future，若不 kill_on_drop，tokio Command 只释放句柄，
        // 子 codex 进程会变僵尸。Decision 12 的 cancel 路径靠这条在 OS 层兜底。
        .kill_on_drop(true);
    if let Some(cwd) = project_cwd.as_ref() {
        cmd.current_dir(cwd);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn codex binary failed: {codex_bin}"))?;

    // γ：per-job 子门客 identity + parser state。
    // - agent_id：home 没有这个远端 sub-agent 的视图，新造一个 worker 侧 id；
    //   home 端 TUI 看到的是 "(unknown agent on node X)" 直到 δ 加上 source_node_id
    //   能把 worker node + agent_id 渲染成可识别名字。
    // - task_id：#76 修——优先用 home 真相 task_id，让事件 meta.task 跟 home
    //   端 dispatch 时记的一致；无值才 fallback worker-local。
    // - translate_state：cc/codex 都有跨事件状态（thinking 块 / responded_this_turn /
    //   last_agent_message）。**per-job 新建 = job 边界即状态边界**——上 job 残留
    //   的 responded_this_turn 不会污染下 job 的冷场景 result-only 回复。
    let job_agent_id = AgentId::new();
    let job_task_id = job
        .task_id
        .as_deref()
        .and_then(|s| {
            s.strip_prefix("task-")
                .unwrap_or(s)
                .parse::<uuid::Uuid>()
                .ok()
        })
        .map(TaskId::from)
        .or_else(|| Some(TaskId::new()));
    // FU-2 跨节点收尾：同 cc 路径——解 job.topic_id 给 worker 事件 stamp meta.topic_id。
    let job_topic_id = job
        .topic_id
        .as_deref()
        .and_then(|s| s.parse::<uuid::Uuid>().ok())
        .map(fuxi_core::TopicId);
    let pid_hint = child.id();
    let mut translate_state = fuxi_agent_codex::TranslateState::new();

    // #77：worker spawn codex 后 publish AgentSpawning（同 cc 路径）。
    if let Some(bus) = ctx.bus_client {
        let role = job.role.clone().unwrap_or_else(|| "unknown".to_string());
        let mut meta = fuxi_core::event::EventMeta::now();
        meta.agent = Some(job_agent_id);
        meta.task = job_task_id;
        meta.topic_id = job_topic_id;
        let _ = bus
            .enqueue(fuxi_core::event::Event {
                meta,
                kind: fuxi_core::event::EventKind::AgentSpawning {
                    role,
                    cli: "codex".to_string(),
                },
            })
            .await;
    }

    let stdout = child.stdout.take().context("codex stdout pipe missing")?;
    let stderr = child.stderr.take();
    let mut reader = BufReader::new(stdout).lines();

    let mut buffer: Vec<ProgressPush> = Vec::new();
    let mut last_flush = Instant::now();
    let mut final_text = String::new();
    let mut got_error = false;
    let mut got_cancel = false;

    loop {
        let line_res = tokio::time::timeout(
            Duration::from_millis(PROGRESS_FLUSH_INTERVAL_MS),
            reader.next_line(),
        )
        .await;

        let mut eof = false;
        match line_res {
            Ok(Ok(Some(line))) => {
                if !line.trim().is_empty()
                    && let Ok(ev) = fuxi_agent_codex::parse_line(&line)
                {
                    if let Some(push) = codex_event_to_push(&ev) {
                        if matches!(push.kind, ProgressKind::AssistantText) {
                            if !final_text.is_empty() {
                                final_text.push('\n');
                            }
                            final_text.push_str(&push.text);
                        }
                        if matches!(push.kind, ProgressKind::Error) {
                            got_error = true;
                        }
                        buffer.push(push);
                    }
                    // γ：第二个消费者——translate 同一个 CodexEvent 成 fuxi Event 灌
                    // 跨节点 bus。push_progress（gateway 兜底）和 bus（home 实时订阅）
                    // 共享一次 parse_line 结果，state 跨调用累积（per-job）。
                    if let Some(bus) = ctx.bus_client {
                        for mut event in fuxi_agent_codex::translate(
                            ev,
                            job_agent_id,
                            job_task_id,
                            &mut translate_state,
                            pid_hint,
                        ) {
                            // FU-2：worker 事件归位发起 topic（translate 不知 topic）。
                            if event.meta.topic_id.is_none() {
                                event.meta.topic_id = job_topic_id;
                            }
                            let _ = bus.enqueue(event).await;
                        }
                    }
                }
            }
            Ok(Ok(None)) => eof = true,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "codex stdout read failed");
                eof = true;
            }
            Err(_) => { /* 200ms timeout —— 落盘机会 */ }
        }

        let due_to_batch = buffer.len() >= PROGRESS_FLUSH_BATCH;
        let due_to_time = !buffer.is_empty()
            && last_flush.elapsed() >= Duration::from_millis(PROGRESS_FLUSH_INTERVAL_MS);
        if due_to_batch || due_to_time || (eof && !buffer.is_empty()) {
            let batch = std::mem::take(&mut buffer);
            let cancel = flush_progress(ctx, &job.id, batch).await;
            last_flush = Instant::now();
            if cancel {
                got_cancel = true;
                let _ = child.start_kill();
                break;
            }
        }
        if eof {
            break;
        }
    }

    let status = child.wait().await.context("waiting codex child")?;
    let stderr_text = match stderr {
        Some(mut se) => {
            let mut s = String::new();
            let _ = se.read_to_string(&mut s).await;
            s
        }
        None => String::new(),
    };

    let ok = status.success() && !got_error && !got_cancel;
    let output = if got_cancel {
        if final_text.trim().is_empty() {
            "cancelled by controller".to_string()
        } else {
            format!(
                "cancelled by controller\n---\n{}",
                truncate_text(&final_text, 800)
            )
        }
    } else if !final_text.trim().is_empty() {
        truncate_text(&final_text, 1200)
    } else if !stderr_text.trim().is_empty() {
        truncate_text(stderr_text.trim(), 1200)
    } else {
        // 兜底：既没 AgentMessage 也没 stderr，留个 status 线索
        format!("codex exited with {status}")
    };

    // v2 跨节点 push back：worker 跑完把 worktree 当前 branch 推回 home，否则
    // home 端永远看不到 mac 改的代码（L2 archive 只 rename + prune；L3 无 hook）。
    // 失败 best-effort log warn，不影响 task 报告。无 project_cwd（裸派）跳过。
    if let Some(cwd) = project_cwd.as_ref() {
        let _ = try_push_back_branch(cwd).await;
    }

    Ok((ok, output))
}

/// cc stream-json 事件 → progress push。
///
/// 关键映射：
/// - `AssistantText` → `AssistantText`（主回复）
/// - `AssistantThinking` → `Thinking`（每条原样推；前端聚合视觉由 TUI rail 做）
/// - `AssistantToolUse` → `ToolCall`（"<tool_name> <input-brief>"）
/// - `UserToolResult` → `ToolCall` 或 `Error`（按 is_error 分）
/// - `ResultError` → `Error`
/// - `ResultSuccess` → `None`——文本已经在 AssistantText 发过，不重复
/// - `SystemInit` / `SystemOther` / `RateLimit` / `Unknown` → `None`（协议噪音）
fn cc_event_to_push(ev: &fuxi_agent_cc::CcEvent) -> Option<ProgressPush> {
    use fuxi_agent_cc::CcEvent;
    match ev {
        CcEvent::AssistantText { text } => Some(ProgressPush {
            kind: ProgressKind::AssistantText,
            text: text.clone(),
        }),
        CcEvent::AssistantThinking { text } => Some(ProgressPush {
            kind: ProgressKind::Thinking,
            text: text.clone(),
        }),
        CcEvent::AssistantToolUse {
            tool_name, input, ..
        } => {
            let brief: String = serde_json::to_string(input)
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect();
            Some(ProgressPush {
                kind: ProgressKind::ToolCall,
                text: if brief.is_empty() {
                    tool_name.clone()
                } else {
                    format!("{tool_name} {brief}")
                },
            })
        }
        CcEvent::UserToolResult {
            is_error,
            content_preview,
            ..
        } => {
            let preview: String = content_preview.chars().take(400).collect();
            Some(ProgressPush {
                kind: if *is_error {
                    ProgressKind::Error
                } else {
                    ProgressKind::ToolCall
                },
                text: if preview.trim().is_empty() {
                    "(empty)".to_string()
                } else {
                    preview
                },
            })
        }
        CcEvent::ResultError { reason, .. } => Some(ProgressPush {
            kind: ProgressKind::Error,
            text: reason.clone(),
        }),
        CcEvent::ResultSuccess { .. }
        | CcEvent::SystemInit { .. }
        | CcEvent::SystemOther { .. }
        | CcEvent::RateLimit { .. }
        | CcEvent::Unknown { .. } => None,
    }
}

/// 流式执行 claude-code 任务：spawn `claude -p` + stdout stream-json 行读。
async fn run_cc_job(ctx: &WorkerCtx<'_>, bin: &str, job: &DistJob) -> Result<(bool, String)> {
    let prompt = if job.body.trim().is_empty() {
        job.title.clone()
    } else {
        format!("{}\n\n{}", job.title, job.body)
    };

    let mut args: Vec<String> = vec![
        "-p".into(),
        prompt,
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--permission-mode".into(),
        "bypassPermissions".into(),
        "--dangerously-skip-permissions".into(),
        "--no-session-persistence".into(),
    ];
    if let Some(sp) = job.system_prompt.as_deref()
        && !sp.trim().is_empty()
    {
        args.push("--append-system-prompt".into());
        args.push(sp.to_string());
    }
    if !job.allowed_tools.is_empty() {
        args.push("--allowed-tools".into());
        args.push(job.allowed_tools.join(","));
    }

    // Decision 21 phase 3：项目 sandbox 解析——同 codex 路径，按 job.project
    // 解出 cc 应该住哪个目录。详见 resolve_project_sandbox_cwd 的 doc。
    let project_cwd = resolve_project_sandbox_cwd(ctx.projects_root, job).await?;

    let mut cmd = Command::new(bin);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 两个坑都要避——参照 fuxi_agent_cc::spawn 的做法：
        // 1. Clash/Surge TUN 可能代理 loopback（即便我们不反连 WS，cc 自己
        //    可能也有 telemetry loopback），NO_PROXY 保险
        // 2. 嵌套检测（父 cc 起子 cc）
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("no_proxy", "127.0.0.1,localhost")
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .env_remove("CLAUDE_CODE_NO_FLICKER")
        .env_remove("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS")
        .env_remove("CLAUDE_CODE_EXECPATH")
        .kill_on_drop(true);
    if let Some(cwd) = project_cwd.as_ref() {
        cmd.current_dir(cwd);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn claude binary failed: {bin}"))?;

    // γ：per-job 子门客 identity + parser state——见 run_codex_job 同段注释。
    // #76：优先用 home 真相 task_id（job.task_id），让 worker 端 cc 跑出来的
    // events.meta.task 跟 home 端 dispatch 时记的 task.id 一致——/api/tasks
    // aggregate 才能把"home 端发的 TaskCreated"和"远端 cc 跑的 agent_responded"
    // 拼到同一条 task 卡片上。无 task_id（老 gateway / cli enqueue 路径）回落
    // worker 自生成。
    let job_agent_id = AgentId::new();
    let job_task_id = job
        .task_id
        .as_deref()
        .and_then(|s| {
            s.strip_prefix("task-")
                .unwrap_or(s)
                .parse::<uuid::Uuid>()
                .ok()
        })
        .map(TaskId::from)
        .or_else(|| Some(TaskId::new()));
    // FU-2 跨节点收尾：解 job.topic_id → 给 worker 跑出来的事件 stamp meta.topic_id，
    // 跟本地适配器口径一致，home bridge 才能把完工路由回归属 topic 分身（不串 general）。
    let job_topic_id = job
        .topic_id
        .as_deref()
        .and_then(|s| s.parse::<uuid::Uuid>().ok())
        .map(fuxi_core::TopicId);
    let pid_hint = child.id();
    let mut translate_state = fuxi_agent_cc::TranslateState::new();

    // #77：worker spawn cc 即刻 publish AgentSpawning + AgentReady 给 home bus，
    // 让 home aggregate 能查到此 agent 的 role（不 fallback "unknown"）。home
    // 端发起 dispatch 时已知道 role（profile.role），透传到 job.role；这里
    // 没值时 fallback "unknown" 跟旧行为对齐。
    if let Some(bus) = ctx.bus_client {
        let role = job.role.clone().unwrap_or_else(|| "unknown".to_string());
        let mut meta = fuxi_core::event::EventMeta::now();
        meta.agent = Some(job_agent_id);
        meta.task = job_task_id;
        meta.topic_id = job_topic_id;
        let _ = bus
            .enqueue(fuxi_core::event::Event {
                meta,
                kind: fuxi_core::event::EventKind::AgentSpawning {
                    role,
                    cli: "claude-code".to_string(),
                },
            })
            .await;
    }

    let stdout = child.stdout.take().context("cc stdout pipe missing")?;
    let stderr = child.stderr.take();
    let mut reader = BufReader::new(stdout).lines();

    let mut buffer: Vec<ProgressPush> = Vec::new();
    let mut last_flush = Instant::now();
    let mut final_text = String::new();
    let mut got_error = false;
    let mut got_cancel = false;
    // #79：track cc 是否真发了 sentinel JSON——cc haiku model 实测不可靠遵守
    // addendum 文案。worker 跑完时若没观察到 AgentRequestReview，就兜底发一条
    // research_summary（summary 取 cc final text 前 200 字符）让玄女知道交付。
    // dist 路径 worker 跑 raw cc 没人格，always-nudge 是合理默认（不违 Decision 13
    // "门客自决"——dist worker 本身就不是"门客"，是个跑 cc 的搬运工）。
    let mut cc_emitted_review = false;

    loop {
        let line_res = tokio::time::timeout(
            Duration::from_millis(PROGRESS_FLUSH_INTERVAL_MS),
            reader.next_line(),
        )
        .await;

        let mut eof = false;
        match line_res {
            Ok(Ok(Some(line))) => {
                if !line.trim().is_empty()
                    && let Ok(ev) = fuxi_agent_cc::parse_line(&line)
                {
                    if let Some(push) = cc_event_to_push(&ev) {
                        if matches!(push.kind, ProgressKind::AssistantText) {
                            if !final_text.is_empty() {
                                final_text.push('\n');
                            }
                            final_text.push_str(&push.text);
                        }
                        if matches!(push.kind, ProgressKind::Error) {
                            got_error = true;
                        }
                        buffer.push(push);
                    }
                    // γ：第二个消费者——translate 同一 CcEvent 灌跨节点 bus。
                    if let Some(bus) = ctx.bus_client {
                        for mut event in fuxi_agent_cc::translate(
                            ev,
                            job_agent_id,
                            job_task_id,
                            &mut translate_state,
                            pid_hint,
                        ) {
                            // FU-2：worker 跑出来的事件归位发起 topic（translate 不知 topic）。
                            if event.meta.topic_id.is_none() {
                                event.meta.topic_id = job_topic_id;
                            }
                            // #79：track cc 是否真发了 sentinel——没发的话末尾兜底。
                            if matches!(
                                event.kind,
                                fuxi_core::event::EventKind::AgentRequestReview { .. }
                            ) {
                                cc_emitted_review = true;
                            }
                            let _ = bus.enqueue(event).await;
                        }
                    }
                }
            }
            Ok(Ok(None)) => eof = true,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "cc stdout read failed");
                eof = true;
            }
            Err(_) => {}
        }

        let due_to_batch = buffer.len() >= PROGRESS_FLUSH_BATCH;
        let due_to_time = !buffer.is_empty()
            && last_flush.elapsed() >= Duration::from_millis(PROGRESS_FLUSH_INTERVAL_MS);
        if due_to_batch || due_to_time || (eof && !buffer.is_empty()) {
            let batch = std::mem::take(&mut buffer);
            let cancel = flush_progress(ctx, &job.id, batch).await;
            last_flush = Instant::now();
            if cancel {
                got_cancel = true;
                let _ = child.start_kill();
                break;
            }
        }
        if eof {
            break;
        }
    }

    let status = child.wait().await.context("waiting cc child")?;
    let stderr_text = match stderr {
        Some(mut se) => {
            let mut s = String::new();
            let _ = se.read_to_string(&mut s).await;
            s
        }
        None => String::new(),
    };

    let ok = status.success() && !got_error && !got_cancel;
    let output = if got_cancel {
        if final_text.trim().is_empty() {
            "cancelled by controller".to_string()
        } else {
            format!(
                "cancelled by controller\n---\n{}",
                truncate_text(&final_text, 800)
            )
        }
    } else if !final_text.trim().is_empty() {
        truncate_text(&final_text, 1200)
    } else if !stderr_text.trim().is_empty() {
        truncate_text(stderr_text.trim(), 1200)
    } else {
        format!("cc exited with {status}")
    };

    // #79：cc 没主动发 sentinel 但 task 跑 ok——worker 兜底发 AgentRequestReview
    // 让玄女知道交付了。kind 默认 research_summary（最广义），summary 用 final
    // assistant text 前 200 chars。无 final_text（极端 case）写"任务已完成"占位。
    if ok
        && !cc_emitted_review
        && let Some(bus) = ctx.bus_client
        && let Some(t) = job_task_id
    {
        let summary = if final_text.trim().is_empty() {
            format!("任务「{}」已完成", job.title)
        } else {
            truncate_text(final_text.trim(), 200)
        };
        let mut meta = fuxi_core::event::EventMeta::now();
        meta.agent = Some(job_agent_id);
        meta.task = Some(t);
        meta.topic_id = job_topic_id;
        let _ = bus
            .enqueue(fuxi_core::event::Event {
                meta,
                kind: fuxi_core::event::EventKind::AgentRequestReview {
                    agent: job_agent_id,
                    task: t,
                    deliverable_kind: fuxi_core::event::DeliverableKind::ResearchSummary,
                    summary,
                    artifact_ref: None,
                },
            })
            .await;
        tracing::info!(
            job_id = %job.id,
            task = %t,
            "worker-side always-nudge：cc 未输出 sentinel，兜底发 AgentRequestReview"
        );
    }

    // v2 跨节点 push back：与 codex 路径同——把 worktree 当前 branch 推回 home，
    // 让 home 端 task 终态后能 `git log origin/task/<uuid>` 看到 worker 推的 commit。
    if let Some(cwd) = project_cwd.as_ref() {
        let _ = try_push_back_branch(cwd).await;
    }

    Ok((ok, output))
}

fn truncate_text(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars().take(max.saturating_sub(1)) {
        out.push(ch);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// β · #69 normalize 4 路 spec 契约。
    #[test]
    fn normalize_controller_base_strips_trailing_slash_and_dist() {
        // 干净（不动）
        assert_eq!(
            normalize_controller_base("https://x"),
            "https://x".to_string()
        );
        // 末尾 /
        assert_eq!(
            normalize_controller_base("https://x/"),
            "https://x".to_string()
        );
        // 末尾 /dist
        assert_eq!(
            normalize_controller_base("https://x/dist"),
            "https://x".to_string()
        );
        // 末尾 /dist/
        assert_eq!(
            normalize_controller_base("https://x/dist/"),
            "https://x".to_string()
        );
        // host:port + /dist 后缀（实战 systemd 常见误写）
        assert_eq!(
            normalize_controller_base("https://im.qmledmq.cn:8443/dist"),
            "https://im.qmledmq.cn:8443".to_string()
        );
        // path 中段含 dist 不动（只剥末尾）
        assert_eq!(
            normalize_controller_base("https://x/dist/proxy"),
            "https://x/dist/proxy".to_string()
        );
    }

    #[test]
    fn normalize_controller_base_concat_yields_clean_register_url() {
        // 关键不变量：normalize 后拼 `{base}/dist/<endpoint>` 不会双 `/dist`
        for input in [
            "https://x",
            "https://x/",
            "https://x/dist",
            "https://x/dist/",
        ] {
            let base = normalize_controller_base(input);
            let url = format!("{base}/dist/register");
            assert_eq!(
                url, "https://x/dist/register",
                "input {input:?} 拼 register 应一致"
            );
        }
    }

    fn job_for(title: &str, body: &str, system: Option<&str>) -> DistJob {
        DistJob {
            id: "job-test".into(),
            node_id: "nodeA".into(),
            title: title.into(),
            body: body.into(),
            created_at: 0,
            system_prompt: system.map(ToOwned::to_owned),
            required_tags: vec![],
            pinned_node: None,
            cli: String::new(),
            allowed_tools: vec![],
            task_id: None,
            role: None,
            project: None,
            ephemeral_task: None,
            topic_id: None,
        }
    }

    /// FU-2 跨节点收尾：`DistJob.topic_id` 是新加的持久化字段，必须 `#[serde(default)]`
    /// 兼容旧库/旧 worker 的 JSON（无此字段）——否则 controller 重启读旧 dist_jobs 行
    /// 或老 worker pull 时反序列化全炸（CLAUDE.md「加字段必 serde default」教训）。
    #[test]
    fn distjob_topic_id_serde_default_compat() {
        // 老 DistJob JSON（无 topic_id）→ 反序列化为 None，不炸。
        let legacy = r#"{"id":"job-x","node_id":"n","title":"t","body":"b","created_at":0}"#;
        let job: DistJob =
            serde_json::from_str(legacy).expect("老 DistJob 无 topic_id 字段应兼容反序列化");
        assert_eq!(job.topic_id, None, "缺字段应回落 None");

        // 带 topic_id 的 round-trip 保形。
        let mut j2 = job.clone();
        j2.topic_id = Some("1bf5390e-fdd4-4647-b976-84705dc0d735".into());
        let s = serde_json::to_string(&j2).unwrap();
        let back: DistJob = serde_json::from_str(&s).unwrap();
        assert_eq!(
            back.topic_id.as_deref(),
            Some("1bf5390e-fdd4-4647-b976-84705dc0d735")
        );
    }

    // ─── Decision 21 phase 3 跨节点 sandbox 解析 ─────────────────

    fn init_repo(path: &std::path::Path) {
        std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(path)
            .status()
            .expect("git init");
        std::process::Command::new("git")
            .args(["config", "user.email", "t@t"])
            .current_dir(path)
            .status()
            .expect("git config email");
        std::process::Command::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(path)
            .status()
            .expect("git config name");
        std::fs::write(path.join("README.md"), "x").expect("write");
        std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(path)
            .status()
            .expect("git add");
        std::process::Command::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(path)
            .status()
            .expect("git commit");
    }

    /// `job.project=None` → 不动 cwd，返 Ok(None)。
    #[tokio::test]
    async fn resolve_project_sandbox_cwd_none_when_job_has_no_project() {
        let job = job_for("t", "b", None);
        let cwd = resolve_project_sandbox_cwd(None, &job).await.expect("ok");
        assert!(cwd.is_none(), "无 project 时不该返 cwd");
    }

    /// `job.project=Some("erp")` 但 worker 无注册 → 明确 Err，不 silent fallback。
    #[tokio::test]
    async fn resolve_project_sandbox_cwd_errors_when_worker_unregistered() {
        let dir = tempfile::tempdir().expect("tmp");
        let registry_root = dir.path().join("registry");
        std::fs::create_dir_all(&registry_root).expect("mkdir registry");

        let mut job = job_for("t", "b", None);
        job.project = Some("erp".into());

        let err = resolve_project_sandbox_cwd(Some(&registry_root), &job)
            .await
            .expect_err("应报 worker 未注册");
        let msg = err.to_string();
        assert!(
            msg.contains("worker 节点未注册项目 erp") || msg.contains("erp"),
            "err 应明确提示 erp 未注册：{msg}"
        );
    }

    /// `job.project=Some` + role=Some → 走 L3：返 sandbox 路径，物理已 create。
    #[tokio::test]
    async fn resolve_project_sandbox_cwd_returns_l3_path_for_role() {
        let dir = tempfile::tempdir().expect("tmp");
        let registry_root = dir.path().join("registry");
        let project_root = dir.path().join("erp-src");
        std::fs::create_dir_all(&project_root).expect("mkdir project");
        init_repo(&project_root);

        let registry = fuxi_workspace::FileSystemProjectRegistry::new(&registry_root);
        registry
            .add(project_root.clone(), Some("erp".into()), None)
            .await
            .expect("add project");

        let mut job = job_for("t", "b", None);
        job.project = Some("erp".into());
        job.role = Some("luban".into());

        let cwd = resolve_project_sandbox_cwd(Some(&registry_root), &job)
            .await
            .expect("ok");
        let cwd = cwd.expect("L3 应返 cwd");
        assert!(cwd.ends_with("sandboxes/luban"), "got {}", cwd.display());
        assert!(cwd.exists(), "L3 sandbox 物理目录应已 create");
    }

    /// `job.project=Some` + ephemeral_task=Some → 走 L2：返 ephemeral worktree 路径。
    #[tokio::test]
    async fn resolve_project_sandbox_cwd_returns_l2_path_for_ephemeral() {
        let dir = tempfile::tempdir().expect("tmp");
        let registry_root = dir.path().join("registry");
        let project_root = dir.path().join("erp-src");
        std::fs::create_dir_all(&project_root).expect("mkdir project");
        init_repo(&project_root);

        let registry = fuxi_workspace::FileSystemProjectRegistry::new(&registry_root);
        registry
            .add(project_root.clone(), Some("erp".into()), None)
            .await
            .expect("add project");

        let task_id = fuxi_core::TaskId::new();
        let mut job = job_for("t", "b", None);
        job.project = Some("erp".into());
        job.ephemeral_task = Some(task_id.to_string());

        let cwd = resolve_project_sandbox_cwd(Some(&registry_root), &job)
            .await
            .expect("ok");
        let cwd = cwd.expect("L2 应返 cwd");
        assert!(
            cwd.to_string_lossy().contains("ephemeral"),
            "L2 路径应含 ephemeral：{}",
            cwd.display()
        );
        assert!(cwd.exists(), "L2 worktree 物理目录应已 create");

        // 同 task 重复调用应幂等复用（不 AlreadyExists 报错）
        let cwd2 = resolve_project_sandbox_cwd(Some(&registry_root), &job)
            .await
            .expect("re-call should reuse");
        assert_eq!(
            cwd2.expect("some"),
            cwd,
            "同 task 重复调用应复用同一 worktree"
        );
    }

    #[test]
    fn build_codex_prompt_from_job_prepends_system_prompt() {
        let job = job_for("标题", "描述", Some("你是鲁班"));
        let p = build_codex_prompt_from_job(&job);
        assert!(p.starts_with("你是鲁班\n\n---\n\n"), "got: {p}");
        assert!(p.contains("标题\n\n描述"), "got: {p}");
    }

    #[test]
    fn build_codex_prompt_from_job_without_system() {
        let job = job_for("标题", "描述", None);
        assert_eq!(build_codex_prompt_from_job(&job), "标题\n\n描述");
        let job = job_for("只有标题", "", None);
        assert_eq!(build_codex_prompt_from_job(&job), "只有标题");
    }

    /// 向后兼容：老版 controller 发来的 payload 不带 system_prompt，
    /// worker 端反序列化不应失败。
    #[test]
    fn dist_enqueue_req_deserializes_without_system_prompt() {
        // path 3 起 `token` 字段已移除——HMAC 签名取代之。serde 会忽略未知字段，
        // 老 controller / 老脚本仍带 `token` payload 不会反序列化失败。
        let raw = r#"{"token":"t","node_id":"n","title":"T","body":"B"}"#;
        let req: DistEnqueueReq = serde_json::from_str(raw).expect("decode");
        assert_eq!(req.node_id, "n");
        assert!(req.system_prompt.is_none());
    }

    /// 向前兼容：新版 worker 解码老版 /dist/pull 的 job 也不能炸。
    #[test]
    fn dist_job_deserializes_without_system_prompt() {
        let raw = r#"{"id":"j","node_id":"n","title":"T","body":"B","created_at":0}"#;
        let job: DistJob = serde_json::from_str(raw).expect("decode");
        assert_eq!(job.id, "j");
        assert!(job.system_prompt.is_none());
    }

    #[test]
    fn dist_enqueue_req_round_trips_system_prompt() {
        let req = DistEnqueueReq {
            node_id: "n".into(),
            title: "T".into(),
            body: "B".into(),
            system_prompt: Some("role preamble".into()),
            required_tags: vec![],
            pinned_node: None,
            cli: String::new(),
            allowed_tools: vec![],
            task_id: None,
            role: None,
            project: None,
            ephemeral_task: None,
            topic_id: None,
        };
        let encoded = serde_json::to_string(&req).unwrap();
        let decoded: DistEnqueueReq = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.system_prompt.as_deref(), Some("role preamble"));
    }

    // ── progress 子系统 ──

    async fn test_ctrl() -> Arc<DistController> {
        let bus = EventBus::with_memory_store().await.expect("bus");
        Arc::new(DistController::new("tok".into(), bus))
    }

    /// 排干 EventStream 一段时间（best-effort），把所有 worker_* 拓扑事件
    /// 收集到 Vec<EventKind>。其它无关事件丢弃；用于断言 publish 行为。
    async fn drain_worker_events(
        bus: &EventBus,
        budget: std::time::Duration,
    ) -> Vec<fuxi_core::EventKind> {
        use futures_util::StreamExt;
        let mut s = bus.subscribe();
        let deadline = tokio::time::Instant::now() + budget;
        let mut out = Vec::new();
        loop {
            let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remain.is_zero() {
                break;
            }
            match tokio::time::timeout(remain, s.next()).await {
                Ok(Some(Ok(ev))) => match &ev.kind {
                    fuxi_core::EventKind::WorkerRegistered { .. }
                    | fuxi_core::EventKind::WorkerHeartbeatStateChanged { .. }
                    | fuxi_core::EventKind::WorkerStaleSwept { .. } => out.push(ev.kind),
                    _ => {}
                },
                Ok(Some(Err(_))) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        out
    }

    /// 同上，但在 budget 内**等到**第一个目标事件就返回；用于不耐烦场景。
    async fn first_worker_event(
        bus: &EventBus,
        budget: std::time::Duration,
    ) -> Option<fuxi_core::EventKind> {
        use futures_util::StreamExt;
        let mut s = bus.subscribe();
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remain.is_zero() {
                return None;
            }
            match tokio::time::timeout(remain, s.next()).await {
                Ok(Some(Ok(ev))) => match ev.kind {
                    fuxi_core::EventKind::WorkerRegistered { .. }
                    | fuxi_core::EventKind::WorkerHeartbeatStateChanged { .. }
                    | fuxi_core::EventKind::WorkerStaleSwept { .. } => return Some(ev.kind),
                    _ => continue,
                },
                Ok(Some(Err(_))) => continue,
                Ok(None) | Err(_) => return None,
            }
        }
    }

    /// P6 [γ]: register 触发 WorkerRegistered（节点 / tags / cap 字段保真）。
    #[tokio::test]
    async fn worker_registered_event_published_on_register() {
        let ctrl = test_ctrl().await;
        let bus = ctrl.bus().clone();
        let probe = tokio::spawn(async move {
            first_worker_event(&bus, std::time::Duration::from_secs(2)).await
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        ctrl.register("nodeX".into(), vec!["cc".into(), "gpu".into()], 4)
            .await;
        let got = probe.await.expect("join").expect("event");
        match got {
            fuxi_core::EventKind::WorkerRegistered {
                node_id,
                tags,
                max_concurrency,
            } => {
                assert_eq!(node_id, "nodeX");
                assert_eq!(tags, vec!["cc".to_string(), "gpu".to_string()]);
                assert_eq!(max_concurrency, 4);
            }
            other => panic!("expect WorkerRegistered, got {other:?}"),
        }
    }

    /// P6 [γ]: 连续三次心跳同 inflight 列表，仅首次（status flip→alive）publish 一条。
    #[tokio::test]
    async fn worker_heartbeat_state_changed_only_on_diff() {
        let ctrl = test_ctrl().await;
        let bus = ctrl.bus().clone();
        ctrl.register("n".into(), vec![], 2).await;
        let _ = drain_worker_events(&bus, std::time::Duration::from_millis(50)).await;

        let probe = {
            let bus = bus.clone();
            tokio::spawn(async move {
                drain_worker_events(&bus, std::time::Duration::from_millis(300)).await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        ctrl.heartbeat("n", vec!["job-A".into()], None).await;
        ctrl.heartbeat("n", vec!["job-A".into()], None).await;
        ctrl.heartbeat("n", vec!["job-A".into()], None).await;
        let collected = probe.await.expect("join");
        let hb_events: Vec<_> = collected
            .iter()
            .filter(|k| matches!(k, fuxi_core::EventKind::WorkerHeartbeatStateChanged { .. }))
            .collect();
        assert_eq!(
            hb_events.len(),
            1,
            "三次同 inflight 心跳只应 publish 一次，实得 {}: {:?}",
            hb_events.len(),
            collected
        );
        match hb_events[0] {
            fuxi_core::EventKind::WorkerHeartbeatStateChanged {
                node_id,
                inflight_count,
                status,
            } => {
                assert_eq!(node_id, "n");
                assert_eq!(*inflight_count, 1);
                assert_eq!(*status, fuxi_core::WorkerStatus::Alive);
            }
            _ => unreachable!(),
        }
    }

    /// P6 [γ]: inflight 数量变化（1→2→2→1）应触发 3 次 publish（中间 2→2 被采样掉）。
    #[tokio::test]
    async fn worker_heartbeat_publishes_on_inflight_count_diff() {
        let ctrl = test_ctrl().await;
        let bus = ctrl.bus().clone();
        ctrl.register("n".into(), vec![], 5).await;
        let _ = drain_worker_events(&bus, std::time::Duration::from_millis(50)).await;

        let probe = {
            let bus = bus.clone();
            tokio::spawn(async move {
                drain_worker_events(&bus, std::time::Duration::from_millis(400)).await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        ctrl.heartbeat("n", vec!["A".into()], None).await;
        ctrl.heartbeat("n", vec!["A".into(), "B".into()], None)
            .await;
        ctrl.heartbeat("n", vec!["A".into(), "B".into()], None)
            .await;
        ctrl.heartbeat("n", vec!["A".into()], None).await;
        let collected = probe.await.expect("join");
        let hb_counts: Vec<u32> = collected
            .iter()
            .filter_map(|k| match k {
                fuxi_core::EventKind::WorkerHeartbeatStateChanged { inflight_count, .. } => {
                    Some(*inflight_count)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            hb_counts,
            vec![1, 2, 1],
            "应发三条（首发 + 1→2 + 2→1），中间 2→2 被采样掉；实得 {hb_counts:?}"
        );
    }

    /// P6 [γ]: sweep_stale 把超时 worker 的 inflight 回收时 publish WorkerStaleSwept。
    #[tokio::test]
    async fn worker_stale_swept_event_published_on_sweep() {
        let ctrl = test_ctrl().await;
        let bus = ctrl.bus().clone();
        ctrl.register("dead".into(), vec![], 1).await;
        ctrl.enqueue(
            "dead".into(),
            "t".into(),
            "b".into(),
            None,
            vec![],
            None,
            "codex".into(),
            vec![],
            None,
            None,
        )
        .await;
        let _job = ctrl.pull("dead").await.expect("pulled");
        let _ = drain_worker_events(&bus, std::time::Duration::from_millis(50)).await;

        let probe = {
            let bus = bus.clone();
            tokio::spawn(async move {
                drain_worker_events(&bus, std::time::Duration::from_millis(300)).await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let future = Instant::now() + Duration::from_secs(300);
        let recycled = ctrl.sweep_stale(future, Duration::from_secs(30)).await;
        assert_eq!(recycled.len(), 1);

        let collected = probe.await.expect("join");
        let swept: Vec<_> = collected
            .iter()
            .filter_map(|k| match k {
                fuxi_core::EventKind::WorkerStaleSwept {
                    node_id,
                    recycled_jobs,
                } => Some((node_id.clone(), recycled_jobs.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(swept.len(), 1, "expect 1 sweep event, got {swept:?}");
        assert_eq!(swept[0].0, "dead");
        assert_eq!(swept[0].1.len(), 1);
    }

    fn push(kind: ProgressKind, text: &str) -> ProgressPush {
        ProgressPush {
            kind,
            text: text.into(),
        }
    }

    #[tokio::test]
    async fn push_progress_assigns_monotonic_seq_per_job() {
        let ctrl = test_ctrl().await;
        let (acc1, last1, cancel1) = ctrl
            .push_progress(
                "nodeA",
                "job-1",
                vec![
                    push(ProgressKind::AssistantText, "hello"),
                    push(ProgressKind::AssistantText, "world"),
                ],
            )
            .await;
        assert_eq!(acc1, 2);
        assert_eq!(last1, 2);
        assert!(!cancel1);
        let (acc2, last2, cancel2) = ctrl
            .push_progress("nodeA", "job-1", vec![push(ProgressKind::Thinking, "嗯")])
            .await;
        assert_eq!(acc2, 1);
        assert_eq!(last2, 3);
        assert!(!cancel2);
    }

    /// cancel_job 标记后，下一次 push_progress 的 ack should_cancel=true。
    #[tokio::test]
    async fn push_progress_ack_reflects_cancellation() {
        let ctrl = test_ctrl().await;
        ctrl.cancel_job("job-x").await;
        let (_, _, should_cancel) = ctrl
            .push_progress(
                "nodeA",
                "job-x",
                vec![push(ProgressKind::AssistantText, "a")],
            )
            .await;
        assert!(
            should_cancel,
            "cancelled job 的 push ack 应返 should_cancel=true"
        );
    }

    /// 未 cancel 的 job ack 不该搞错成 true。
    #[tokio::test]
    async fn push_progress_ack_false_for_non_cancelled() {
        let ctrl = test_ctrl().await;
        let (_, _, should_cancel) = ctrl
            .push_progress(
                "nodeA",
                "job-y",
                vec![push(ProgressKind::AssistantText, "a")],
            )
            .await;
        assert!(!should_cancel);
    }

    #[tokio::test]
    async fn pull_progress_after_cursor_filters() {
        let ctrl = test_ctrl().await;
        ctrl.push_progress(
            "nodeA",
            "job-1",
            vec![
                push(ProgressKind::AssistantText, "a"),
                push(ProgressKind::AssistantText, "b"),
                push(ProgressKind::Thinking, "c"),
            ],
        )
        .await;
        let resp = ctrl.pull_progress_after("job-1", 0).await;
        assert_eq!(resp.chunks.len(), 3);
        assert!(!resp.done);
        let resp2 = ctrl.pull_progress_after("job-1", 2).await;
        assert_eq!(resp2.chunks.len(), 1);
        assert_eq!(resp2.chunks[0].seq, 3);
        assert_eq!(resp2.chunks[0].kind, ProgressKind::Thinking);
    }

    #[tokio::test]
    async fn pull_progress_reports_done_after_report() {
        let ctrl = test_ctrl().await;
        ctrl.push_progress(
            "nodeA",
            "job-1",
            vec![push(ProgressKind::AssistantText, "partial")],
        )
        .await;
        // 在收到 final report 前 done=false
        assert!(!ctrl.pull_progress_after("job-1", 0).await.done);
        ctrl.report(DistReportReq {
            node_id: "nodeA".into(),
            job_id: "job-1".into(),
            ok: true,
            output: "final".into(),
            duration_ms: 42,
        })
        .await;
        let resp = ctrl.pull_progress_after("job-1", 0).await;
        assert!(resp.done);
        assert_eq!(resp.final_ok, Some(true));
        assert_eq!(resp.final_output.as_deref(), Some("final"));
    }

    #[tokio::test]
    async fn progress_seq_spaces_are_independent_per_job() {
        let ctrl = test_ctrl().await;
        ctrl.push_progress(
            "nodeA",
            "job-A",
            vec![push(ProgressKind::AssistantText, "a")],
        )
        .await;
        let (_, last_b, _) = ctrl
            .push_progress(
                "nodeA",
                "job-B",
                vec![push(ProgressKind::AssistantText, "b")],
            )
            .await;
        assert_eq!(last_b, 1, "job-B 的 seq 从 1 起，不受 job-A 影响");
    }

    #[test]
    fn codex_event_agent_message_maps_to_assistant_text() {
        let ev = CodexEvent::AgentMessage {
            item_id: "i".into(),
            text: "hello".into(),
        };
        let p = codex_event_to_push(&ev).unwrap();
        assert_eq!(p.kind, ProgressKind::AssistantText);
        assert_eq!(p.text, "hello");
    }

    #[test]
    fn codex_event_command_events_map_to_tool_call() {
        let started = CodexEvent::CommandStarted {
            item_id: "i".into(),
            command: "ls".into(),
            raw_item: serde_json::Value::Null,
        };
        assert_eq!(
            codex_event_to_push(&started).unwrap().kind,
            ProgressKind::ToolCall
        );

        let completed = CodexEvent::CommandCompleted {
            item_id: "i".into(),
            command: "ls".into(),
            exit_code: Some(1),
            status: "failed".into(),
            output_preview: "oops".into(),
        };
        let p = codex_event_to_push(&completed).unwrap();
        assert_eq!(p.kind, ProgressKind::ToolCall);
        assert!(p.text.contains("exit=1"), "got: {}", p.text);
        assert!(p.text.contains("oops"), "got: {}", p.text);
    }

    #[test]
    fn codex_event_errors_map_to_error_kind() {
        let p = codex_event_to_push(&CodexEvent::Error {
            message: "boom".into(),
        })
        .unwrap();
        assert_eq!(p.kind, ProgressKind::Error);
        let p = codex_event_to_push(&CodexEvent::TurnFailed {
            reason: "rate".into(),
        })
        .unwrap();
        assert_eq!(p.kind, ProgressKind::Error);
    }

    #[test]
    fn codex_event_protocol_meta_events_are_silent() {
        assert!(
            codex_event_to_push(&CodexEvent::TurnStarted).is_none(),
            "TurnStarted 不该上报"
        );
        assert!(
            codex_event_to_push(&CodexEvent::ThreadStarted {
                thread_id: "t".into()
            })
            .is_none(),
            "ThreadStarted 不该上报"
        );
        assert!(
            codex_event_to_push(&CodexEvent::TurnCompleted {
                usage: serde_json::Value::Null
            })
            .is_none(),
            "TurnCompleted 不该上报"
        );
    }

    #[tokio::test]
    async fn progress_req_round_trips_via_serde() {
        let req = DistProgressReq {
            node_id: "n".into(),
            job_id: "j".into(),
            chunks: vec![push(ProgressKind::ToolCall, "ls -la")],
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: DistProgressReq = serde_json::from_str(&s).unwrap();
        assert_eq!(back.chunks.len(), 1);
        assert_eq!(back.chunks[0].kind, ProgressKind::ToolCall);
        assert_eq!(back.chunks[0].text, "ls -la");
    }

    // ── Phase 3a · worker capability 上报 ──

    #[tokio::test]
    async fn register_stores_tags_and_capacity() {
        let ctrl = test_ctrl().await;
        ctrl.register("nodeA".into(), vec!["home".into(), "codex".into()], 2)
            .await;
        let info = ctrl.node_info("nodeA").await.expect("node registered");
        assert_eq!(info.tags, vec!["home", "codex"]);
        assert_eq!(info.max_concurrency, 2);
        assert!(info.last_seen.is_some());
        assert!(info.inflight.is_empty());
    }

    #[tokio::test]
    async fn register_reconnect_updates_tags_but_preserves_inflight() {
        let ctrl = test_ctrl().await;
        ctrl.register("n".into(), vec!["a".into()], 1).await;
        // 模拟 inflight：Phase 3b 会通过 pull 自动填，这里手注入一条验证边界。
        {
            let mut g = ctrl.inner.lock().await;
            g.nodes.get_mut("n").unwrap().inflight.push("job-1".into());
        }
        // worker 重连（同 node_id 再次 register）
        ctrl.register("n".into(), vec!["b".into()], 3).await;
        let info = ctrl.node_info("n").await.unwrap();
        assert_eq!(info.tags, vec!["b"], "tags 应被覆盖");
        assert_eq!(info.max_concurrency, 3);
        assert_eq!(
            info.inflight,
            vec!["job-1"],
            "inflight 是 runtime state，不该被 register 清空"
        );
    }

    #[tokio::test]
    async fn register_zero_capacity_clamps_to_one() {
        let ctrl = test_ctrl().await;
        ctrl.register("n".into(), vec![], 0).await;
        assert_eq!(
            ctrl.node_info("n").await.unwrap().max_concurrency,
            1,
            "0 会让 worker 永远不接任务——归一到 1"
        );
    }

    #[tokio::test]
    async fn node_info_returns_none_for_unknown_node() {
        let ctrl = test_ctrl().await;
        assert!(ctrl.node_info("ghost").await.is_none());
    }

    #[tokio::test]
    async fn nodes_snapshot_returns_empty_when_no_workers() {
        let ctrl = test_ctrl().await;
        let snap = ctrl.nodes_snapshot().await;
        assert!(snap.is_empty(), "无 worker 应返回空 vec, got {snap:?}");
    }

    /// 三个 worker 按 node_id 字典序输出——TUI 渲染稳定靠这个。
    /// 同时验证 ms_ago 字段非 None（刚 register 完）+ status="alive"。
    #[tokio::test]
    async fn nodes_snapshot_returns_all_registered_workers_sorted_by_id() {
        let ctrl = test_ctrl().await;
        ctrl.register("z-node".into(), vec!["cc".into()], 4).await;
        ctrl.register("a-node".into(), vec!["codex".into(), "gpu".into()], 2)
            .await;
        ctrl.register("m-node".into(), vec![], 1).await;

        let snap = ctrl.nodes_snapshot().await;
        assert_eq!(snap.len(), 3);
        let ids: Vec<&str> = snap.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(ids, vec!["a-node", "m-node", "z-node"], "应字典序");

        let a = &snap[0];
        assert_eq!(a.tags, vec!["codex", "gpu"]);
        assert_eq!(a.max_concurrency, 2);
        assert_eq!(a.inflight_count, 0);
        assert!(a.last_seen_ms_ago.is_some(), "刚 register，应有 last_seen");
        assert!(a.registered_at_ms_ago.is_some());
        assert_eq!(a.status, "alive");
    }

    /// 重连场景：第二次 register `registered_at` 不被覆盖。
    /// 这是 wire 字段 `registered_at_ms_ago` 表达"在线时长"的核心保证——
    /// 否则重连一次就清零，TUI 上看起来 worker 永远是新生的。
    #[tokio::test]
    async fn nodes_snapshot_registered_at_preserved_across_reconnect() {
        let ctrl = test_ctrl().await;
        ctrl.register("n".into(), vec!["a".into()], 1).await;
        // 让时间过去一点
        tokio::time::sleep(Duration::from_millis(20)).await;
        let first_at = {
            let g = ctrl.inner.lock().await;
            g.nodes.get("n").unwrap().registered_at
        };
        // 重连
        ctrl.register("n".into(), vec!["b".into()], 2).await;
        let second_at = {
            let g = ctrl.inner.lock().await;
            g.nodes.get("n").unwrap().registered_at
        };
        assert_eq!(first_at, second_at, "重连不该刷新 registered_at");
    }

    /// `pull` 把 job 写进 worker.inflight，snapshot 应反映 inflight_count。
    #[tokio::test]
    async fn nodes_snapshot_reflects_inflight_after_pull() {
        let ctrl = test_ctrl().await;
        ctrl.register("nodeA".into(), vec![], 2).await;
        let _jid1 = ctrl
            .enqueue(
                "h".into(),
                "t1".into(),
                String::new(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        let _jid2 = ctrl
            .enqueue(
                "h".into(),
                "t2".into(),
                String::new(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        ctrl.pull("nodeA").await.expect("pull1");
        ctrl.pull("nodeA").await.expect("pull2");

        let snap = ctrl.nodes_snapshot().await;
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].inflight_count, 2);
        assert_eq!(snap[0].inflight.len(), 2);
    }

    /// 向后兼容：老版 worker 不带 tags/max_concurrency，serde 要给默认值。
    #[test]
    fn dist_register_req_deserializes_without_new_fields() {
        let raw = r#"{"token":"t","node_id":"n"}"#;
        let req: DistRegisterReq = serde_json::from_str(raw).expect("decode");
        assert!(req.tags.is_empty());
        assert_eq!(req.max_concurrency, 1, "默认 1 并发");
    }

    // ── Phase 3b · 全局 queue + tag matcher ──

    async fn enq_simple(ctrl: &DistController, title: &str) -> String {
        ctrl.enqueue(
            "hint".into(),
            title.into(),
            String::new(),
            None,
            vec![],
            None,
            String::new(),
            vec![],
            None,
            None,
        )
        .await
    }

    /// 无 tag 要求的 job 可被任一 worker pull（全局 queue）。
    #[tokio::test]
    async fn pull_anyone_gets_untagged_job() {
        let ctrl = test_ctrl().await;
        ctrl.register("nodeA".into(), vec!["codex".into()], 1).await;
        ctrl.register("nodeB".into(), vec![], 1).await;
        let jid = enq_simple(&ctrl, "任意").await;
        let got = ctrl.pull("nodeB").await.expect("nodeB should match");
        assert_eq!(got.id, jid);
        assert!(
            ctrl.pull("nodeA").await.is_none(),
            "job 已被 B 取走，A 应为空"
        );
    }

    /// required_tags 必须是 worker.tags 的子集；不匹配的 worker 拿不到。
    #[tokio::test]
    async fn pull_filters_by_required_tags() {
        let ctrl = test_ctrl().await;
        ctrl.register("nodeA".into(), vec!["codex".into(), "gpu".into()], 1)
            .await;
        ctrl.register("nodeB".into(), vec!["codex".into()], 1).await;
        let jid = ctrl
            .enqueue(
                "hint".into(),
                "需要 gpu".into(),
                String::new(),
                None,
                vec!["gpu".into()],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        assert!(
            ctrl.pull("nodeB").await.is_none(),
            "nodeB 无 gpu tag 不应匹配"
        );
        let got = ctrl.pull("nodeA").await.expect("nodeA 应匹配");
        assert_eq!(got.id, jid);
    }

    /// pinned_node 只允许指定 node 取；别的 worker 跳过。
    #[tokio::test]
    async fn pull_honors_pinned_node() {
        let ctrl = test_ctrl().await;
        ctrl.register("nodeA".into(), vec![], 1).await;
        ctrl.register("nodeB".into(), vec![], 1).await;
        let jid = ctrl
            .enqueue(
                "hint".into(),
                "pin to B".into(),
                String::new(),
                None,
                vec![],
                Some("nodeB".into()),
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        assert!(ctrl.pull("nodeA").await.is_none(), "pin 到 B，A 不该取到");
        let got = ctrl.pull("nodeB").await.expect("B 能取到");
        assert_eq!(got.id, jid);
    }

    /// pinned 的 job 跳过后，后面无 pin 的 job 能被跳过者取走——matcher 不是贪婪的。
    #[tokio::test]
    async fn pull_skips_pinned_to_find_later_match() {
        let ctrl = test_ctrl().await;
        ctrl.register("nodeA".into(), vec![], 1).await;
        ctrl.register("nodeB".into(), vec![], 1).await;
        let _pinned_to_b = ctrl
            .enqueue(
                "hint".into(),
                "pin B".into(),
                String::new(),
                None,
                vec![],
                Some("nodeB".into()),
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        let free = enq_simple(&ctrl, "anyone").await;
        let got = ctrl
            .pull("nodeA")
            .await
            .expect("nodeA 应跳过 pinned-to-B，取第二条");
        assert_eq!(got.id, free);
    }

    /// capacity 满时 pull 返 None——即使 queue 非空。
    #[tokio::test]
    async fn pull_blocks_when_worker_at_capacity() {
        let ctrl = test_ctrl().await;
        ctrl.register("nodeA".into(), vec![], 1).await;
        let _j1 = enq_simple(&ctrl, "j1").await;
        let _j2 = enq_simple(&ctrl, "j2").await;
        assert!(ctrl.pull("nodeA").await.is_some(), "第一条取走");
        assert!(ctrl.pull("nodeA").await.is_none(), "容量 1 已满，不应再派");
    }

    /// report 释放 capacity——之后 pull 能再取。
    #[tokio::test]
    async fn report_releases_capacity_for_next_pull() {
        let ctrl = test_ctrl().await;
        ctrl.register("nodeA".into(), vec![], 1).await;
        let j1 = enq_simple(&ctrl, "j1").await;
        let _j2 = enq_simple(&ctrl, "j2").await;
        let _ = ctrl.pull("nodeA").await.expect("take j1");
        ctrl.report(DistReportReq {
            node_id: "nodeA".into(),
            job_id: j1,
            ok: true,
            output: "done".into(),
            duration_ms: 1,
        })
        .await;
        assert!(ctrl.pull("nodeA").await.is_some(), "report 释放后应能再取");
    }

    /// max_concurrency=2 能同时 hold 两条 job。
    #[tokio::test]
    async fn pull_respects_max_concurrency_greater_than_one() {
        let ctrl = test_ctrl().await;
        ctrl.register("nodeA".into(), vec![], 2).await;
        let _ = enq_simple(&ctrl, "a").await;
        let _ = enq_simple(&ctrl, "b").await;
        let _ = enq_simple(&ctrl, "c").await;
        assert!(ctrl.pull("nodeA").await.is_some());
        assert!(ctrl.pull("nodeA").await.is_some(), "第二条仍 OK");
        assert!(ctrl.pull("nodeA").await.is_none(), "第三条被 capacity 拦");
    }

    /// 未 register 的 worker 直接 pull 也不炸——默认 1 并发 + 空 tags。
    #[tokio::test]
    async fn pull_works_for_unregistered_worker_with_defaults() {
        let ctrl = test_ctrl().await;
        let jid = enq_simple(&ctrl, "anyone").await;
        let got = ctrl.pull("ghost").await.expect("默认 1 容量应能取");
        assert_eq!(got.id, jid);
    }

    #[test]
    fn dist_enqueue_req_round_trips_tag_and_pin_fields() {
        let req = DistEnqueueReq {
            node_id: "hint".into(),
            title: "T".into(),
            body: "B".into(),
            system_prompt: None,
            required_tags: vec!["codex".into(), "gpu".into()],
            pinned_node: Some("home".into()),
            cli: String::new(),
            allowed_tools: vec![],
            task_id: None,
            role: None,
            project: None,
            ephemeral_task: None,
            topic_id: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: DistEnqueueReq = serde_json::from_str(&s).unwrap();
        assert_eq!(back.required_tags, vec!["codex", "gpu"]);
        assert_eq!(back.pinned_node.as_deref(), Some("home"));
    }

    // ── Phase 4a · CliAdapter trait ──

    fn worker_args_stub() -> DistWorkerArgs {
        DistWorkerArgs {
            controller: "http://x".into(),
            node: "n".into(),
            token: None,
            codex_bin: "codex".into(),
            cc_bin: "claude".into(),
            poll_ms: 1000,
            tags: vec![],
            max_concurrency: 1,
            projects_root: None,
        }
    }

    #[test]
    fn select_adapter_accepts_codex() {
        let a = match super::select_adapter("codex", &worker_args_stub()) {
            Ok(a) => a,
            Err(e) => panic!("codex 应通过: {e}"),
        };
        assert_eq!(a.name(), "codex");
    }

    #[test]
    fn select_adapter_treats_empty_string_as_codex_default() {
        // 老版 gateway 不填 cli 字段 → serde default 到空串 → 走 codex 兼容。
        let a = match super::select_adapter("", &worker_args_stub()) {
            Ok(a) => a,
            Err(e) => panic!("空串应落回 codex: {e}"),
        };
        assert_eq!(a.name(), "codex");
    }

    #[test]
    fn select_adapter_rejects_unknown_cli() {
        match super::select_adapter("vim", &worker_args_stub()) {
            Ok(a) => panic!("vim 不该被接受，却返回了 {}", a.name()),
            Err(e) => {
                let msg = format!("{e}");
                assert!(msg.contains("vim"), "error 里应带 unknown cli 名: {msg}");
            }
        }
    }

    // ── Phase 4c · CcAdapter MVP ──

    #[test]
    fn select_adapter_accepts_claude_code() {
        let a = match super::select_adapter("claude-code", &worker_args_stub()) {
            Ok(a) => a,
            Err(e) => panic!("claude-code 应通过: {e}"),
        };
        assert_eq!(a.name(), "claude-code");
    }

    #[test]
    fn cc_event_assistant_text_maps_to_assistant_kind() {
        let ev = fuxi_agent_cc::CcEvent::AssistantText {
            text: "hello world".into(),
        };
        let p = super::cc_event_to_push(&ev).unwrap();
        assert_eq!(p.kind, ProgressKind::AssistantText);
        assert_eq!(p.text, "hello world");
    }

    #[test]
    fn cc_event_thinking_maps_to_thinking_kind() {
        let ev = fuxi_agent_cc::CcEvent::AssistantThinking {
            text: "Hmm let me think".into(),
        };
        let p = super::cc_event_to_push(&ev).unwrap();
        assert_eq!(p.kind, ProgressKind::Thinking);
    }

    #[test]
    fn cc_event_tool_use_maps_to_tool_call() {
        let ev = fuxi_agent_cc::CcEvent::AssistantToolUse {
            tool_id: "tooluse_1".into(),
            tool_name: "Read".into(),
            input: serde_json::json!({"path": "/tmp/x"}),
        };
        let p = super::cc_event_to_push(&ev).unwrap();
        assert_eq!(p.kind, ProgressKind::ToolCall);
        assert!(p.text.starts_with("Read"), "tool_name 应在前: {}", p.text);
        assert!(p.text.contains("/tmp/x"), "input brief 应保留: {}", p.text);
    }

    #[test]
    fn cc_event_tool_result_error_maps_to_error_kind() {
        let ev = fuxi_agent_cc::CcEvent::UserToolResult {
            tool_use_id: "t1".into(),
            is_error: true,
            content_preview: "permission denied".into(),
        };
        let p = super::cc_event_to_push(&ev).unwrap();
        assert_eq!(p.kind, ProgressKind::Error);
    }

    #[test]
    fn cc_event_result_success_is_silent() {
        // ResultSuccess.text 和最后 AssistantText 往往重复——worker 已累积过，
        // 不能重复 push 否则 TUI 看到两份相同文本。
        let ev = fuxi_agent_cc::CcEvent::ResultSuccess {
            text: "hello".into(),
            usage: None,
        };
        assert!(super::cc_event_to_push(&ev).is_none());
    }

    #[test]
    fn cc_event_result_error_maps_to_error_kind() {
        let ev = fuxi_agent_cc::CcEvent::ResultError {
            reason: "rate limited".into(),
            usage: None,
        };
        let p = super::cc_event_to_push(&ev).unwrap();
        assert_eq!(p.kind, ProgressKind::Error);
        assert!(p.text.contains("rate limited"));
    }

    #[test]
    fn cc_event_system_events_are_silent() {
        let ev = fuxi_agent_cc::CcEvent::SystemInit {
            session_id: "s".into(),
            model: None,
            cwd: None,
        };
        assert!(super::cc_event_to_push(&ev).is_none());
        let ev = fuxi_agent_cc::CcEvent::RateLimit {
            info: serde_json::Value::Null,
        };
        assert!(super::cc_event_to_push(&ev).is_none());
    }

    // ── Phase 3c · 心跳 + sweep ──

    #[tokio::test]
    async fn heartbeat_refreshes_last_seen_and_inflight() {
        let ctrl = test_ctrl().await;
        ctrl.register("n".into(), vec![], 1).await;
        let pending = ctrl
            .heartbeat("n", vec!["job-x".into(), "job-y".into()], None)
            .await;
        assert!(pending.is_empty());
        let info = ctrl.node_info("n").await.unwrap();
        assert_eq!(info.inflight, vec!["job-x", "job-y"]);
        assert!(info.last_seen.is_some());
    }

    /// worker 视角权威：心跳里的 inflight 覆盖 controller 的记录。模拟"worker
    /// 重启丢了某 job 的 tracking" → 心跳后 controller 跟上。
    #[tokio::test]
    async fn heartbeat_lets_worker_be_authoritative_on_its_inflight() {
        let ctrl = test_ctrl().await;
        ctrl.register("n".into(), vec![], 2).await;
        // 先模拟 pull 让 controller 记 inflight=[A,B]
        {
            let mut g = ctrl.inner.lock().await;
            g.nodes
                .get_mut("n")
                .unwrap()
                .inflight
                .extend(["A".into(), "B".into()]);
        }
        // worker 心跳只声明 B——表示 A 已经没在跑（可能它已经 report 完、
        // 或者进程重启丢 state）
        ctrl.heartbeat("n", vec!["B".into()], None).await;
        let info = ctrl.node_info("n").await.unwrap();
        assert_eq!(info.inflight, vec!["B"]);
    }

    /// PR-B：controller 重启丢内存 nodes 表后，worker 下次心跳带 metadata
    /// 自愈 entry——tags / max_concurrency / registered_at 立即恢复，不再需要
    /// worker 端独立 re-register RPC。
    ///
    /// 反演 home 部署的实测 bug：mac worker register 5 天前跑过一次成功；home
    /// fuxi-im 重启清 nodes 表；之后 mac 心跳走 `or_default()` 重建 entry
    /// 但 tags=[] / max_concurrency=1 / registered_at=None。修后心跳就能填回。
    #[tokio::test]
    async fn heartbeat_with_metadata_restores_entry_after_controller_restart() {
        let ctrl = test_ctrl().await;
        // 反演重启：node 从未在本 controller 上 register（旧 controller 死掉了）。
        assert!(ctrl.node_info("mac").await.is_none());

        let meta = NodeHeartbeatMetadata {
            tags: vec!["mac".into(), "local".into()],
            max_concurrency: 2,
        };
        ctrl.heartbeat("mac", vec![], Some(meta)).await;

        let info = ctrl.node_info("mac").await.expect("心跳建 entry");
        assert_eq!(info.tags, vec!["mac".to_string(), "local".to_string()]);
        assert_eq!(info.max_concurrency, 2);
        assert!(
            info.registered_at.is_some(),
            "registered_at 由 metadata 心跳填充（不再是 None）"
        );
    }

    /// PR-B：metadata 反向兼容——老 worker 不带 metadata（None）时，
    /// `or_default()` 新 entry 仍是默认值（tags=[]、max_concurrency=1）；
    /// 已注册的 entry 不被覆盖。
    #[tokio::test]
    async fn heartbeat_without_metadata_keeps_existing_register_values() {
        let ctrl = test_ctrl().await;
        ctrl.register("n".into(), vec!["a".into()], 3).await;
        ctrl.heartbeat("n", vec![], None).await;
        let info = ctrl.node_info("n").await.unwrap();
        assert_eq!(info.tags, vec!["a".to_string()]);
        assert_eq!(info.max_concurrency, 3);
    }

    /// PR-B：心跳是 worker 真当前认知——metadata 来时刷新 entry。worker 改 tag
    /// 后无需重启 controller，下个心跳就生效。
    #[tokio::test]
    async fn heartbeat_metadata_refreshes_existing_entry() {
        let ctrl = test_ctrl().await;
        ctrl.register("n".into(), vec!["old-tag".into()], 1).await;
        let meta = NodeHeartbeatMetadata {
            tags: vec!["new-tag".into()],
            max_concurrency: 4,
        };
        ctrl.heartbeat("n", vec![], Some(meta)).await;
        let info = ctrl.node_info("n").await.unwrap();
        assert_eq!(info.tags, vec!["new-tag".to_string()]);
        assert_eq!(info.max_concurrency, 4);
    }

    /// PR-B：metadata 路径也跑 `max_concurrency.max(1)` 归一——worker 误传 0
    /// 不能让自己锁死。
    #[tokio::test]
    async fn heartbeat_metadata_zero_capacity_clamps_to_one() {
        let ctrl = test_ctrl().await;
        let meta = NodeHeartbeatMetadata {
            tags: vec![],
            max_concurrency: 0,
        };
        ctrl.heartbeat("n", vec![], Some(meta)).await;
        assert_eq!(ctrl.node_info("n").await.unwrap().max_concurrency, 1);
    }

    /// cancel 请求的 job 被 worker 心跳感知——返回给 worker 去杀 child。
    #[tokio::test]
    async fn heartbeat_reports_cancel_pending_in_intersection() {
        let ctrl = test_ctrl().await;
        ctrl.register("n".into(), vec![], 2).await;
        ctrl.cancel_job("A").await;
        ctrl.cancel_job("Z").await; // Z 不在 worker 的 inflight 里
        let pending = ctrl
            .heartbeat("n", vec!["A".into(), "B".into()], None)
            .await;
        assert_eq!(
            pending,
            vec!["A".to_string()],
            "只返回 inflight ∩ cancelled"
        );
    }

    #[tokio::test]
    async fn sweep_reclaims_stale_worker_inflight_to_queue_front() {
        let ctrl = test_ctrl().await;
        ctrl.register("dead".into(), vec![], 1).await;
        // 给 dead worker 手动塞 inflight + 往 controller.inflight 注入 job
        let (job_a, job_b) = {
            let mut g = ctrl.inner.lock().await;
            let job_a = DistJob {
                id: "A".into(),
                node_id: "dead".into(),
                title: "ta".into(),
                body: String::new(),
                created_at: 0,
                system_prompt: None,
                required_tags: vec![],
                pinned_node: None,
                cli: String::new(),
                allowed_tools: vec![],
                task_id: None,
                role: None,
                project: None,
                ephemeral_task: None,
                topic_id: None,
            };
            let job_b = DistJob {
                id: "B".into(),
                node_id: "dead".into(),
                title: "tb".into(),
                body: String::new(),
                created_at: 0,
                system_prompt: None,
                required_tags: vec![],
                pinned_node: None,
                cli: String::new(),
                allowed_tools: vec![],
                task_id: None,
                role: None,
                project: None,
                ephemeral_task: None,
                topic_id: None,
            };
            g.inflight.insert("A".into(), job_a.clone());
            g.inflight.insert("B".into(), job_b.clone());
            let node = g.nodes.get_mut("dead").unwrap();
            node.inflight = vec!["A".into(), "B".into()];
            // 模拟 last_seen 在很久以前
            node.last_seen = Some(Instant::now() - Duration::from_secs(120));
            (job_a, job_b)
        };
        let recycled = ctrl
            .sweep_stale(Instant::now(), Duration::from_secs(30))
            .await;
        assert_eq!(recycled.len(), 1);
        assert_eq!(recycled[0].0, "dead");
        assert_eq!(recycled[0].1, vec!["A", "B"]);
        // 验证 job 回到 queue 前端
        let g = ctrl.inner.lock().await;
        let front_ids: Vec<&str> = g.global_queue.iter().map(|j| j.id.as_str()).collect();
        assert_eq!(front_ids, vec!["A", "B"]);
        assert!(!g.inflight.contains_key("A"));
        assert!(!g.inflight.contains_key("B"));
        assert!(g.nodes.get("dead").unwrap().inflight.is_empty());
        // 使用 jobs 避免 clippy dead_let
        let _ = (job_a.id, job_b.id);
    }

    #[tokio::test]
    async fn sweep_leaves_live_workers_alone() {
        let ctrl = test_ctrl().await;
        ctrl.register("alive".into(), vec![], 1).await;
        {
            let mut g = ctrl.inner.lock().await;
            let node = g.nodes.get_mut("alive").unwrap();
            node.inflight = vec!["A".into()];
            // 心跳刚刚
            node.last_seen = Some(Instant::now());
        }
        let recycled = ctrl
            .sweep_stale(Instant::now(), Duration::from_secs(30))
            .await;
        assert!(recycled.is_empty());
        let info = ctrl.node_info("alive").await.unwrap();
        assert_eq!(info.inflight, vec!["A"]);
    }

    #[test]
    fn dist_heartbeat_req_serde_round_trip() {
        let req = DistHeartbeatReq {
            node_id: "n".into(),
            inflight: vec!["job-1".into(), "job-2".into()],
            tags: Some(vec!["mac".into()]),
            max_concurrency: Some(2),
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: DistHeartbeatReq = serde_json::from_str(&s).unwrap();
        assert_eq!(back.inflight.len(), 2);
        assert_eq!(back.tags.as_deref(), Some(&["mac".to_string()][..]));
        assert_eq!(back.max_concurrency, Some(2));
    }

    /// 老版 worker 没有 inflight 字段——要能兜底空。
    #[test]
    fn dist_heartbeat_req_deserializes_without_inflight() {
        let raw = r#"{"token":"t","node_id":"n"}"#;
        let req: DistHeartbeatReq = serde_json::from_str(raw).unwrap();
        assert!(req.inflight.is_empty());
        assert!(req.tags.is_none());
        assert!(req.max_concurrency.is_none());
    }

    /// PR-B：心跳 metadata 字段对老版 worker 必须 `#[serde(default)]`——
    /// 缺字段不能 panic，反向兼容性是 wire 协议的硬契约。
    #[test]
    fn dist_heartbeat_req_omits_metadata_when_none() {
        let req = DistHeartbeatReq {
            node_id: "n".into(),
            inflight: vec![],
            tags: None,
            max_concurrency: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        // `skip_serializing_if = Option::is_none` 让 None 字段不出现在 wire——
        // 老 controller 解新 worker 的 JSON 看不到这俩 unknown 字段，兼容性 OK。
        assert!(!s.contains("\"tags\""));
        assert!(!s.contains("\"max_concurrency\""));
    }

    /// 老版 gateway 不带 required_tags/pinned_node，serde 要兜默认。
    #[test]
    fn dist_enqueue_req_deserializes_without_tag_fields() {
        let raw = r#"{"token":"t","node_id":"n","title":"T","body":"B"}"#;
        let req: DistEnqueueReq = serde_json::from_str(raw).unwrap();
        assert!(req.required_tags.is_empty());
        assert!(req.pinned_node.is_none());
    }

    #[test]
    fn dist_register_req_round_trips_new_fields() {
        let req = DistRegisterReq {
            node_id: "n".into(),
            tags: vec!["home".into(), "gpu".into()],
            max_concurrency: 4,
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: DistRegisterReq = serde_json::from_str(&s).unwrap();
        assert_eq!(back.tags, vec!["home", "gpu"]);
        assert_eq!(back.max_concurrency, 4);
    }

    // ── Decision 12 · worker 真并发 + 静默期 cancel 经心跳 ack ──
    //
    // 这两个测试起一个真 axum controller + 真 worker loop（spawn 在 task 里），
    // 不 mock controller 协议；只在 adapter 这一层用 stub 替换 codex/cc 子进程。
    // 这样能验证 worker 主循环 + 心跳 ack cancel 真的端到端走通，而不是只
    // 验单元逻辑。

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 测试用的 fake adapter：按构造时给的"行为"决定 run() 怎么返回。
    /// 用 Arc 共享计数 + Notify 让测试侧可以观察"几个 job 同时在跑"。
    struct StubAdapter {
        behavior: StubBehavior,
        active: Arc<AtomicUsize>,
    }

    #[derive(Clone)]
    enum StubBehavior {
        /// 立即 ok 返回——验证 worker pickup/report 用。
        Immediate,
        /// sleep 后 ok 返回——验证并发用。
        Sleep(Duration),
        /// 长 sleep 但响应外部 cancel（adapter.run 自身**不**消费 token——
        /// 让 worker 外层 select 在 token cancelled 时退出 adapter.run future）。
        SleepResponsiveToOuterCancel(Duration),
    }

    #[async_trait::async_trait]
    impl CliAdapter for StubAdapter {
        fn name(&self) -> &'static str {
            "stub"
        }
        async fn run(&self, _ctx: &WorkerCtx<'_>, _job: &DistJob) -> Result<(bool, String)> {
            self.active.fetch_add(1, Ordering::SeqCst);
            // RAII guard 保证无论 path（正常 return / future drop by select cancel）
            // active 都正确减回——避免测试假 fail。
            struct Guard<'a>(&'a AtomicUsize);
            impl Drop for Guard<'_> {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::SeqCst);
                }
            }
            let _g = Guard(&self.active);
            match self.behavior {
                StubBehavior::Immediate => Ok((true, "stub done".into())),
                StubBehavior::Sleep(d) => {
                    tokio::time::sleep(d).await;
                    Ok((true, "stub done".into()))
                }
                StubBehavior::SleepResponsiveToOuterCancel(d) => {
                    tokio::time::sleep(d).await;
                    Ok((true, "should have been cancelled".into()))
                }
            }
        }
    }

    /// 起一个 axum dist controller 在随机端口；返回 (controller arc, base url, server task)。
    /// 用 task spawn 而不是 block——让测试主体跑 worker loop / 客户端调用。
    async fn spawn_controller() -> (Arc<DistController>, String, tokio::task::JoinHandle<()>) {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let ctrl = Arc::new(DistController::new("tok".into(), bus));
        let app = router(ctrl.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        // 给 axum 一个微小窗口完成 listener accept-loop 的 ready，避免首次 POST race。
        tokio::time::sleep(Duration::from_millis(20)).await;
        (ctrl, format!("http://{addr}"), handle)
    }

    /// β path 3: worker 测试用的 HMAC secret——controller 端 router 不挂
    /// hmac_layer（测试 spawn_controller 用 `dist::router`，无 layer），所以
    /// secret 内容在测试中不验签，仅满足 `run_worker_with` 的入参约束。
    fn test_secret() -> Arc<crate::dist_auth::HmacSecret> {
        Arc::new(crate::dist_auth::HmacSecret::new("test".into()))
    }

    fn make_factory(adapter: Arc<StubAdapter>) -> super::AdapterFactory {
        let inner = adapter;
        Arc::new(move |_cli, _args| {
            // 这里 Box 一个新 trait object 包装同一份共享 active 计数。
            // 不能直接 `Box::new(*inner.clone())` 因为 StubAdapter 不 Clone；
            // 借 Arc deref 重新组装即可。
            let cloned = StubAdapter {
                behavior: inner.behavior.clone(),
                active: inner.active.clone(),
            };
            Ok(Box::new(cloned) as Box<dyn CliAdapter>)
        })
    }

    /// P1 回归：home 节点被 controller 自注册后，必须有同进程 worker
    /// 真正消费 pinned 到 home 的队列项。否则 auto-pin 选 home 时 job 会永远
    /// 停在 queued/assignee=""。
    #[tokio::test]
    async fn embedded_worker_pulls_and_reports_home_pinned_job() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let ctrl = Arc::new(DistController::new("tok".into(), bus));
        let active = Arc::new(AtomicUsize::new(0));
        let stub = Arc::new(StubAdapter {
            behavior: StubBehavior::Immediate,
            active: active.clone(),
        });

        let args = DistWorkerArgs {
            controller: "http://127.0.0.1:9".into(),
            node: "home".into(),
            token: Some("tok".into()),
            codex_bin: "codex".into(),
            cc_bin: "claude".into(),
            poll_ms: 10,
            tags: vec!["home".into(), "linux".into()],
            max_concurrency: 4,
            projects_root: None,
        };
        let worker = spawn_embedded_worker_with(
            ctrl.clone(),
            args,
            "tok".into(),
            test_secret(),
            make_factory(stub),
            Duration::from_millis(50),
        );

        let job_id = ctrl
            .enqueue_with_project(
                String::new(),
                "home job".into(),
                "run here".into(),
                None,
                Vec::new(),
                Some("home".into()),
                "codex".into(),
                Vec::new(),
                None,
                Some("luban".into()),
                None,
                None,
                None,
            )
            .await;

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let progress = ctrl.pull_progress_after(&job_id, 0).await;
            if progress.done {
                assert_eq!(
                    progress.final_ok,
                    Some(true),
                    "embedded worker 应 report ok=true"
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "embedded worker 未在 2s 内消费并完成 home job"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(active.load(Ordering::SeqCst), 0);
        worker.abort();
    }

    /// max_concurrency=2 时，两个慢 job 应**并行**而非串行：wall clock < 1.5s
    /// 即说明二者重叠跑了；旧的串行实现会 ≥2s。
    #[tokio::test]
    async fn worker_runs_two_jobs_concurrently() {
        let (ctrl, base, srv) = spawn_controller().await;
        let active = Arc::new(AtomicUsize::new(0));
        let stub = Arc::new(StubAdapter {
            behavior: StubBehavior::Sleep(Duration::from_secs(1)),
            active: active.clone(),
        });

        let args = DistWorkerArgs {
            controller: base.clone(),
            node: "nodeP".into(),
            token: Some("tok".into()),
            codex_bin: "codex".into(),
            cc_bin: "claude".into(),
            poll_ms: 50,
            tags: vec![],
            max_concurrency: 2,
            projects_root: None,
        };
        let factory = make_factory(stub);

        // 派两条 job——必须先 enqueue 再 spawn worker，否则 worker 先 register 后
        // capacity_left 可用但还没 job，会进入 poll 死循环（OK，但慢）。
        let j1 = enq_simple(&ctrl, "job1").await;
        let j2 = enq_simple(&ctrl, "job2").await;

        let worker_handle = tokio::spawn(async move {
            // worker_with 是无限循环，测试侧靠 abort 终止。
            let _ = super::run_worker_with(
                args,
                "tok".into(),
                test_secret(),
                factory,
                Duration::from_millis(200),
            )
            .await;
        });

        // 等"两个 job 同时在跑"——peak_active==2 后才开始计 elapsed。
        // 这剔除了 axum/register/pull 的初始化噪音（约 100-200ms），让
        // 测试断言专注 worker 真并发本身的耗时。
        let pickup_deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if active.load(Ordering::SeqCst) >= 2 {
                break;
            }
            if Instant::now() > pickup_deadline {
                worker_handle.abort();
                srv.abort();
                panic!("3s 内 active 未 ≥2（仍是串行 / capacity 拦了 / register 失败）");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // 两 job 都 in-flight 后开始测墙时——并发跑完应 ~1s（Sleep(1s)），
        // 串行得 2s。给 1.5s 容忍调度抖动 + 200ms axum overhead。
        let started = Instant::now();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let s1 = ctrl.job_status(&j1).await;
            let s2 = ctrl.job_status(&j2).await;
            if s1.done && s2.done {
                break;
            }
            if Instant::now() > deadline {
                worker_handle.abort();
                srv.abort();
                panic!(
                    "两 job 在 3s 内未都 done（j1.done={}, j2.done={}）",
                    s1.done, s2.done
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let elapsed = started.elapsed();
        worker_handle.abort();
        srv.abort();

        assert!(
            elapsed < Duration::from_millis(1500),
            "两 job 并发跑（已 in-flight）后应 < 1.5s 完成，实际 {elapsed:?}（说明仍是串行）"
        );
    }

    /// 静默执行（不 push progress）的 job 也应能在 ~heartbeat interval 内被
    /// cancel：controller cancel_job → 下次心跳 ack 带 cancel_pending → worker
    /// 触发 token cancel → adapter.run future 被 select 抢断 → final report 失败。
    #[tokio::test]
    async fn cancel_in_silent_period_via_heartbeat_ack() {
        let (ctrl, base, srv) = spawn_controller().await;
        let active = Arc::new(AtomicUsize::new(0));
        let stub = Arc::new(StubAdapter {
            // 5s "silent"——adapter 内不 push progress，老路径完全没法 cancel。
            behavior: StubBehavior::SleepResponsiveToOuterCancel(Duration::from_secs(5)),
            active: active.clone(),
        });
        let factory = make_factory(stub);

        let args = DistWorkerArgs {
            controller: base.clone(),
            node: "nodeQ".into(),
            token: Some("tok".into()),
            codex_bin: "codex".into(),
            cc_bin: "claude".into(),
            poll_ms: 50,
            tags: vec![],
            max_concurrency: 1,
            projects_root: None,
        };

        let job_id = enq_simple(&ctrl, "silent").await;

        // 心跳 200ms——比生产 10s 短得多，让测试在秒级完成。
        let worker_handle = tokio::spawn(async move {
            let _ = super::run_worker_with(
                args,
                "tok".into(),
                test_secret(),
                factory,
                Duration::from_millis(200),
            )
            .await;
        });

        // 等 active==1（worker pull 到并 spawn task）
        // pickup deadline 5s——并行 cargo test 高 load 下 worker register→pull
        // 链路慢于 2s 的实测概率非 0；放宽到 5s（仍小于 stub 5s sleep + 2s 取消窗）
        let pickup_deadline = Instant::now() + Duration::from_secs(5);
        while active.load(Ordering::SeqCst) == 0 {
            if Instant::now() > pickup_deadline {
                worker_handle.abort();
                srv.abort();
                panic!("worker 5s 内未 pickup silent job");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // 让 worker 进入"adapter 跑了一会但还没有 progress" 的窗口
        tokio::time::sleep(Duration::from_millis(100)).await;
        let cancel_at = Instant::now();
        ctrl.cancel_job(&job_id).await;

        // 等 done——超时上限远小于 5s（说明真被中断）。
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut final_status = None;
        while Instant::now() < deadline {
            let s = ctrl.job_status(&job_id).await;
            if s.done {
                final_status = Some(s);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let elapsed_after_cancel = cancel_at.elapsed();
        worker_handle.abort();
        srv.abort();

        let s = final_status.expect("job 在 2s 内未 done（heartbeat ack cancel 路径未生效）");
        assert!(
            elapsed_after_cancel < Duration::from_millis(800),
            "cancel 后应 ~heartbeat interval (200ms) 内退出，实际 {elapsed_after_cancel:?}"
        );
        assert_eq!(s.ok, Some(false), "cancel 终态应 ok=false");
        let out = s.output.unwrap_or_default();
        assert!(
            out.contains("cancelled"),
            "终态 output 应标注 cancelled, 实际: {out}"
        );
    }

    // ── P1-δ · 韧性 e2e（心跳网络中断恢复 / sweep_stale 重派）──

    /// 在 worker 与真 controller 之间插入一个**仅拦截 `/dist/heartbeat`** 的代理。
    /// 切换 `block_hb` 为 true 时返 503 模拟链路抖动；其他路由始终透传。
    /// 复用真 reqwest::Client 转发，避免重写 axum body 解析。
    async fn spawn_hb_blocking_proxy(
        upstream: String,
    ) -> (
        String,
        Arc<std::sync::atomic::AtomicBool>,
        tokio::task::JoinHandle<()>,
    ) {
        use axum::body::Bytes;
        use axum::http::{HeaderMap, Method, Uri};
        use std::sync::atomic::AtomicBool;

        #[derive(Clone)]
        struct ProxyState {
            upstream: String,
            client: Client,
            block_hb: Arc<AtomicBool>,
        }

        let block_hb = Arc::new(AtomicBool::new(false));
        let state = ProxyState {
            upstream,
            client: Client::new(),
            block_hb: block_hb.clone(),
        };

        // 单一 fallback handler 转发任意路径——只在 path == /dist/heartbeat 且
        // block_hb=true 时返 503，模拟"controller 心跳通道拒包"。
        async fn forward(
            axum::extract::State(st): axum::extract::State<ProxyState>,
            method: Method,
            uri: Uri,
            headers: HeaderMap,
            body: Bytes,
        ) -> axum::response::Response {
            let path_q = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
            if uri.path() == "/dist/heartbeat"
                && st.block_hb.load(std::sync::atomic::Ordering::SeqCst)
            {
                return (StatusCode::SERVICE_UNAVAILABLE, "hb blocked by proxy").into_response();
            }
            let url = format!("{}{}", st.upstream, path_q);
            let mut rb = st.client.request(method, &url);
            // Host 头必须重写或不带——用上游内 reqwest 默认即可，所以剔除。
            for (k, v) in headers.iter() {
                if k.as_str().eq_ignore_ascii_case("host") {
                    continue;
                }
                rb = rb.header(k, v);
            }
            let resp = match rb.body(body).send().await {
                Ok(r) => r,
                Err(e) => {
                    return (StatusCode::BAD_GATEWAY, format!("proxy upstream err: {e}"))
                        .into_response();
                }
            };
            let status = resp.status();
            let bytes = resp.bytes().await.unwrap_or_default();
            (status, bytes).into_response()
        }

        let app = Router::new().fallback(forward).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("proxy bind");
        let addr = listener.local_addr().expect("proxy addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        (format!("http://{addr}"), block_hb, handle)
    }

    /// 网络中断恢复：worker 在心跳通道被拒一段时间后链路恢复，仍能继续上报、
    /// 拉新任务并完成。覆盖"短暂网络抖动不应让 worker 死掉"。
    #[tokio::test]
    async fn worker_heartbeat_network_interruption_recovers() {
        let (ctrl, base, srv) = spawn_controller().await;
        let (proxy_url, hb_blocked, proxy_handle) = spawn_hb_blocking_proxy(base.clone()).await;
        let active = Arc::new(AtomicUsize::new(0));
        let stub = Arc::new(StubAdapter {
            // 每 job 短跑——重点不在并发，而在中断窗口前后两轮 pull 都成功。
            behavior: StubBehavior::Sleep(Duration::from_millis(100)),
            active: active.clone(),
        });
        let factory = make_factory(stub);

        let args = DistWorkerArgs {
            controller: proxy_url.clone(),
            node: "nodeR".into(),
            token: Some("tok".into()),
            codex_bin: "codex".into(),
            cc_bin: "claude".into(),
            poll_ms: 50,
            tags: vec![],
            max_concurrency: 1,
            projects_root: None,
        };

        // 心跳 100ms——中断/恢复窗口要按 hb 间隔决议。
        let hb_interval = Duration::from_millis(100);
        let worker_handle = tokio::spawn(async move {
            let _ = super::run_worker_with(args, "tok".into(), test_secret(), factory, hb_interval)
                .await;
        });

        // 派一条 job，等它跑完，验证基线通畅。
        let j1 = enq_simple(&ctrl, "before-outage").await;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if ctrl.job_status(&j1).await.done {
                break;
            }
            if Instant::now() > deadline {
                worker_handle.abort();
                proxy_handle.abort();
                srv.abort();
                panic!("中断前 baseline job 2s 未完成");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // baseline ok=true
        assert_eq!(ctrl.job_status(&j1).await.ok, Some(true));

        // 等 worker 至少发一次心跳让 controller.last_seen 被刷过——后面验证恢复后能再次刷。
        tokio::time::sleep(Duration::from_millis(200)).await;
        let last_seen_pre = ctrl
            .node_info("nodeR")
            .await
            .and_then(|n| n.last_seen)
            .expect("节点 baseline 后应有 last_seen");

        // 模拟链路中断：心跳通道返 503 持续 ~600ms（≈ 6 个心跳被拒）。
        // 期间 pull / report 仍可走（其它路由透传），但心跳全失败。
        hb_blocked.store(true, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(600)).await;
        // 恢复
        hb_blocked.store(false, std::sync::atomic::Ordering::SeqCst);

        // 给恢复后至少 2 个心跳间隔 + 一点抖动，确保 last_seen 被新心跳刷新。
        tokio::time::sleep(Duration::from_millis(300)).await;
        let last_seen_post = ctrl
            .node_info("nodeR")
            .await
            .and_then(|n| n.last_seen)
            .expect("节点恢复后仍应在 controller 视图中");
        assert!(
            last_seen_post > last_seen_pre,
            "恢复后心跳 last_seen 应有推进，pre={last_seen_pre:?} post={last_seen_post:?}"
        );

        // 派恢复后的 job，验证 worker 仍正常 pickup 且 inflight 对账正确。
        let j2 = enq_simple(&ctrl, "after-outage").await;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if ctrl.job_status(&j2).await.done {
                break;
            }
            if Instant::now() > deadline {
                worker_handle.abort();
                proxy_handle.abort();
                srv.abort();
                panic!("恢复后 job 2s 未完成（worker 卡死或 hb 路径未恢复）");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let s2 = ctrl.job_status(&j2).await;
        assert_eq!(s2.ok, Some(true), "恢复后 job 应 ok=true");

        // 终态：worker 既不残留 inflight，也未被 controller 视作死节点（last_seen 近）。
        // 给最后一次心跳到达 controller 一个窗口（~hb_interval + jitter）。
        tokio::time::sleep(Duration::from_millis(200)).await;
        let info = ctrl.node_info("nodeR").await.unwrap();
        assert!(
            info.inflight.is_empty(),
            "终态 inflight 应清空, 实际: {:?}",
            info.inflight
        );

        worker_handle.abort();
        proxy_handle.abort();
        srv.abort();
    }

    /// sweep_stale 后被孤立 job 重派给 live worker 完成。覆盖"worker 突死，
    /// 已派出的 job 不会永远卡住"——controller 把它捞回 queue 前端，下一个
    /// 可派 worker 拉走完成。
    #[tokio::test]
    async fn sweep_stale_redispatches_orphaned_job() {
        let (ctrl, base, srv) = spawn_controller().await;
        let active = Arc::new(AtomicUsize::new(0));

        // worker_a 跑一个永远不会自然结束的任务（5s sleep），让它在被 sweep
        // 前一直占着 job——这样 sweep 才有"orphan" 可回收。
        let stub_a = Arc::new(StubAdapter {
            behavior: StubBehavior::SleepResponsiveToOuterCancel(Duration::from_secs(5)),
            active: active.clone(),
        });
        let factory_a = make_factory(stub_a);
        let args_a = DistWorkerArgs {
            controller: base.clone(),
            node: "workerA".into(),
            token: Some("tok".into()),
            codex_bin: "codex".into(),
            cc_bin: "claude".into(),
            poll_ms: 50,
            tags: vec![],
            max_concurrency: 1,
            projects_root: None,
        };

        let job_id = enq_simple(&ctrl, "orphaned").await;

        // worker_a 起 + 拉到 job
        let worker_a = tokio::spawn(async move {
            let _ = super::run_worker_with(
                args_a,
                "tok".into(),
                test_secret(),
                factory_a,
                Duration::from_millis(200),
            )
            .await;
        });
        let pickup_deadline = Instant::now() + Duration::from_secs(2);
        while active.load(Ordering::SeqCst) == 0 {
            if Instant::now() > pickup_deadline {
                worker_a.abort();
                srv.abort();
                panic!("workerA 1s 内未 pickup orphaned job");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // 确认 job 真在 controller.inflight + workerA.inflight。
        // node.inflight 由心跳刷新（200ms 间隔），active>0 后第一拍 hb 还没来时
        // node.inflight 仍为空——poll 等到第一次 hb 到达。
        let assert_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let g = ctrl.inner.lock().await;
            let in_global = g.inflight.contains_key(&job_id);
            let in_node = g
                .nodes
                .get("workerA")
                .map(|i| i.inflight == vec![job_id.clone()])
                .unwrap_or(false);
            drop(g);
            if in_global && in_node {
                break;
            }
            if Instant::now() > assert_deadline {
                worker_a.abort();
                srv.abort();
                panic!("sweep 前 job 未同时出现在 controller.inflight + workerA.inflight");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // workerA "死"——abort 心跳/拉取；从此 controller 收不到 A 的心跳。
        worker_a.abort();
        // 等够 hb_interval (200ms) × 2 + 余量——abort 不取消已发出去的 reqwest，
        // 50ms 不足让在飞 hb 抵达 controller 前被丢；workspace 并行高负载下
        // 这条 race 窗口被放大成 50% flaky。500ms 是经验值。
        tokio::time::sleep(Duration::from_millis(500)).await;

        // 把 workerA.last_seen 推到很久以前，绕开"等真 60s sweep timeout"——
        // 不依赖 sweep tick 间隔本身正确（那条由 spawn_sweep_task 单测覆盖），
        // 这里直接调 sweep_stale 验证算法 + 后续派工链路。
        {
            let mut g = ctrl.inner.lock().await;
            let node = g.nodes.get_mut("workerA").expect("A registered");
            node.last_seen = Some(Instant::now() - Duration::from_secs(120));
        }
        let recycled = ctrl
            .sweep_stale(Instant::now(), Duration::from_secs(30))
            .await;
        assert_eq!(recycled.len(), 1, "应仅回收 workerA");
        assert_eq!(recycled[0].0, "workerA");
        assert_eq!(recycled[0].1, vec![job_id.clone()]);
        // job 已从 controller.inflight 移走、回到 queue 前端
        {
            let g = ctrl.inner.lock().await;
            assert!(!g.inflight.contains_key(&job_id), "sweep 后 inflight 应清");
            let head = g.global_queue.front().expect("queue 应有 orphan job");
            assert_eq!(head.id, job_id, "orphan 应 push_front 到 queue 头");
        }

        // worker_b 起，应能拉到同一 job 完成（短任务，ok=true）。
        let active_b = Arc::new(AtomicUsize::new(0));
        let stub_b = Arc::new(StubAdapter {
            behavior: StubBehavior::Sleep(Duration::from_millis(80)),
            active: active_b.clone(),
        });
        let factory_b = make_factory(stub_b);
        let args_b = DistWorkerArgs {
            controller: base.clone(),
            node: "workerB".into(),
            token: Some("tok".into()),
            codex_bin: "codex".into(),
            cc_bin: "claude".into(),
            poll_ms: 50,
            tags: vec![],
            max_concurrency: 1,
            projects_root: None,
        };
        let worker_b = tokio::spawn(async move {
            let _ = super::run_worker_with(
                args_b,
                "tok".into(),
                test_secret(),
                factory_b,
                Duration::from_millis(200),
            )
            .await;
        });

        // workspace 并行测试时 CPU 抢占严重，2s 不够 workerB pickup+exec+report。
        let deadline = Instant::now() + Duration::from_secs(5);
        let final_status = loop {
            let s = ctrl.job_status(&job_id).await;
            if s.done {
                break s;
            }
            if Instant::now() > deadline {
                worker_b.abort();
                srv.abort();
                panic!("workerB 5s 内未完成被回收的 job");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(final_status.ok, Some(true), "重派后应 ok=true");
        assert_eq!(
            final_status.node_id.as_deref(),
            Some("workerB"),
            "终态 node_id 应是 workerB"
        );

        worker_b.abort();
        srv.abort();
    }

    // ── Phase 6 · prometheus metrics ──

    /// 端到端：跑一遍 register → enqueue → pull → report 闭环，断言
    /// 8 个核心 metric name 都出现在 /metrics 文本里。
    #[tokio::test]
    async fn metrics_endpoint_emits_all_core_series() {
        let ctrl = test_ctrl().await;
        ctrl.register("nodeA".into(), vec!["codex".into()], 1).await;
        let job_id = ctrl
            .enqueue(
                "nodeA".into(),
                "T".into(),
                "B".into(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        let _job = ctrl.pull("nodeA").await.expect("job pulled");
        let accepted = ctrl
            .report(DistReportReq {
                node_id: "nodeA".into(),
                job_id: job_id.clone(),
                ok: true,
                output: "done".into(),
                duration_ms: 42,
            })
            .await;
        assert!(accepted);
        let _ = ctrl
            .sweep_stale(Instant::now(), Duration::from_secs(0))
            .await;

        let bytes = ctrl.metrics.encode_text();
        let text = String::from_utf8(bytes).expect("metrics utf8");
        for name in [
            "fuxi_dist_jobs_enqueued_total",
            "fuxi_dist_jobs_dispatched_total",
            "fuxi_dist_jobs_completed_total",
            "fuxi_dist_job_duration_ms",
            "fuxi_dist_workers_registered",
            "fuxi_dist_queue_depth",
            "fuxi_dist_inflight_jobs",
            "fuxi_dist_workers_swept_total",
            "fuxi_dist_workers_max_concurrency",
        ] {
            assert!(text.contains(name), "/metrics 应包含 {name}\n----\n{text}");
        }
        // max_concurrency 来自 register 声明的容量
        assert!(
            text.contains("fuxi_dist_workers_max_concurrency{node_id=\"nodeA\"} 1"),
            "max_concurrency gauge 应反映 register 声明值\n{text}"
        );
        assert!(
            text.contains("fuxi_dist_jobs_enqueued_total{cli=\"codex\"} 1"),
            "enqueue counter 应递增\n{text}"
        );
        assert!(
            text.contains("fuxi_dist_jobs_completed_total{cli=\"codex\",ok=\"true\"} 1"),
            "completed counter 应记 ok=true\n{text}"
        );
    }

    /// histogram bucket 边界：42ms 应落在 le=50 及更大 bucket（cumulative）。
    #[tokio::test]
    async fn metrics_histogram_uses_configured_buckets() {
        let ctrl = test_ctrl().await;
        ctrl.register("nodeA".into(), vec![], 1).await;
        let job_id = ctrl
            .enqueue(
                "nodeA".into(),
                "T".into(),
                "B".into(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        let _ = ctrl.pull("nodeA").await.expect("job pulled");
        ctrl.report(DistReportReq {
            node_id: "nodeA".into(),
            job_id,
            ok: true,
            output: String::new(),
            duration_ms: 42,
        })
        .await;
        let text = String::from_utf8(ctrl.metrics.encode_text()).unwrap();
        assert!(
            text.contains("fuxi_dist_job_duration_ms_bucket{cli=\"codex\",le=\"50\"} 1"),
            "42ms 应落入 le=50 bucket\n{text}"
        );
        assert!(
            text.contains("fuxi_dist_job_duration_ms_bucket{cli=\"codex\",le=\"10\"} 0"),
            "42ms 不应落入 le=10 bucket\n{text}"
        );
    }

    // ── P2 [α]: /dist/event endpoint ────────────────────────────────

    fn ev_agent_responded(text: &str) -> fuxi_core::Event {
        fuxi_core::Event {
            meta: fuxi_core::EventMeta::now(),
            kind: fuxi_core::EventKind::AgentResponded {
                text: text.to_string(),
                artifact_ref: None,
            },
        }
    }

    /// 远端 worker POST /dist/event 一批已注册节点 → controller 转发到本地 bus，
    /// 订阅者能收到原 event（kind 完整保真）。
    #[tokio::test]
    async fn dist_event_publish_to_bus_when_authorized() {
        let (ctrl, base, srv) = spawn_controller().await;
        ctrl.register("remoteA".into(), vec![], 1).await;

        // 同步 subscribe **在 POST 前完成**（race fix；同 cross_node_bus_e2e_*）
        use futures_util::StreamExt;
        let mut s = ctrl.bus().subscribe();
        let probe = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remain.is_zero() {
                    return None;
                }
                if let Ok(Some(Ok(ev))) = tokio::time::timeout(remain, s.next()).await
                    && matches!(ev.kind, fuxi_core::EventKind::AgentResponded { .. })
                {
                    return Some(ev);
                }
            }
        });

        let req = DistEventReq {
            node_id: "remoteA".into(),
            events: vec![ev_agent_responded("hello from remote")],
        };
        let resp = reqwest::Client::new()
            .post(format!("{base}/dist/event"))
            .json(&req)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let body: DistEventResp = resp.json().await.expect("decode");
        assert_eq!(body.accepted, 1);

        let got = probe.await.expect("join").expect("event");
        match got.kind {
            fuxi_core::EventKind::AgentResponded { text, .. } => {
                assert_eq!(text, "hello from remote");
            }
            other => panic!("expect AgentResponded, got {other:?}"),
        }
        srv.abort();
    }

    /// 未 register 的 node_id 一律 403——拒收陌生流量是 P2 安全前提。
    #[tokio::test]
    async fn dist_event_rejects_unregistered_node_with_403() {
        let (_ctrl, base, srv) = spawn_controller().await;
        let req = DistEventReq {
            node_id: "ghost-node".into(),
            events: vec![ev_agent_responded("x")],
        };
        let resp = reqwest::Client::new()
            .post(format!("{base}/dist/event"))
            .json(&req)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), reqwest::StatusCode::FORBIDDEN);
        srv.abort();
    }

    /// HMAC 缺签名 header → 401。连业务校验（node 是否 register）都不该走到。
    /// 起带 HMAC layer 的 router——`router_with_hmac` 的薄端到端验证。
    #[tokio::test]
    async fn dist_event_without_hmac_headers_rejected_with_401() {
        use crate::dist_auth::{HmacGate, HmacSecret};
        let bus = EventBus::with_memory_store().await.expect("bus");
        let ctrl = Arc::new(DistController::new("tok".into(), bus));
        ctrl.register("remoteA".into(), vec![], 1).await;
        let gate = HmacGate::new(HmacSecret::new("test-secret".into()));
        let app = router_with_hmac(ctrl, gate);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let srv = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let req = DistEventReq {
            node_id: "remoteA".into(),
            events: vec![ev_agent_responded("x")],
        };
        let resp = reqwest::Client::new()
            .post(format!("http://{addr}/dist/event"))
            .json(&req)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
        srv.abort();
    }

    /// batch：一次 POST 多条 event 全部 publish；accepted 正确反映条数。
    #[tokio::test]
    async fn dist_event_handles_batch() {
        let (ctrl, base, srv) = spawn_controller().await;
        ctrl.register("remoteA".into(), vec![], 1).await;

        // 同步 subscribe **在 POST 前完成**（race fix；同 cross_node_bus_e2e_*）
        use futures_util::StreamExt;
        let mut s = ctrl.bus().subscribe();
        let probe = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            let mut got = Vec::new();
            loop {
                let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remain.is_zero() || got.len() >= 3 {
                    return got;
                }
                if let Ok(Some(Ok(ev))) = tokio::time::timeout(remain, s.next()).await
                    && let fuxi_core::EventKind::AgentResponded { text, .. } = &ev.kind
                {
                    got.push(text.clone());
                }
            }
        });

        let req = DistEventReq {
            node_id: "remoteA".into(),
            events: vec![
                ev_agent_responded("a"),
                ev_agent_responded("b"),
                ev_agent_responded("c"),
            ],
        };
        let resp = reqwest::Client::new()
            .post(format!("{base}/dist/event"))
            .json(&req)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let body: DistEventResp = resp.json().await.expect("decode");
        assert_eq!(body.accepted, 3);

        let got = probe.await.expect("join");
        assert_eq!(got, vec!["a".to_string(), "b".to_string(), "c".to_string()]);

        // metrics 计数对齐：3 条 received，0 条 failed
        let text = String::from_utf8(ctrl.metrics.encode_text()).unwrap();
        assert!(
            text.contains("fuxi_dist_remote_events_received_total{node_id=\"remoteA\"} 3"),
            "应记录 3 条 remote event；metrics:\n{text}"
        );
        srv.abort();
    }

    // ── γ：worker 子门客 stdout → translate → NetworkBusClient 桥接 ──
    //
    // 验路 Y 主张「不动 worker cancel/Child 所有权，只在解析层加第二个消费者」。
    // 直接喂 raw stdout 行给 codex_publish_line / cc_publish_line，drain client 队列
    // 断言事件类型 + 字段——不必起 mock controller / fake 子进程，runtime 开销 0。

    fn gamma_dummy_bus() -> NetworkBusClient {
        // controller 不可达 + 0 retry：测试只断 enqueue 入队，不该有 HTTP 出栈。
        NetworkBusClient::with_config(
            Client::new(),
            "http://127.0.0.1:1".into(),
            "tok".into(),
            std::sync::Arc::new(crate::dist_auth::HmacSecret::new("tok".into())),
            "node-test".into(),
            64,
            128, // batch_size 故意大，防止 enqueue 自动触发 flush_signal
            Duration::from_secs(60),
            vec![],
        )
    }

    /// codex AgentMessage 行 → bus 拿到 AgentResponded（路 Y 最小切片）。
    #[tokio::test]
    async fn codex_publish_line_emits_agent_responded_to_bus() {
        let bus = gamma_dummy_bus();
        let agent = AgentId::new();
        let task = Some(TaskId::new());
        let mut state = fuxi_agent_codex::TranslateState::new();
        let line =
            r#"{"type":"item.completed","item":{"id":"i","type":"agent_message","text":"hi"}}"#;
        super::codex_publish_line(&bus, line, agent, task, &mut state, Some(99)).await;
        let drained = bus.take_batch(16).await;
        assert_eq!(drained.len(), 1, "agent_message 单行应只产 1 条 Event");
        match &drained[0].kind {
            EventKind::AgentResponded { text, .. } => assert_eq!(text, "hi"),
            other => panic!("expected AgentResponded, got {other:?}"),
        }
        assert_eq!(drained[0].meta.agent, Some(agent));
        assert_eq!(drained[0].meta.task, task);
    }

    /// cc tool_use 行 → bus 拿到 ToolCallStarted。
    #[tokio::test]
    async fn cc_publish_line_emits_tool_call_started_to_bus() {
        let bus = gamma_dummy_bus();
        let agent = AgentId::new();
        let task = Some(TaskId::new());
        let mut state = fuxi_agent_cc::TranslateState::new();
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"cmd":"ls"}}]}}"#;
        super::cc_publish_line(&bus, line, agent, task, &mut state, Some(42)).await;
        let drained = bus.take_batch(16).await;
        assert_eq!(drained.len(), 1);
        match &drained[0].kind {
            EventKind::ToolCallStarted { tool, args } => {
                assert_eq!(tool, "Bash");
                assert_eq!(args["cmd"], "ls");
            }
            other => panic!("expected ToolCallStarted, got {other:?}"),
        }
    }

    /// Decision 13 sentinel：`AssistantText` 行装 `_fuxi:request_review` JSON →
    /// bus 拿到 AgentRequestReview（**不**是 AgentResponded——sentinel suppresses）。
    #[tokio::test]
    async fn cc_publish_line_routes_request_review_sentinel_to_bus() {
        let bus = gamma_dummy_bus();
        let agent = AgentId::new();
        let task = Some(TaskId::new());
        let mut state = fuxi_agent_cc::TranslateState::new();
        let sentinel_line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"{\"_fuxi\":\"request_review\",\"kind\":\"code_change\",\"summary\":\"小绿了\",\"artifact_ref\":\"sha:abc\"}"}]}}"#;
        super::cc_publish_line(&bus, sentinel_line, agent, task, &mut state, Some(7)).await;
        let drained = bus.take_batch(16).await;
        assert_eq!(drained.len(), 1, "sentinel suppresses AgentResponded");
        match &drained[0].kind {
            EventKind::AgentRequestReview {
                deliverable_kind,
                summary,
                artifact_ref,
                ..
            } => {
                use fuxi_core::event::DeliverableKind;
                assert_eq!(*deliverable_kind, DeliverableKind::CodeChange);
                assert_eq!(summary, "小绿了");
                assert_eq!(artifact_ref.as_deref(), Some("sha:abc"));
            }
            other => panic!("expected AgentRequestReview, got {other:?}"),
        }
    }

    /// per-job state 隔离：上 job AssistantText 置 `responded_this_turn=true`
    /// 不该污染下 job 的冷场景 ResultSuccess（否则 home 完全看不到回复）。
    /// 走两个 fresh TranslateState 验证「new() 调用 = 状态边界」。
    #[tokio::test]
    async fn cc_publish_line_per_job_state_does_not_leak() {
        let bus = gamma_dummy_bus();
        let agent = AgentId::new();
        let task = Some(TaskId::new());

        // job 1：Assistant 流式回复后置 responded_this_turn=true
        let mut state1 = fuxi_agent_cc::TranslateState::new();
        super::cc_publish_line(
            &bus,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"job1 reply"}]}}"#,
            agent,
            task,
            &mut state1,
            Some(1),
        )
        .await;
        let _ = bus.take_batch(16).await; // drain job1 events

        // job 2：fresh state——result-only 冷场景（cc 极短回复路径）必须发 AgentResponded
        let mut state2 = fuxi_agent_cc::TranslateState::new();
        super::cc_publish_line(
            &bus,
            r#"{"type":"result","subtype":"success","result":"job2 reply"}"#,
            agent,
            task,
            &mut state2,
            Some(2),
        )
        .await;
        let drained = bus.take_batch(16).await;
        // ResultSuccess 冷场景 → TaskStateChanged + AgentResponded（per cc translate 逻辑）
        assert_eq!(
            drained.len(),
            2,
            "fresh state job2 应发 TaskStateChanged + AgentResponded（冷场景）"
        );
        let has_responded = drained.iter().any(
            |e| matches!(&e.kind, EventKind::AgentResponded { text, .. } if text == "job2 reply"),
        );
        assert!(
            has_responded,
            "per-job 新 TranslateState 不应继承上 job 的 responded_this_turn 状态"
        );
    }

    /// 坏 JSON 不该让 worker 崩——translate path 跟 push_progress path 一样静默 swallow。
    #[tokio::test]
    async fn codex_publish_line_swallows_invalid_json() {
        let bus = gamma_dummy_bus();
        let mut state = fuxi_agent_codex::TranslateState::new();
        super::codex_publish_line(&bus, "{not json", AgentId::new(), None, &mut state, None).await;
        assert_eq!(bus.queue_len().await, 0, "坏行不该入队");
    }

    // ── δ #4 P2 v1 收尾：cross-node bus e2e（EventMeta.source_node_id 全链路）──

    /// δ #4 cross_node_bus_e2e_marks_source_node_id：HTTP 路径全链路验证——
    /// 远端 worker POST /dist/event → controller stamp `source_node_id =
    /// Some(node_id)` → bus broadcast → home 订阅者收到的事件 meta 带 source_node_id。
    /// **不依赖 NetworkBusClient 的 transport 细节**——直接 HTTP POST 模拟 worker
    /// 任何 HTTP 客户端的可观察行为；β 的 client 走的就是这条 wire。
    #[tokio::test]
    async fn cross_node_bus_e2e_marks_source_node_id() {
        let (ctrl, base, srv) = spawn_controller().await;
        ctrl.register("far".into(), vec![], 1).await;

        // 同步 subscribe **在 POST 前完成**——broadcast 不留历史，spawn 内 subscribe
        // 有 race（spawn 首次 poll 可能晚于 publish）。早期版本靠 `sleep(20ms)` 让
        // spawn 起来，高 load 下不可靠（用户实测 10/10 fail）。改 outer subscribe，
        // BroadcastStream 跨 await/move 安全。
        use futures_util::StreamExt;
        let mut s = ctrl.bus().subscribe();
        let probe = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remain.is_zero() {
                    return None;
                }
                if let Ok(Some(Ok(ev))) = tokio::time::timeout(remain, s.next()).await
                    && matches!(ev.kind, fuxi_core::EventKind::AgentResponded { .. })
                {
                    return Some(ev);
                }
            }
        });

        // worker 发的事件 meta.source_node_id 故意 None——controller 应**覆盖**它，
        // 不能依赖 worker 端自填（信任域 = controller 边界）。
        let req = DistEventReq {
            node_id: "far".into(),
            events: vec![ev_agent_responded("远端来的")],
        };
        let resp = reqwest::Client::new()
            .post(format!("{base}/dist/event"))
            .json(&req)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), reqwest::StatusCode::OK);

        let got = probe.await.expect("join").expect("event");
        assert_eq!(
            got.meta.source_node_id.as_deref(),
            Some("far"),
            "controller 必须 stamp source_node_id"
        );
        match got.kind {
            fuxi_core::EventKind::AgentResponded { text, .. } => {
                assert_eq!(text, "远端来的");
            }
            other => panic!("expect AgentResponded, got {other:?}"),
        }
        srv.abort();
    }

    /// δ #4 cross_node_bus_e2e_overrides_worker_supplied_source_node_id：
    /// 即便 worker 自己塞了 `source_node_id = "imposter"`，controller 仍会
    /// 覆盖成 endpoint 的 node_id。防止伪造。
    #[tokio::test]
    async fn cross_node_bus_e2e_overrides_worker_supplied_source_node_id() {
        let (ctrl, base, srv) = spawn_controller().await;
        ctrl.register("realnode".into(), vec![], 1).await;

        use futures_util::StreamExt;
        let mut s = ctrl.bus().subscribe();
        let probe = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                let remain = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remain.is_zero() {
                    return None;
                }
                if let Ok(Some(Ok(ev))) = tokio::time::timeout(remain, s.next()).await
                    && matches!(ev.kind, fuxi_core::EventKind::AgentResponded { .. })
                {
                    return Some(ev);
                }
            }
        });

        let mut spoofed = ev_agent_responded("trying to spoof");
        spoofed.meta.source_node_id = Some("imposter".into());
        let req = DistEventReq {
            node_id: "realnode".into(),
            events: vec![spoofed],
        };
        let resp = reqwest::Client::new()
            .post(format!("{base}/dist/event"))
            .json(&req)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), reqwest::StatusCode::OK);

        let got = probe.await.expect("join").expect("event");
        assert_eq!(
            got.meta.source_node_id.as_deref(),
            Some("realnode"),
            "controller 必须以 endpoint 的 node_id 为准，覆盖 worker 自填"
        );
        srv.abort();
    }

    /// δ #4 cross_node_bus_e2e_persists_source_node_id：远端事件经 controller
    /// 落 SQLite，replay 出来 source_node_id 字段保真——TUI 重启后回放历史
    /// 事件仍能区分本地/远端。
    #[tokio::test]
    async fn cross_node_bus_e2e_persists_source_node_id() {
        let (ctrl, base, srv) = spawn_controller().await;
        ctrl.register("far".into(), vec![], 1).await;
        let bus = ctrl.bus().clone();

        let req = DistEventReq {
            node_id: "far".into(),
            events: vec![ev_agent_responded("落库测试")],
        };
        let resp = reqwest::Client::new()
            .post(format!("{base}/dist/event"))
            .json(&req)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), reqwest::StatusCode::OK);

        // 等 EventBus writer 任务把事件落库——内部 mpsc + 串行 writer。
        // 用 deadline 轮询比固定 sleep 稳：CI 慢机器也能等到。
        use futures_util::StreamExt;
        use fuxi_events::ReplayCursor;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let got = loop {
            let mut stream = bus.replay(ReplayCursor::Beginning, false);
            let mut all = Vec::new();
            while let Some(Ok(ev)) = stream.next().await {
                all.push(ev);
            }
            if let Some(ev) = all
                .into_iter()
                .find(|e| matches!(e.kind, fuxi_core::EventKind::AgentResponded { .. }))
            {
                break ev;
            }
            if std::time::Instant::now() > deadline {
                panic!("3s 内没等到事件落库");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert_eq!(
            got.meta.source_node_id.as_deref(),
            Some("far"),
            "SQLite payload 必须保留 source_node_id（v1 reside 在 JSON blob，不加专列）"
        );
        srv.abort();
    }

    // ── path 3 γ：HMAC e2e + replay/skew/tampering 攻击防御 ──
    //
    // 起带 `router_with_hmac` 的 controller，针对 6 类攻击 + 2 类合法路径验证：
    //   A. e2e full cycle（β signed_post/get 全链路）
    //   B. tampering（body / method / path 篡改）
    //   C. replay（同 sig+ts+nonce 二发）
    //   D. clock skew（容忍窗内 / 窗外）
    //   E. config（HmacSecret::from_env env 缺值拒绝）
    //
    // β 的 wrapper 把签名 + 发送原子化，攻击侧要"先签后改"必须直接调 α 的
    // `sign_request` 算 sig，再手搓 reqwest::Request 加 3 header 发——helper
    // `build_signed_attack_request` 让所有攻击 case 都通过这一个入口构造。

    /// γ：起带 HMAC layer 的 controller。secret 注入 router，client 端用
    /// 同一份 secret 算签名 → 走中间件验证。
    async fn spawn_controller_signed() -> (
        Arc<DistController>,
        String,
        Arc<crate::dist_auth::HmacSecret>,
        tokio::task::JoinHandle<()>,
    ) {
        use crate::dist_auth::{HmacGate, HmacSecret};
        let bus = EventBus::with_memory_store().await.expect("bus");
        let ctrl = Arc::new(DistController::new("tok".into(), bus));
        let secret = Arc::new(HmacSecret::new("γ-test-secret-very-long-key-32b+".into()));
        let gate = HmacGate::new(HmacSecret::new("γ-test-secret-very-long-key-32b+".into()));
        let app = router_with_hmac(ctrl.clone(), gate);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        (ctrl, format!("http://{addr}"), secret, handle)
    }

    /// γ：手搓"已签名 + 可篡改"的 reqwest 请求。
    ///
    /// `sign_method` / `sign_path` / `sign_body` 是签名喂进 canonical 的字段；
    /// `wire_method` / `wire_url` / `wire_body` 是真正发出去的请求字段——
    /// 攻击 case 故意让两套不一致，验证 middleware 拦得住。
    ///
    /// 合法 case 直接传同样的值（用 `sign_request_legitimate` 包一层）。
    #[allow(clippy::too_many_arguments)]
    fn build_signed_attack_request(
        client: &reqwest::Client,
        secret: &crate::dist_auth::HmacSecret,
        sign_method: &str,
        sign_path: &str,
        sign_body: &[u8],
        ts_ms: u64,
        nonce: &str,
        wire_method: reqwest::Method,
        wire_url: &str,
        wire_body: Vec<u8>,
    ) -> reqwest::Request {
        use crate::dist_auth::{X_FUXI_NONCE, X_FUXI_SIGNATURE, X_FUXI_TIMESTAMP, sign_request};
        let sig = sign_request(secret, sign_method, sign_path, ts_ms, nonce, sign_body);
        // 注：HTTP header 值必须 ASCII（RFC 7230），axum middleware 的
        // `to_str().ok()` 会对非 ASCII 字节返 None → 误判 MissingHeader 401。
        // 测试用的 nonce 字面量保持 ASCII（避免中文/Greek 标识混入 header）。
        client
            .request(wire_method, wire_url)
            .body(wire_body)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(X_FUXI_TIMESTAMP, ts_ms.to_string())
            .header(X_FUXI_NONCE, nonce)
            .header(X_FUXI_SIGNATURE, sig)
            .build()
            .expect("build attack request")
    }

    // ─── A：e2e full cycle ───────────────────────────────────────

    /// A：register POST + pull GET + progress POST + report POST，全走 β
    /// 的 signed_post / signed_get → 全链路 200。这是"正路通"的最小验证。
    /// pull 在无 job 入队时返回空 job——只验 200 即可，job 内容由别的测试覆盖。
    #[tokio::test]
    async fn hmac_e2e_signed_register_pull_report_full_cycle() {
        use crate::dist_auth_client::{signed_get, signed_post};
        let (ctrl, base, secret, srv) = spawn_controller_signed().await;
        let client = reqwest::Client::new();

        // 1) register
        let register_req = DistRegisterReq {
            node_id: "γnode".into(),
            tags: vec!["cc".into()],
            max_concurrency: 1,
        };
        let r = signed_post(
            &client,
            &secret,
            &format!("{base}/dist/register"),
            &register_req,
        )
        .await
        .expect("register send");
        assert_eq!(r.status(), reqwest::StatusCode::OK, "register 必须 200");

        // 2) enqueue（也走 HMAC，证明所有 endpoint 一致鉴权）
        let enq_req = DistEnqueueReq {
            node_id: String::new(),
            title: "γjob".into(),
            body: "do work".into(),
            system_prompt: None,
            required_tags: vec![],
            pinned_node: None,
            cli: String::new(),
            allowed_tools: vec![],
            task_id: None,
            role: None,
            project: None,
            ephemeral_task: None,
            topic_id: None,
        };
        let r = signed_post(&client, &secret, &format!("{base}/dist/enqueue"), &enq_req)
            .await
            .expect("enqueue send");
        assert_eq!(r.status(), reqwest::StatusCode::OK);
        let enq_resp: DistEnqueueResp = r.json().await.expect("decode enqueue");
        let job_id = enq_resp.job_id;

        // 3) pull
        let r = signed_get(
            &client,
            &secret,
            &format!("{base}/dist/pull"),
            &[("token", "tok"), ("node_id", "γnode")],
        )
        .await
        .expect("pull send");
        assert_eq!(r.status(), reqwest::StatusCode::OK, "pull 必须 200");
        let pull_resp: DistPullResp = r.json().await.expect("decode pull");
        let pulled_id = pull_resp.job.expect("应 pull 到刚 enqueue 的 job").id;

        // 4) progress
        let progress_req = DistProgressReq {
            node_id: "γnode".into(),
            job_id: pulled_id.clone(),
            chunks: vec![ProgressPush {
                kind: ProgressKind::AssistantText,
                text: "halfway".into(),
            }],
        };
        let r = signed_post(
            &client,
            &secret,
            &format!("{base}/dist/progress"),
            &progress_req,
        )
        .await
        .expect("progress send");
        assert_eq!(r.status(), reqwest::StatusCode::OK);
        let ack: DistProgressAck = r.json().await.expect("decode progress");
        assert_eq!(ack.accepted, 1);

        // 5) report
        let report_req = DistReportReq {
            node_id: "γnode".into(),
            job_id: pulled_id.clone(),
            ok: true,
            output: "done".into(),
            duration_ms: 42,
        };
        let r = signed_post(
            &client,
            &secret,
            &format!("{base}/dist/report"),
            &report_req,
        )
        .await
        .expect("report send");
        assert_eq!(r.status(), reqwest::StatusCode::OK);

        // controller 视角：job 已终态
        let s = ctrl.job_status(&job_id).await;
        assert!(s.done && s.ok == Some(true));

        srv.abort();
    }

    // ─── B：tampering 攻击 ───────────────────────────────────────

    /// B-1：worker 合法签名后 attacker MITM 改 body 1 byte → 401。
    /// 模拟方式：手算 sig（用真 body）但发送时塞篡改后的 body。
    #[tokio::test]
    async fn hmac_e2e_tampered_body_rejected() {
        use crate::dist_auth::now_unix_ms;
        let (_ctrl, base, secret, srv) = spawn_controller_signed().await;
        let client = reqwest::Client::new();

        let real_body = serde_json::to_vec(&DistRegisterReq {
            node_id: "honest".into(),
            tags: vec![],
            max_concurrency: 1,
        })
        .unwrap();
        let mut tampered = real_body.clone();
        // 改 1 byte——找第一个 '"' 后面的字符替换成 '!'，破坏 JSON 但仍能进 middleware
        let pos = tampered.iter().position(|&b| b == b'"').unwrap_or(0) + 1;
        tampered[pos] = b'!';
        assert_ne!(real_body, tampered, "篡改未生效");

        let req = build_signed_attack_request(
            &client,
            &secret,
            "POST",
            "/dist/register",
            &real_body, // 用 real_body 算签
            now_unix_ms(),
            "gnonce-body",
            reqwest::Method::POST,
            &format!("{base}/dist/register"),
            tampered, // 但发 tampered body
        );
        let resp = client.execute(req).await.expect("send");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "篡改 body 后 middleware 必须 401"
        );
        srv.abort();
    }

    /// B-2：sig 是 POST 的，attacker 改成 PUT → 401。
    /// PUT 无 route 命中也是 401（middleware 在路由前），目标是验 method 进 canonical。
    /// 用 axum 真实存在的 method 路径——POST /dist/cancel 改 PUT 同 path 看 middleware 是否拦下。
    #[tokio::test]
    async fn hmac_e2e_tampered_method_rejected() {
        use crate::dist_auth::now_unix_ms;
        let (_ctrl, base, secret, srv) = spawn_controller_signed().await;
        let client = reqwest::Client::new();

        let body = serde_json::to_vec(&DistCancelReq {
            job_id: "any".into(),
        })
        .unwrap();
        // 签 POST，发 PUT
        let req = build_signed_attack_request(
            &client,
            &secret,
            "POST",
            "/dist/cancel",
            &body,
            now_unix_ms(),
            "gnonce-method",
            reqwest::Method::PUT,
            &format!("{base}/dist/cancel"),
            body.clone(),
        );
        let resp = client.execute(req).await.expect("send");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "method substitution 必须 401（PUT≠POST canonical）"
        );
        srv.abort();
    }

    /// B-3：endpoint substitution——sig 是 /dist/heartbeat 的，attacker
    /// 拷贝同样的 sig + headers 路由到 /dist/event → 401。task spec 高亮的关键测试。
    #[tokio::test]
    async fn hmac_e2e_tampered_path_rejected() {
        use crate::dist_auth::now_unix_ms;
        let (ctrl, base, secret, srv) = spawn_controller_signed().await;
        ctrl.register("substituted".into(), vec![], 1).await;
        let client = reqwest::Client::new();

        // body 同时是合法的 heartbeat 和合法的 event payload——只验签名拒，
        // 排除 body decode 失败导致的 401 假阳。用一个简单 heartbeat req。
        let hb_body = serde_json::to_vec(&DistHeartbeatReq {
            node_id: "substituted".into(),
            inflight: vec![],
            tags: None,
            max_concurrency: None,
        })
        .unwrap();
        let req = build_signed_attack_request(
            &client,
            &secret,
            "POST",
            "/dist/heartbeat", // 签 heartbeat
            &hb_body,
            now_unix_ms(),
            "gnonce-path",
            reqwest::Method::POST,
            &format!("{base}/dist/event"), // 发到 event
            hb_body.clone(),
        );
        let resp = client.execute(req).await.expect("send");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "endpoint substitution 必须 401（path 进 canonical → sig 不匹配）"
        );
        srv.abort();
    }

    // ─── C：replay 攻击 ─────────────────────────────────────────

    /// C-1：相同 sig + timestamp + nonce 二发 → 第二次 401（NonceCache 命中）。
    #[tokio::test]
    async fn hmac_e2e_replayed_request_within_window_rejected() {
        use crate::dist_auth::now_unix_ms;
        let (_ctrl, base, secret, srv) = spawn_controller_signed().await;
        let client = reqwest::Client::new();

        let body = serde_json::to_vec(&DistRegisterReq {
            node_id: "replay-target".into(),
            tags: vec![],
            max_concurrency: 1,
        })
        .unwrap();
        let ts = now_unix_ms();
        let nonce = "gnonce-replay-fixed";

        // 第一次发：build → execute
        let req1 = build_signed_attack_request(
            &client,
            &secret,
            "POST",
            "/dist/register",
            &body,
            ts,
            nonce,
            reqwest::Method::POST,
            &format!("{base}/dist/register"),
            body.clone(),
        );
        let resp1 = client.execute(req1).await.expect("send 1");
        assert_eq!(resp1.status(), reqwest::StatusCode::OK, "首次合法应 200");

        // 第二次：完全相同的参数（同 ts、同 nonce、同 sig、同 body） → 401
        let req2 = build_signed_attack_request(
            &client,
            &secret,
            "POST",
            "/dist/register",
            &body,
            ts,
            nonce,
            reqwest::Method::POST,
            &format!("{base}/dist/register"),
            body.clone(),
        );
        let resp2 = client.execute(req2).await.expect("send 2");
        assert_eq!(
            resp2.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "replay（同 nonce）必须 401"
        );
        srv.abort();
    }

    /// C-2：timestamp = now - 10min（超 5min skew tolerance）→ 401。
    #[tokio::test]
    async fn hmac_e2e_old_timestamp_rejected_after_skew() {
        use crate::dist_auth::now_unix_ms;
        let (_ctrl, base, secret, srv) = spawn_controller_signed().await;
        let client = reqwest::Client::new();

        let body = serde_json::to_vec(&DistRegisterReq {
            node_id: "old-ts".into(),
            tags: vec![],
            max_concurrency: 1,
        })
        .unwrap();
        let stale_ts = now_unix_ms().saturating_sub(10 * 60 * 1000); // 10 分钟前
        let req = build_signed_attack_request(
            &client,
            &secret,
            "POST",
            "/dist/register",
            &body,
            stale_ts,
            "gnonce-old-ts",
            reqwest::Method::POST,
            &format!("{base}/dist/register"),
            body.clone(),
        );
        let resp = client.execute(req).await.expect("send");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "10 分钟前的 timestamp 超 5min skew → 401"
        );
        srv.abort();
    }

    // ─── D：clock skew tolerance ────────────────────────────────

    /// D-1：timestamp = now - 4min（5min 容忍窗内）→ 200。
    #[tokio::test]
    async fn hmac_e2e_request_within_5min_skew_accepted() {
        use crate::dist_auth::now_unix_ms;
        let (_ctrl, base, secret, srv) = spawn_controller_signed().await;
        let client = reqwest::Client::new();

        let body = serde_json::to_vec(&DistRegisterReq {
            node_id: "past-skew".into(),
            tags: vec![],
            max_concurrency: 1,
        })
        .unwrap();
        let past_ts = now_unix_ms().saturating_sub(4 * 60 * 1000); // 4 分钟前
        let req = build_signed_attack_request(
            &client,
            &secret,
            "POST",
            "/dist/register",
            &body,
            past_ts,
            "gnonce-past-skew",
            reqwest::Method::POST,
            &format!("{base}/dist/register"),
            body.clone(),
        );
        let resp = client.execute(req).await.expect("send");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::OK,
            "4 分钟前在 5min 容忍窗内 → 200"
        );
        srv.abort();
    }

    /// D-2：timestamp = now + 4min（worker 时钟走快，未来在窗内）→ 200。
    #[tokio::test]
    async fn hmac_e2e_future_timestamp_within_skew_accepted() {
        use crate::dist_auth::now_unix_ms;
        let (_ctrl, base, secret, srv) = spawn_controller_signed().await;
        let client = reqwest::Client::new();

        let body = serde_json::to_vec(&DistRegisterReq {
            node_id: "future-skew".into(),
            tags: vec![],
            max_concurrency: 1,
        })
        .unwrap();
        let future_ts = now_unix_ms() + 4 * 60 * 1000; // 未来 4 分钟
        let req = build_signed_attack_request(
            &client,
            &secret,
            "POST",
            "/dist/register",
            &body,
            future_ts,
            "gnonce-future-skew",
            reqwest::Method::POST,
            &format!("{base}/dist/register"),
            body.clone(),
        );
        let resp = client.execute(req).await.expect("send");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::OK,
            "未来 4 分钟在 5min 容忍窗内（worker 时钟走快） → 200"
        );
        srv.abort();
    }

    // ─── E：config-fail（HmacSecret::from_env 缺 env） ──────────

    /// E：env 未设 → `from_env()` 返 `MissingSecretEnv` 携 env 名。
    /// 单元测试代替 spawn cargo run binary——daemon main 自决 panic vs eprintln+exit，
    /// 测试只验"config 这层会拦下"。
    ///
    /// edition 2024：`std::env::set_var/remove_var` 是 unsafe。本测试是
    /// crate 内**唯一**触碰此 env 的测试，无并发风险；保存/恢复以避免污染
    /// `cargo test` 跑下一个测试用例时的环境。
    #[test]
    fn hmac_secret_from_env_returns_err_when_unset() {
        use crate::dist_auth::{FUXI_DIST_HMAC_SECRET_ENV, HmacError, HmacSecret};
        // 保存当前值（dev 可能 export 过）
        let saved = std::env::var(FUXI_DIST_HMAC_SECRET_ENV).ok();
        // SAFETY: 测试单线程访问 env；无并发 reader；finally 恢复。
        unsafe {
            std::env::remove_var(FUXI_DIST_HMAC_SECRET_ENV);
        }
        let r = HmacSecret::from_env();
        // 恢复
        unsafe {
            match saved {
                Some(v) => std::env::set_var(FUXI_DIST_HMAC_SECRET_ENV, v),
                None => std::env::remove_var(FUXI_DIST_HMAC_SECRET_ENV),
            }
        }
        // HmacSecret 不实现 Debug（防止 secret 字节意外打印），所以直接 match 而非 expect_err。
        match r {
            Err(HmacError::MissingSecretEnv(name)) => {
                assert_eq!(name, FUXI_DIST_HMAC_SECRET_ENV);
            }
            Err(other) => panic!("期望 MissingSecretEnv，得到 {other:?}"),
            Ok(_) => panic!("env 已 unset 但 from_env 仍返 Ok"),
        }
    }

    // ─── path 4 α：DistController 与 JobPersistence 的集成 ───
    //
    // dist_persistence 自身的 5 条 TDD 测试在 `crate::dist_persistence::tests`；
    // 这里只测"DistController 注入 persistence 后 mutating ops 真的写盘 +
    // restore_from_persistence 真的塞回 queue"，避免 wiring drift。

    use crate::dist_persistence::{JobPersistence, STATE_CANCELLED, STATE_DONE, STATE_INFLIGHT};

    /// path 4 α #1：with_persistence 注入后 enqueue 在 SQLite 留行 + state=queued。
    #[tokio::test]
    async fn controller_enqueue_writes_persistence_row() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let p = Arc::new(JobPersistence::connect_memory().await.expect("p"));
        let ctrl = Arc::new(DistController::new("tok".into(), bus).with_persistence(p.clone()));
        let job_id = ctrl
            .enqueue(
                "hint".into(),
                "T".into(),
                "B".into(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        let row = p.job_row(&job_id).await.expect("row").expect("exists");
        assert_eq!(row.state, "queued");
        assert!(row.assignee.is_none());
    }

    /// path 4 α #2：pull → SQLite state=inflight, assignee=node。
    #[tokio::test]
    async fn controller_pull_writes_inflight_to_persistence() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let p = Arc::new(JobPersistence::connect_memory().await.expect("p"));
        let ctrl = Arc::new(DistController::new("tok".into(), bus).with_persistence(p.clone()));
        ctrl.register("nodeA".into(), vec![], 1).await;
        let job_id = ctrl
            .enqueue(
                "hint".into(),
                "T".into(),
                "B".into(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        let pulled = ctrl.pull("nodeA").await.expect("job");
        assert_eq!(pulled.id, job_id);
        let row = p.job_row(&job_id).await.expect("row").expect("exists");
        assert_eq!(row.state, STATE_INFLIGHT);
        assert_eq!(row.assignee.as_deref(), Some("nodeA"));
    }

    /// path 4 α #3：report → SQLite state=done, ok 标志正确。
    #[tokio::test]
    async fn controller_report_writes_done_to_persistence() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let p = Arc::new(JobPersistence::connect_memory().await.expect("p"));
        let ctrl = Arc::new(DistController::new("tok".into(), bus).with_persistence(p.clone()));
        ctrl.register("nodeA".into(), vec![], 1).await;
        let job_id = ctrl
            .enqueue(
                "h".into(),
                "T".into(),
                "B".into(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        ctrl.pull("nodeA").await.expect("job");
        let accepted = ctrl
            .report(DistReportReq {
                node_id: "nodeA".into(),
                job_id: job_id.clone(),
                ok: false,
                output: "boom".into(),
                duration_ms: 12,
            })
            .await;
        assert!(accepted);
        let row = p.job_row(&job_id).await.expect("row").expect("exists");
        assert_eq!(row.state, STATE_DONE);
        assert_eq!(row.ok, Some(0));
    }

    /// path 4 α #4：cancel → SQLite state=cancelled。
    #[tokio::test]
    async fn controller_cancel_writes_cancelled_to_persistence() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let p = Arc::new(JobPersistence::connect_memory().await.expect("p"));
        let ctrl = Arc::new(DistController::new("tok".into(), bus).with_persistence(p.clone()));
        let job_id = ctrl
            .enqueue(
                "h".into(),
                "T".into(),
                "B".into(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        ctrl.cancel_job(&job_id).await;
        let row = p.job_row(&job_id).await.expect("row").expect("exists");
        assert_eq!(row.state, STATE_CANCELLED);
    }

    /// path 4 α #5（核心 γ 契约）：controller A enqueue 一堆 → 死掉；
    /// controller B 用同一 SQLite 重启 → restore_from_persistence → queue 恢复。
    /// 模拟 gateway 重启 in-flight 不丢的场景。
    #[tokio::test]
    async fn controller_restart_via_restore_repopulates_queue() {
        // 共享一个 sqlite 文件——两个 controller 像两次启动
        let tmp = tempfile::tempdir().expect("tempdir");
        let dbpath = tmp.path().join("dist.db");

        // === controller A：enqueue 3 个，pull 1 个，然后死掉（drop） ===
        let bus_a = EventBus::with_memory_store().await.expect("bus");
        let p_a = Arc::new(JobPersistence::connect_file(&dbpath).await.expect("p"));
        let ctrl_a =
            Arc::new(DistController::new("tok".into(), bus_a).with_persistence(p_a.clone()));
        ctrl_a.register("nodeA".into(), vec![], 1).await;
        let id_q1 = ctrl_a
            .enqueue(
                "h".into(),
                "T1".into(),
                "B1".into(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        let id_q2 = ctrl_a
            .enqueue(
                "h".into(),
                "T2".into(),
                "B2".into(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        let id_inf = ctrl_a
            .enqueue(
                "h".into(),
                "T3".into(),
                "B3".into(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        // pull 把 id_q1 翻 inflight（因为 pull 拿 queue head）。下面 restore 应该把它当 orphan。
        let pulled = ctrl_a.pull("nodeA").await.expect("job");
        assert_eq!(pulled.id, id_q1);
        drop(ctrl_a);
        drop(p_a);

        // === controller B：相同 sqlite 重启 ===
        let bus_b = EventBus::with_memory_store().await.expect("bus");
        let p_b = Arc::new(JobPersistence::connect_file(&dbpath).await.expect("p"));
        let ctrl_b =
            Arc::new(DistController::new("tok".into(), bus_b).with_persistence(p_b.clone()));
        let (queued_n, orphan_n) = ctrl_b.restore_from_persistence().await;
        assert_eq!(
            queued_n, 2,
            "id_q2 + id_inf 还是 queued（id_inf 在 controller A 没 pull）"
        );
        assert_eq!(orphan_n, 1, "id_q1 之前被 pull 了");

        // controller B 的 worker 来 pull——三个 job 全应能被取走，证明 restart 不丢。
        ctrl_b.register("nodeB".into(), vec![], 5).await;
        let mut got_ids: Vec<String> = Vec::new();
        while let Some(job) = ctrl_b.pull("nodeB").await {
            got_ids.push(job.id);
        }
        got_ids.sort();
        let mut expect_ids = vec![id_q1.clone(), id_q2.clone(), id_inf.clone()];
        expect_ids.sort();
        assert_eq!(got_ids, expect_ids, "重启后三个 job 全部能再派发");

        // SQLite 的 orphan 行应该被 restore 翻回 queued 然后 pull 又翻 inflight——
        // 重点是不会再被当 orphan
        let row = p_b.job_row(&id_q1).await.expect("row").expect("exists");
        assert_eq!(
            row.state, STATE_INFLIGHT,
            "orphan 经 restore + pull 后应是 inflight 状态，assignee=nodeB"
        );
        assert_eq!(row.assignee.as_deref(), Some("nodeB"));
    }

    /// path 4 α #6：no persistence 时 restore_from_persistence noop——保留向后兼容。
    /// 现有 in-memory-only 测试 / bench / repl 路径不能因为这条 wire 被破坏。
    #[tokio::test]
    async fn restore_is_noop_without_persistence() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let ctrl = Arc::new(DistController::new("tok".into(), bus));
        let (q, o) = ctrl.restore_from_persistence().await;
        assert_eq!(q, 0);
        assert_eq!(o, 0);
    }

    /// path 4 α #7：restore 后 orphan 排在 restore 之后 enqueue 的 queued 之前。
    /// 验 push_front 语义（与 sweep_stale 既有 "被回收的 job 优先派发" 注释对齐）。
    #[tokio::test]
    async fn restore_orders_orphans_before_queued() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dbpath = tmp.path().join("dist.db");

        // ctrl_a：enqueue 4 个 + pull 全部 → 4 个 inflight，drop 后 controller 死。
        let bus_a = EventBus::with_memory_store().await.expect("bus");
        let p_a = Arc::new(JobPersistence::connect_file(&dbpath).await.expect("p"));
        let ctrl_a =
            Arc::new(DistController::new("tok".into(), bus_a).with_persistence(p_a.clone()));
        ctrl_a.register("nodeA".into(), vec![], 4).await;
        let mut orphan_ids = Vec::new();
        for label in ["O1", "O2", "O3", "O4"] {
            orphan_ids.push(
                ctrl_a
                    .enqueue(
                        "h".into(),
                        label.into(),
                        "B".into(),
                        None,
                        vec![],
                        None,
                        String::new(),
                        vec![],
                        None,
                        None,
                    )
                    .await,
            );
        }
        for _ in 0..4 {
            ctrl_a.pull("nodeA").await.expect("pull");
        }
        drop(ctrl_a);
        drop(p_a);

        // ctrl_b：restore 先（4 个 orphan push_front），再 enqueue 2 个新 queued
        // → pull 顺序应是 [orphans..., new_queueds...]
        let bus_b = EventBus::with_memory_store().await.expect("bus");
        let p_b = Arc::new(JobPersistence::connect_file(&dbpath).await.expect("p"));
        let ctrl_b = Arc::new(DistController::new_with_persistence(
            "tok".into(),
            bus_b,
            p_b.clone(),
        ));
        let (qn, on) = ctrl_b.restore_from_persistence().await;
        assert_eq!(qn, 0);
        assert_eq!(on, 4);
        let new_q1 = ctrl_b
            .enqueue(
                "h".into(),
                "NewQ1".into(),
                "B".into(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        let new_q2 = ctrl_b
            .enqueue(
                "h".into(),
                "NewQ2".into(),
                "B".into(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        ctrl_b.register("nodeB".into(), vec![], 10).await;
        let mut order = Vec::new();
        while let Some(job) = ctrl_b.pull("nodeB").await {
            order.push(job.id);
        }
        assert_eq!(order.len(), 6);
        // 头 4 个必为 orphans（顺序 = restored.orphans 顺序，dispatched_at ASC）
        assert_eq!(&order[..4], &orphan_ids[..]);
        // 末 2 个是 restore 之后 enqueue 的 fresh queued
        assert_eq!(order[4], new_q1);
        assert_eq!(order[5], new_q2);
    }

    // ── v2 跨节点 sandbox · pre-spawn git fetch ─────────────────────────

    /// 起一个 source repo（home 端），返回路径 + 第二条 commit 的 hash。
    async fn make_seed_repo_with_two_commits() -> (tempfile::TempDir, std::path::PathBuf, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().to_path_buf();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            let _ = tokio::process::Command::new("git")
                .current_dir(&path)
                .args(&args)
                .output()
                .await;
        }
        tokio::fs::write(path.join("README.md"), "v1")
            .await
            .unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-qm", "v1"]] {
            let _ = tokio::process::Command::new("git")
                .current_dir(&path)
                .args(&args)
                .output()
                .await;
        }
        tokio::fs::write(path.join("README.md"), "v2")
            .await
            .unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-qm", "v2"]] {
            let _ = tokio::process::Command::new("git")
                .current_dir(&path)
                .args(&args)
                .output()
                .await;
        }
        let out = tokio::process::Command::new("git")
            .current_dir(&path)
            .args(["rev-parse", "HEAD"])
            .output()
            .await
            .unwrap();
        let head = String::from_utf8(out.stdout).unwrap().trim().to_string();
        (dir, path, head)
    }

    /// fetch 后 `origin/main` 应跟上 home 的 HEAD。
    #[tokio::test]
    async fn fetch_default_branch_advances_origin_ref() {
        let (_home_td, home_path, head_initial) = make_seed_repo_with_two_commits().await;

        // worker clone（file:// remote）
        let worker_td = tempfile::tempdir().unwrap();
        let worker_path = worker_td.path().join("worker");
        let _ = tokio::process::Command::new("git")
            .args([
                "clone",
                "-q",
                home_path.to_string_lossy().as_ref(),
                worker_path.to_string_lossy().as_ref(),
            ])
            .output()
            .await
            .unwrap();
        // worker 端配置 user 让后续 git op 不抱怨
        for args in [
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            let _ = tokio::process::Command::new("git")
                .current_dir(&worker_path)
                .args(&args)
                .output()
                .await;
        }

        // home 推第三条 commit
        tokio::fs::write(home_path.join("README.md"), "v3")
            .await
            .unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-qm", "v3"]] {
            let _ = tokio::process::Command::new("git")
                .current_dir(&home_path)
                .args(&args)
                .output()
                .await;
        }
        let out = tokio::process::Command::new("git")
            .current_dir(&home_path)
            .args(["rev-parse", "HEAD"])
            .output()
            .await
            .unwrap();
        let head_v3 = String::from_utf8(out.stdout).unwrap().trim().to_string();
        assert_ne!(head_v3, head_initial);

        // worker fetch
        let ok = try_fetch_default_branch(&worker_path, "main").await;
        assert!(ok, "fetch 应成功");

        // worker 上的 origin/main 应跟上
        let out = tokio::process::Command::new("git")
            .current_dir(&worker_path)
            .args(["rev-parse", "origin/main"])
            .output()
            .await
            .unwrap();
        let origin_main = String::from_utf8(out.stdout).unwrap().trim().to_string();
        assert_eq!(
            origin_main, head_v3,
            "fetch 后 worker 端 origin/main 应等于 home 端 HEAD"
        );
    }

    /// 没有 origin remote 时 fetch 应 best-effort 失败返 false，不 panic。
    #[tokio::test]
    async fn fetch_default_branch_returns_false_when_no_remote() {
        let (_td, path, _head) = make_seed_repo_with_two_commits().await;
        // 这是源 repo 自己（无 origin remote）
        let ok = try_fetch_default_branch(&path, "main").await;
        assert!(!ok, "无 remote 应返 false 而不是 panic");
    }

    /// FUXI_DISABLE_PRESPAWN_FETCH=1 时直接跳过——返 false 但不 spawn git 进程。
    /// 测试通过设置 env + 跑 fetch 即返，不验证副作用（无法分辨"未跑 git"和"跑了但失败"
    /// 仅靠返回值）；真正的开关效果在 stdout 的 tracing log。
    #[tokio::test]
    async fn fetch_default_branch_respects_disable_env() {
        // tempdir 起空目录——若 env 没生效会调 git 失败；env 生效则秒返 false
        let dir = tempfile::tempdir().unwrap();

        // env 设置仅在本测试 scope 内有效——其他并发测试拿空值不受影响。
        // SAFETY: tokio 单测多线程；变量只在此函数内读写，并行其他测不会读它。
        // 注意 std::env::set_var unsafe 在 Rust 2024 edition 已移除安全性 attribute。
        unsafe { std::env::set_var("FUXI_DISABLE_PRESPAWN_FETCH", "1") };
        let ok = try_fetch_default_branch(dir.path(), "main").await;
        unsafe { std::env::remove_var("FUXI_DISABLE_PRESPAWN_FETCH") };
        assert!(!ok);
    }

    // ── v2 跨节点 sandbox · post-job git push back ───────────────────────

    /// worker 端在 task 分支上 commit + push back 后，home（origin）应能看到
    /// `task/<uuid>` 分支与该 commit。
    #[tokio::test]
    async fn push_back_branch_advances_origin_ref() {
        // home 端用 bare repo 当 origin——non-bare 推 currently-checked-out 会被
        // receive.denyCurrentBranch=warn 默认拒；但本测推的是新 branch 不撞 main，
        // 仍用 bare 更稳：清晰是 "home 收 branch ref" 的语义。
        let home_td = tempfile::tempdir().unwrap();
        let home_path = home_td.path().join("home.git");
        let _ = tokio::process::Command::new("git")
            .args([
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                home_path.to_string_lossy().as_ref(),
            ])
            .output()
            .await
            .unwrap();

        // home 先有一条 main commit——bare 不能 commit，借 seed 仓库 push 进去
        let (_seed_td, seed_path, _) = make_seed_repo_with_two_commits().await;
        let _ = tokio::process::Command::new("git")
            .current_dir(&seed_path)
            .args([
                "remote",
                "add",
                "origin",
                home_path.to_string_lossy().as_ref(),
            ])
            .output()
            .await
            .unwrap();
        let _ = tokio::process::Command::new("git")
            .current_dir(&seed_path)
            .args(["push", "-q", "origin", "main"])
            .output()
            .await
            .unwrap();

        // worker clone home
        let worker_td = tempfile::tempdir().unwrap();
        let worker_path = worker_td.path().join("worker");
        let _ = tokio::process::Command::new("git")
            .args([
                "clone",
                "-q",
                home_path.to_string_lossy().as_ref(),
                worker_path.to_string_lossy().as_ref(),
            ])
            .output()
            .await
            .unwrap();
        for args in [
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            let _ = tokio::process::Command::new("git")
                .current_dir(&worker_path)
                .args(&args)
                .output()
                .await;
        }

        // worker 在新 branch 上写一条 commit（模拟 cc/codex 跑完留下 commit）
        let task_branch = format!("task/{}", uuid::Uuid::new_v4());
        let _ = tokio::process::Command::new("git")
            .current_dir(&worker_path)
            .args(["switch", "-q", "-c", &task_branch])
            .output()
            .await
            .unwrap();
        tokio::fs::write(worker_path.join("CHANGED.md"), "from worker")
            .await
            .unwrap();
        for args in [vec!["add", "-A"], vec!["commit", "-qm", "worker work"]] {
            let _ = tokio::process::Command::new("git")
                .current_dir(&worker_path)
                .args(&args)
                .output()
                .await;
        }
        let out = tokio::process::Command::new("git")
            .current_dir(&worker_path)
            .args(["rev-parse", "HEAD"])
            .output()
            .await
            .unwrap();
        let worker_head = String::from_utf8(out.stdout).unwrap().trim().to_string();

        // call try_push_back_branch
        let ok = try_push_back_branch(&worker_path).await;
        assert!(ok, "push back 应成功");

        // home 端应能看到这个 branch + commit
        let out = tokio::process::Command::new("git")
            .current_dir(&home_path)
            .args(["rev-parse", &task_branch])
            .output()
            .await
            .unwrap();
        assert!(out.status.success(), "home 应有 {task_branch}");
        let home_head = String::from_utf8(out.stdout).unwrap().trim().to_string();
        assert_eq!(
            home_head, worker_head,
            "push back 后 home 端 task branch HEAD 应等于 worker HEAD"
        );
    }

    /// 没有 origin remote 时 push back 应 best-effort 失败返 false，不 panic。
    #[tokio::test]
    async fn push_back_branch_returns_false_when_no_remote() {
        let (_td, path, _head) = make_seed_repo_with_two_commits().await;
        // seed 仓库自己——无 origin remote
        let ok = try_push_back_branch(&path).await;
        assert!(!ok, "无 remote 应返 false 而不是 panic");
    }

    /// detached HEAD 时不 push（避免推 anonymous ref）。
    #[tokio::test]
    async fn push_back_branch_skips_detached_head() {
        let (_td, path, head) = make_seed_repo_with_two_commits().await;
        // 加 origin 让 push 路径"理论上能通"；但 detached HEAD 应早返
        let remote_td = tempfile::tempdir().unwrap();
        let remote_path = remote_td.path().join("origin.git");
        let _ = tokio::process::Command::new("git")
            .args([
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                remote_path.to_string_lossy().as_ref(),
            ])
            .output()
            .await
            .unwrap();
        let _ = tokio::process::Command::new("git")
            .current_dir(&path)
            .args([
                "remote",
                "add",
                "origin",
                remote_path.to_string_lossy().as_ref(),
            ])
            .output()
            .await
            .unwrap();
        // detach
        let _ = tokio::process::Command::new("git")
            .current_dir(&path)
            .args(["checkout", "-q", "--detach", &head])
            .output()
            .await;

        let ok = try_push_back_branch(&path).await;
        assert!(!ok, "detached HEAD 应跳过 push");
    }

    /// FUXI_DISABLE_PUSHBACK=1 时直接跳过——返 false 不 spawn git。
    #[tokio::test]
    async fn push_back_branch_respects_disable_env() {
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("FUXI_DISABLE_PUSHBACK", "1") };
        let ok = try_push_back_branch(dir.path()).await;
        unsafe { std::env::remove_var("FUXI_DISABLE_PUSHBACK") };
        assert!(!ok);
    }

    /// path 4 α #8：`new_with_persistence` free-fn ctor smoke test——
    /// 与 builder 路径行为等价（都启用 dual-write）。
    #[tokio::test]
    async fn new_with_persistence_ctor_enables_dual_write() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let p = Arc::new(JobPersistence::connect_memory().await.expect("p"));
        let ctrl = Arc::new(DistController::new_with_persistence(
            "tok".into(),
            bus,
            p.clone(),
        ));
        let job_id = ctrl
            .enqueue(
                "h".into(),
                "T".into(),
                "B".into(),
                None,
                vec![],
                None,
                String::new(),
                vec![],
                None,
                None,
            )
            .await;
        let row = p.job_row(&job_id).await.expect("row").expect("exists");
        assert_eq!(row.state, "queued");
    }
}

//! 分布式 worker 最小闭环（80 分测试版）。
//!
//! 目标：让远端机器主动连接 controller 拉任务并回传结果，不依赖 controller 入站到家宽。

use anyhow::{Context, Result};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::{Args as ClapArgs, Subcommand};
use fuxi_agent_codex::CodexEvent;
use fuxi_agent_codex::parser::ItemPhase;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_events::EventBus;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistEnqueueReq {
    pub token: String,
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
    pub token: String,
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
    pub token: String,
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
    pub token: String,
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
    pub token: String,
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
    pub token: String,
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
    pub token: String,
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistCancelResp {
    /// 成功记下 cancel flag（job 不存在也返回 true，无状态幂等）。
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistProgressQuery {
    pub token: String,
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
    /// Worker 上次 register 声明的 tags。register 重连会覆盖。
    pub tags: Vec<String>,
    /// 同一 worker 允许的最大并发 job 数（≥1）。
    pub max_concurrency: u32,
    /// 当前 inflight 的 job_ids——Phase 3c 的 heartbeat / timeout 会用到。
    /// register 不清它（runtime state 不该被 worker 重连抹掉）。
    // Phase 3a 仅做占位；3c heartbeat 上来后会有真正 read site。
    #[allow(dead_code)]
    pub inflight: Vec<String>,
}

impl Default for NodeRuntimeInfo {
    fn default() -> Self {
        Self {
            last_seen: None,
            tags: Vec::new(),
            max_concurrency: 1,
            inflight: Vec::new(),
        }
    }
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
}

/// controller 进程内状态。
pub struct DistController {
    token: String,
    bus: EventBus,
    inner: Mutex<DistInner>,
}

impl DistController {
    pub fn new(token: String, bus: EventBus) -> Self {
        Self {
            token,
            bus,
            inner: Mutex::new(DistInner::default()),
        }
    }

    pub fn token(&self) -> &str {
        &self.token
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
        let mut g = self.inner.lock().await;
        let entry = g.nodes.entry(node_id).or_default();
        entry.last_seen = Some(Instant::now());
        entry.tags = tags;
        entry.max_concurrency = max_concurrency.max(1);
    }

    /// 快照查询：返回 `node_id` 当前的 runtime 信息（`None` 表示从未 register）。
    /// Phase 3b 的 tag-based 派工匹配会消费 `tags` 和 `max_concurrency`。
    // Phase 3a 只在测试里有 caller；3b 的派工算法会正式消费。
    #[allow(dead_code)]
    pub async fn node_info(&self, node_id: &str) -> Option<NodeRuntimeInfo> {
        self.inner.lock().await.nodes.get(node_id).cloned()
    }

    pub async fn enqueue(
        &self,
        node_id_hint: String,
        title: String,
        body: String,
        system_prompt: Option<String>,
        required_tags: Vec<String>,
        pinned_node: Option<String>,
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
        };
        let mut g = self.inner.lock().await;
        g.global_queue.push_back(job);
        drop(g);
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
        g.nodes
            .get_mut(node_id)
            .expect("entry was just touched")
            .inflight
            .push(job.id.clone());
        drop(g);
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
        let existed = g.inflight.remove(&req.job_id).is_some();
        g.finished.insert(req.job_id.clone(), req.clone());
        // Phase 3b: 从 worker 的 inflight list 释放——否则 capacity 永远 0
        if let Some(worker) = g.nodes.get_mut(&req.node_id) {
            worker.inflight.retain(|id| id != &req.job_id);
        }
        drop(g);
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
}

fn unauthorized() -> (StatusCode, String) {
    (StatusCode::UNAUTHORIZED, "invalid dist token".to_string())
}

async fn register_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistRegisterReq>,
) -> impl IntoResponse {
    if req.token != ctrl.token() {
        return unauthorized().into_response();
    }
    ctrl.register(req.node_id, req.tags, req.max_concurrency)
        .await;
    Json(DistRegisterResp { ok: true }).into_response()
}

async fn enqueue_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistEnqueueReq>,
) -> impl IntoResponse {
    if req.token != ctrl.token() {
        return unauthorized().into_response();
    }
    if req.title.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "title empty".to_string()).into_response();
    }
    let job_id = ctrl
        .enqueue(
            req.node_id,
            req.title,
            req.body,
            req.system_prompt,
            req.required_tags,
            req.pinned_node,
        )
        .await;
    Json(DistEnqueueResp { job_id }).into_response()
}

async fn pull_handler(
    State(ctrl): State<Arc<DistController>>,
    Query(q): Query<DistPullQuery>,
) -> impl IntoResponse {
    if q.token != ctrl.token() {
        return unauthorized().into_response();
    }
    let job = ctrl.pull(&q.node_id).await;
    Json(DistPullResp { job }).into_response()
}

async fn report_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistReportReq>,
) -> impl IntoResponse {
    if req.token != ctrl.token() {
        return unauthorized().into_response();
    }
    let accepted = ctrl.report(req).await;
    Json(DistReportResp { accepted }).into_response()
}

async fn job_status_handler(
    State(ctrl): State<Arc<DistController>>,
    Query(q): Query<DistJobStatusQuery>,
) -> impl IntoResponse {
    if q.token != ctrl.token() {
        return unauthorized().into_response();
    }
    Json(ctrl.job_status(&q.job_id).await).into_response()
}

async fn progress_post_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistProgressReq>,
) -> impl IntoResponse {
    if req.token != ctrl.token() {
        return unauthorized().into_response();
    }
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
    if q.token != ctrl.token() {
        return unauthorized().into_response();
    }
    Json(ctrl.pull_progress_after(&q.job_id, q.after).await).into_response()
}

async fn cancel_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistCancelReq>,
) -> impl IntoResponse {
    if req.token != ctrl.token() {
        return unauthorized().into_response();
    }
    ctrl.cancel_job(&req.job_id).await;
    Json(DistCancelResp { accepted: true }).into_response()
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
        .with_state(ctrl)
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

#[derive(Debug, ClapArgs)]
pub struct DistWorkerArgs {
    #[arg(long)]
    pub controller: String,
    #[arg(long)]
    pub node: String,
    #[arg(long)]
    pub token: Option<String>,
    #[arg(long, default_value = "codex")]
    pub codex_bin: String,
    #[arg(long, default_value_t = 1000)]
    pub poll_ms: u64,
    /// 声明本节点能力（可重复），用于 tag-based 派工。示例：
    /// `--tag home --tag codex --tag gpu`。不传 = 空集（只接无要求的 job）。
    #[arg(long = "tag", value_name = "TAG")]
    pub tags: Vec<String>,
    /// 本 worker 允许的最大并发 job 数。默认 1。
    #[arg(long, default_value_t = 1)]
    pub max_concurrency: u32,
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
    let token = resolve_token(args.token)?;
    let body = args.body.join(" ");
    let client = Client::new();
    let url = format!("{}/dist/enqueue", args.controller.trim_end_matches('/'));
    let resp = client
        .post(url)
        .json(&DistEnqueueReq {
            token,
            node_id: args.node,
            title: args.title,
            body,
            // CLI 入口裸派，不组装 role 心智——gateway agent 路径才会填。
            system_prompt: None,
            // CLI 同样不带 tags / pin——派工走全局 queue，谁空闲谁取。
            // 若真要定点派，用户用 `fuxi spawn --node` 走 gateway 路径。
            required_tags: Vec::new(),
            pinned_node: None,
        })
        .send()
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

pub async fn run_worker(args: DistWorkerArgs) -> Result<()> {
    let token = resolve_token(args.token)?;
    let controller = args.controller.trim_end_matches('/').to_string();
    let client = Client::new();
    client
        .post(format!("{controller}/dist/register"))
        .json(&DistRegisterReq {
            token: token.clone(),
            node_id: args.node.clone(),
            tags: args.tags.clone(),
            max_concurrency: args.max_concurrency,
        })
        .send()
        .await
        .context("dist register request failed")?
        .error_for_status()
        .context("dist register non-2xx")?;

    loop {
        let pull = client
            .get(format!("{controller}/dist/pull"))
            .query(&[("token", token.as_str()), ("node_id", args.node.as_str())])
            .send()
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
        let started = Instant::now();
        let ctx = WorkerCtx {
            client: &client,
            controller: &controller,
            token: &token,
            node_id: &args.node,
        };
        let run = run_codex_job(&ctx, &args.codex_bin, &job).await;
        let (ok, output) = match run {
            Ok(pair) => pair,
            Err(e) => (false, format!("worker run error: {e}")),
        };
        let _ = client
            .post(format!("{controller}/dist/report"))
            .json(&DistReportReq {
                token: token.clone(),
                node_id: args.node.clone(),
                job_id,
                ok,
                output,
                duration_ms: started.elapsed().as_millis(),
            })
            .send()
            .await;
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

/// worker 运行上下文——push progress 需要的 HTTP 目标。
struct WorkerCtx<'a> {
    client: &'a Client,
    controller: &'a str,
    token: &'a str,
    node_id: &'a str,
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
    let resp = ctx
        .client
        .post(format!("{}/dist/progress", ctx.controller))
        .json(&DistProgressReq {
            token: ctx.token.to_string(),
            node_id: ctx.node_id.to_string(),
            job_id: job_id.to_string(),
            chunks,
        })
        .send()
        .await;
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

    let mut child = Command::new(codex_bin)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn codex binary failed: {codex_bin}"))?;

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
                    && let Some(push) = codex_event_to_push(&ev)
                {
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
        }
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
        let raw = r#"{"token":"t","node_id":"n","title":"T","body":"B"}"#;
        let req: DistEnqueueReq = serde_json::from_str(raw).expect("decode");
        assert_eq!(req.token, "t");
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
            token: "t".into(),
            node_id: "n".into(),
            title: "T".into(),
            body: "B".into(),
            system_prompt: Some("role preamble".into()),
            required_tags: vec![],
            pinned_node: None,
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
            token: "tok".into(),
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
            token: "t".into(),
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
            token: "tok".into(),
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
            token: "t".into(),
            node_id: "hint".into(),
            title: "T".into(),
            body: "B".into(),
            system_prompt: None,
            required_tags: vec!["codex".into(), "gpu".into()],
            pinned_node: Some("home".into()),
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: DistEnqueueReq = serde_json::from_str(&s).unwrap();
        assert_eq!(back.required_tags, vec!["codex", "gpu"]);
        assert_eq!(back.pinned_node.as_deref(), Some("home"));
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
            token: "t".into(),
            node_id: "n".into(),
            tags: vec!["home".into(), "gpu".into()],
            max_concurrency: 4,
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: DistRegisterReq = serde_json::from_str(&s).unwrap();
        assert_eq!(back.tags, vec!["home", "gpu"]);
        assert_eq!(back.max_concurrency, 4);
    }
}

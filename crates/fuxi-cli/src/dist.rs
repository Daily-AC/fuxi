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
use std::collections::{HashMap, VecDeque};
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
    pub node_id: String,
    pub title: String,
    pub body: String,
    pub created_at: i64,
    /// 从 controller 端 resolve 的 role 系统提示；worker 会 prepend 到 codex
    /// prompt 头部来赋予 role 心智。老版 worker 不认识这个字段会直接忽略
    /// （`#[serde(default)]`），两端不强耦合升级节奏。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistEnqueueReq {
    pub token: String,
    pub node_id: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
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

#[derive(Debug, Clone, Default)]
pub struct NodeRuntimeInfo {
    pub last_seen: Option<Instant>,
}

#[derive(Default)]
struct DistInner {
    queues: HashMap<String, VecDeque<DistJob>>,
    inflight: HashMap<String, DistJob>,
    finished: HashMap<String, DistReportReq>,
    nodes: HashMap<String, NodeRuntimeInfo>,
    /// job_id → 按 seq 有序的 progress chunks。
    progress: HashMap<String, Vec<ProgressChunk>>,
    /// job_id → 下一个要分配的 seq（从 1 开始）。
    progress_next_seq: HashMap<String, u64>,
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

    pub async fn enqueue(
        &self,
        node_id: String,
        title: String,
        body: String,
        system_prompt: Option<String>,
    ) -> String {
        let id = format!("job-{}", Uuid::new_v4());
        let job = DistJob {
            id: id.clone(),
            node_id: node_id.clone(),
            title: title.clone(),
            body,
            created_at: chrono::Utc::now().timestamp(),
            system_prompt,
        };
        let mut g = self.inner.lock().await;
        g.queues.entry(node_id.clone()).or_default().push_back(job);
        drop(g);
        let _ = self.bus.publish(Event {
            meta: EventMeta::now(),
            kind: EventKind::Custom {
                label: "dist_job_enqueued".into(),
                payload: serde_json::json!({
                    "job_id": id,
                    "node_id": node_id,
                    "title": title
                }),
            },
        });
        id
    }

    pub async fn pull(&self, node_id: &str) -> Option<DistJob> {
        let mut g = self.inner.lock().await;
        g.nodes.entry(node_id.to_string()).or_default().last_seen = Some(Instant::now());
        let job = g
            .queues
            .entry(node_id.to_string())
            .or_default()
            .pop_front()?;
        g.inflight.insert(job.id.clone(), job.clone());
        drop(g);
        let _ = self.bus.publish(Event {
            meta: EventMeta::now(),
            kind: EventKind::Custom {
                label: "dist_job_dispatched".into(),
                payload: serde_json::json!({
                    "job_id": job.id,
                    "node_id": node_id,
                    "title": job.title
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
    ) -> (usize, u64) {
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
        (accepted, last_seq)
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
    {
        let mut g = ctrl.inner.lock().await;
        g.nodes.entry(req.node_id).or_default().last_seen = Some(Instant::now());
    }
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
        .enqueue(req.node_id, req.title, req.body, req.system_prompt)
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
    let (accepted, last_seq) = ctrl
        .push_progress(&req.node_id, &req.job_id, req.chunks)
        .await;
    Json(DistProgressAck { accepted, last_seq }).into_response()
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

pub fn router(ctrl: Arc<DistController>) -> Router {
    Router::new()
        .route("/dist/register", post(register_handler))
        .route("/dist/enqueue", post(enqueue_handler))
        .route("/dist/pull", get(pull_handler))
        .route("/dist/report", post(report_handler))
        .route("/dist/job", get(job_status_handler))
        .route("/dist/progress", post(progress_post_handler))
        .route("/dist/progress", get(progress_get_handler))
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

async fn flush_progress(ctx: &WorkerCtx<'_>, job_id: &str, chunks: Vec<ProgressPush>) {
    if chunks.is_empty() {
        return;
    }
    let _ = ctx
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
            flush_progress(ctx, &job.id, batch).await;
            last_flush = Instant::now();
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

    let ok = status.success() && !got_error;
    let output = if !final_text.trim().is_empty() {
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
        let (acc1, last1) = ctrl
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
        let (acc2, last2) = ctrl
            .push_progress("nodeA", "job-1", vec![push(ProgressKind::Thinking, "嗯")])
            .await;
        assert_eq!(acc2, 1);
        assert_eq!(last2, 3);
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
        let (_, last_b) = ctrl
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
}

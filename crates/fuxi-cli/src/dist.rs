//! 分布式 worker 最小闭环（80 分测试版）。
//!
//! 目标：让远端机器主动连接 controller 拉任务并回传结果，不依赖 controller 入站到家宽。

use anyhow::{Context, Result, anyhow};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::{Args as ClapArgs, Subcommand};
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_events::EventBus;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistEnqueueReq {
    pub token: String,
    pub node_id: String,
    pub title: String,
    pub body: String,
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

#[derive(Debug, Clone, Default)]
pub struct NodeRuntimeInfo {
    pub last_seen: Option<Instant>,
}

#[derive(Default)]
struct DistInner {
    queues: HashMap<String, VecDeque<DistJob>>,
    inflight: HashMap<String, DistJob>,
    nodes: HashMap<String, NodeRuntimeInfo>,
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

    pub async fn enqueue(&self, node_id: String, title: String, body: String) -> String {
        let id = format!("job-{}", Uuid::new_v4());
        let job = DistJob {
            id: id.clone(),
            node_id: node_id.clone(),
            title: title.clone(),
            body,
            created_at: chrono::Utc::now().timestamp(),
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
        let job = g.queues.entry(node_id.to_string()).or_default().pop_front()?;
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
        g.nodes
            .entry(req.node_id.clone())
            .or_default()
            .last_seen = Some(Instant::now());
        let existed = g.inflight.remove(&req.job_id).is_some();
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
    let job_id = ctrl.enqueue(req.node_id, req.title, req.body).await;
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

pub fn router(ctrl: Arc<DistController>) -> Router {
    Router::new()
        .route("/dist/register", post(register_handler))
        .route("/dist/enqueue", post(enqueue_handler))
        .route("/dist/pull", get(pull_handler))
        .route("/dist/report", post(report_handler))
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
        format!(
            "missing dist token: pass --token or set ${DIST_TOKEN_ENV} environment variable"
        )
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

        let started = Instant::now();
        let run = run_codex_job(&args.codex_bin, &job.title, &job.body).await;
        let (ok, output) = match run {
            Ok(text) => (true, text),
            Err(e) => (false, format!("worker run error: {e}")),
        };
        let _ = client
            .post(format!("{controller}/dist/report"))
            .json(&DistReportReq {
                token: token.clone(),
                node_id: args.node.clone(),
                job_id: job.id,
                ok,
                output,
                duration_ms: started.elapsed().as_millis(),
            })
            .send()
            .await;
    }
}

async fn run_codex_job(codex_bin: &str, title: &str, body: &str) -> Result<String> {
    let prompt = if body.trim().is_empty() {
        title.to_string()
    } else {
        format!("{title}\n\n{body}")
    };
    let out = Command::new(codex_bin)
        .args([
            "exec",
            "--json",
            "--dangerously-bypass-approvals-and-sandbox",
            &prompt,
        ])
        .output()
        .await
        .with_context(|| format!("spawn codex binary failed: {codex_bin}"))?;

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let mut summary = extract_text_summary(&stdout);
    if summary.is_empty() {
        summary = stdout.trim().to_string();
    }
    if summary.is_empty() {
        summary = stderr.trim().to_string();
    }
    if summary.is_empty() {
        return Err(anyhow!("codex produced empty stdout/stderr"));
    }
    if !out.status.success() {
        return Err(anyhow!("codex exited with {}: {}", out.status, summary));
    }
    Ok(summary)
}

fn extract_text_summary(stdout: &str) -> String {
    let mut texts = Vec::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        collect_text_fields(&v, &mut texts);
    }
    let joined = texts.join("\n");
    truncate_text(&joined, 1200)
}

fn collect_text_fields(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, vv) in map {
                if k == "text"
                    && let Some(s) = vv.as_str()
                    && !s.trim().is_empty()
                {
                    out.push(s.trim().to_string());
                }
                collect_text_fields(vv, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for vv in arr {
                collect_text_fields(vv, out);
            }
        }
        _ => {}
    }
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

    #[test]
    fn extract_text_summary_collects_nested_text_fields() {
        let raw = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello"}]}}
{"type":"tool","x":{"text":"world"}}"#;
        let s = extract_text_summary(raw);
        assert!(s.contains("hello"), "got: {s}");
        assert!(s.contains("world"), "got: {s}");
    }
}

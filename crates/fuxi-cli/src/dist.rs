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

pub fn router(ctrl: Arc<DistController>) -> Router {
    Router::new()
        .route("/dist/register", post(register_handler))
        .route("/dist/enqueue", post(enqueue_handler))
        .route("/dist/pull", get(pull_handler))
        .route("/dist/report", post(report_handler))
        .route("/dist/job", get(job_status_handler))
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
        let run = run_codex_job(&args.codex_bin, &job).await;
        let (ok, output) = match run {
            Ok(text) => (true, text),
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

async fn run_codex_job(codex_bin: &str, job: &DistJob) -> Result<String> {
    let prompt = build_codex_prompt_from_job(job);
    let cfg = fuxi_agent_codex::CodexLaunchConfig {
        binary: codex_bin.to_string(),
        ..Default::default()
    };
    let mut args = cfg.build_args();
    args.push(prompt);
    let out = Command::new(codex_bin)
        .args(&args)
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
}

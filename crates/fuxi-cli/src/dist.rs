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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cli: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
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

/// Worker 周期心跳——把**自己真实的 inflight 列表**报给 controller。
///
/// 为什么带 inflight 不只是 ping：worker 可能因为意外重启丢失 in-memory 状态，
/// controller 这边的 `NodeRuntimeInfo.inflight` 可能比实际多。让 worker 权威
/// 声明 "我现在真的在跑这些 job"，controller 以 worker 为准，自动修复漂移。
///
/// 频率约定：worker 每 10s 发一次；controller 30s 未收到视作 dead。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistHeartbeatReq {
    pub token: String,
    pub node_id: String,
    /// worker 自身视角的 inflight job_ids。空 = 当前空闲。
    #[serde(default)]
    pub inflight: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistHeartbeatResp {
    pub ok: bool,
    /// controller 汇报的"你应该 cancel 的 job_ids"——worker 对账后杀相应 child。
    /// 当前只填 `cancelled` 集合与 worker.inflight 的交集。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cancel_pending: Vec<String>,
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
    /// 当前 inflight 的 job_ids——pull 添加，report/heartbeat/sweep_stale
    /// 维护。worker 心跳的 inflight 是对账权威（自愈 controller-side 漂移）。
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
    pub async fn heartbeat(&self, node_id: &str, worker_inflight: Vec<String>) -> Vec<String> {
        let mut g = self.inner.lock().await;
        let cancelled = g.cancelled.clone();
        let node = g.nodes.entry(node_id.to_string()).or_default();
        node.last_seen = Some(Instant::now());
        node.inflight = worker_inflight.clone();
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
                recycled.push((nid, jobs));
            }
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
            req.cli,
            req.allowed_tools,
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

async fn heartbeat_handler(
    State(ctrl): State<Arc<DistController>>,
    Json(req): Json<DistHeartbeatReq>,
) -> impl IntoResponse {
    if req.token != ctrl.token() {
        return unauthorized().into_response();
    }
    let cancel_pending = ctrl.heartbeat(&req.node_id, req.inflight).await;
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
            // CLI 入口不指定 cli——worker 按默认（codex）跑；若用户就想
            // 在分布式命令行直派 cc，Phase 4b 之后可扩 `fuxi dist enqueue --cli cc`。
            cli: String::new(),
            allowed_tools: Vec::new(),
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
    let factory: AdapterFactory =
        Arc::new(|cli, args| select_adapter(cli, args).map(|a| a as Box<dyn CliAdapter>));
    run_worker_with(args, token, factory, HEARTBEAT_INTERVAL).await
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
    adapter_factory: AdapterFactory,
    heartbeat_interval: Duration,
) -> Result<()> {
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

    let inflight: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // 心跳 task：除上报 inflight 外，**消费 ack 的 cancel_pending**——
    // worker 静默执行（无 progress push）时段也能 ~heartbeat interval 内拿到
    // cancel 信号，弥补只靠 push_progress.should_cancel 的盲区。
    {
        let hb_inflight = inflight.clone();
        let hb_token = token.clone();
        let hb_node = args.node.clone();
        let hb_controller = controller.clone();
        let hb_client = client.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(heartbeat_interval);
            loop {
                tick.tick().await;
                let snapshot: Vec<String> = {
                    let g = hb_inflight.lock().await;
                    g.keys().cloned().collect()
                };
                let req = DistHeartbeatReq {
                    token: hb_token.clone(),
                    node_id: hb_node.clone(),
                    inflight: snapshot,
                };
                let resp = hb_client
                    .post(format!("{hb_controller}/dist/heartbeat"))
                    .json(&req)
                    .send()
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
        let node_c = args.node.clone();
        let inflight_c = inflight.clone();
        let factory_c = adapter_factory.clone();
        let args_for_factory = args.clone();
        let started = Instant::now();

        jobs.spawn(async move {
            let ctx = WorkerCtx {
                client: &client_c,
                controller: &controller_c,
                token: &token_c,
                node_id: &node_c,
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
            let _ = client_c
                .post(format!("{controller_c}/dist/report"))
                .json(&DistReportReq {
                    token: token_c,
                    node_id: node_c,
                    job_id: job.id.clone(),
                    ok,
                    output,
                    duration_ms: started.elapsed().as_millis(),
                })
                .send()
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

/// worker 运行上下文——push progress 需要的 HTTP 目标。
pub(crate) struct WorkerCtx<'a> {
    client: &'a Client,
    controller: &'a str,
    token: &'a str,
    node_id: &'a str,
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
        CcEvent::ResultError { reason } => Some(ProgressPush {
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

    let mut child = Command::new(bin)
        .args(&args)
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
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawn claude binary failed: {bin}"))?;

    let stdout = child.stdout.take().context("cc stdout pipe missing")?;
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
                    && let Ok(ev) = fuxi_agent_cc::parse_line(&line)
                    && let Some(push) = cc_event_to_push(&ev)
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
            cli: String::new(),
            allowed_tools: vec![],
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
            cli: String::new(),
            allowed_tools: vec![],
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
            String::new(),
            vec![],
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
            cli: String::new(),
            allowed_tools: vec![],
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
        };
        assert!(super::cc_event_to_push(&ev).is_none());
    }

    #[test]
    fn cc_event_result_error_maps_to_error_kind() {
        let ev = fuxi_agent_cc::CcEvent::ResultError {
            reason: "rate limited".into(),
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
            .heartbeat("n", vec!["job-x".into(), "job-y".into()])
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
        ctrl.heartbeat("n", vec!["B".into()]).await;
        let info = ctrl.node_info("n").await.unwrap();
        assert_eq!(info.inflight, vec!["B"]);
    }

    /// cancel 请求的 job 被 worker 心跳感知——返回给 worker 去杀 child。
    #[tokio::test]
    async fn heartbeat_reports_cancel_pending_in_intersection() {
        let ctrl = test_ctrl().await;
        ctrl.register("n".into(), vec![], 2).await;
        ctrl.cancel_job("A").await;
        ctrl.cancel_job("Z").await; // Z 不在 worker 的 inflight 里
        let pending = ctrl.heartbeat("n", vec!["A".into(), "B".into()]).await;
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
            token: "t".into(),
            node_id: "n".into(),
            inflight: vec!["job-1".into(), "job-2".into()],
        };
        let s = serde_json::to_string(&req).unwrap();
        let back: DistHeartbeatReq = serde_json::from_str(&s).unwrap();
        assert_eq!(back.inflight.len(), 2);
    }

    /// 老版 worker 没有 inflight 字段——要能兜底空。
    #[test]
    fn dist_heartbeat_req_deserializes_without_inflight() {
        let raw = r#"{"token":"t","node_id":"n"}"#;
        let req: DistHeartbeatReq = serde_json::from_str(raw).unwrap();
        assert!(req.inflight.is_empty());
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
        };
        let factory = make_factory(stub);

        // 派两条 job——必须先 enqueue 再 spawn worker，否则 worker 先 register 后
        // capacity_left 可用但还没 job，会进入 poll 死循环（OK，但慢）。
        let j1 = enq_simple(&ctrl, "job1").await;
        let j2 = enq_simple(&ctrl, "job2").await;

        let worker_handle = tokio::spawn(async move {
            // worker_with 是无限循环，测试侧靠 abort 终止。
            let _ = super::run_worker_with(args, "tok".into(), factory, Duration::from_millis(200))
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
        };

        let job_id = enq_simple(&ctrl, "silent").await;

        // 心跳 200ms——比生产 10s 短得多，让测试在秒级完成。
        let worker_handle = tokio::spawn(async move {
            let _ = super::run_worker_with(args, "tok".into(), factory, Duration::from_millis(200))
                .await;
        });

        // 等 active==1（worker pull 到并 spawn task）
        let pickup_deadline = Instant::now() + Duration::from_secs(2);
        while active.load(Ordering::SeqCst) == 0 {
            if Instant::now() > pickup_deadline {
                worker_handle.abort();
                srv.abort();
                panic!("worker 1s 内未 pickup silent job");
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
}

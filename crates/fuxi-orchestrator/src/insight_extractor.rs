//! 仓颉 InsightExtractorTask —— 论文 arXiv:2604.14004 Memory Transfer Learning
//! 的写入侧链路。
//!
//! ## 设计
//!
//! 订阅 EventBus 的 `TaskStateChanged { to: Done }` → 拉 task trajectory →
//! spawn 仓颉跑 extraction prompt → 拿到 JSON 数组 → 对**每条**候选 insight
//! 再 spawn 一次仓颉跑 judge prompt → 评分过阈值（默认 0.6）才 record 进
//! `hetu_patterns`，带 `source="cangjie-auto" / abstraction_score / derived_from_task`。
//!
//! ## 与 fuxi-memory::extractor 的差异
//!
//! - extractor 抽 **三元组事实**（subject/predicate/object）写 oracle_facts
//! - insight_extractor 抽 **可迁移原则**（role/task_type/pattern）写 hetu_patterns
//! - extractor 单趟 spawn；insight_extractor **每条候选** judge 一次（论文要求
//!   抽象度门槛——低层 trace 复述会 negative transfer，必须把不达标的剔掉）
//!
//! ## 公理对应
//!
//! - #1（不显式沟通 = 没做）：仓颉的产出经 record 进 hetu_patterns，玄女后续
//!   spawn 门客时由 γ 的 #7 注入桥读出注 prompt——形成完整经验闭环
//! - #2（玄女有知情权）：record 是写策府不抢 attention；玄女想看自己 query
//! - #3（真实时不轮询）：subscribe EventBus 被动接收 Done 推送
//!
//! ## 防递归
//!
//! - 任务所属 agent 的 role == "cangjie" → 跳过（仓颉自己不该被自己抽）
//! - 任务所属 agent 的 role == "extractor" → 跳过（fuxi-memory 的事实抽取器
//!   也是平台幕后工，没"经验"可抽）
//!
//! ## 配置 env
//!
//! - `FUXI_INSIGHT_EXTRACTOR_ENABLED`（**default true**——v2 跟 v1 反，论文支持开）
//!
//! ## Spawner trait 注入而非直依赖 Fuxi
//!
//! 跟 `fuxi_memory::extractor::FactExtractorSpawner` 同思路——把 spawn 抽成
//! trait，单测可以塞 mock 不必起真 cc 进程。生产实装放 fuxi-cli（顶层依赖
//! 全部 crate，避免 orchestrator 反向依赖 cli）。

use crate::error::Result;
use async_trait::async_trait;
use futures_util::StreamExt;
use fuxi_core::{AgentId, Event, EventKind, TaskId, TaskState};
use fuxi_events::EventBus;
use fuxi_memory::{HetuStore, NewPattern};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Spawner 通用错误——调用方包任意错误，本模块只关心"成功拿到最终文本"vs"超时/失败"。
pub type CangjieSpawnerError = Box<dyn std::error::Error + Send + Sync>;
pub type CangjieSpawnerResult<T> = std::result::Result<T, CangjieSpawnerError>;

/// 仓颉门客 spawner 契约。
///
/// 跟 `fuxi_memory::FactExtractorSpawner` 同型——抽出来方便单测。生产实装把
/// `Arc<Fuxi>` 包成它（fuxi-cli 端做 adapter）。
#[async_trait]
pub trait CangjieSpawner: Send + Sync {
    /// 派一个短寿命仓颉门客跑 prompt，等最终文本。超时/失败返 Err。
    async fn spawn_and_run(
        &self,
        prompt: String,
        timeout: Duration,
    ) -> CangjieSpawnerResult<String>;

    /// 按 agent_id 返回 role 字符串。找不到 None。用于防递归——cangjie 自己
    /// 跑完一个 task 不应再触发抽取（无限自环）。
    async fn role_of(&self, agent: AgentId) -> Option<String>;
}

/// 仓颉 role 标签——跟 `roles/cangjie/ROLE.md` 的 `name` 对齐。
pub const CANGJIE_ROLE: &str = "cangjie";
/// fuxi-memory 自家 extractor 的 role——这里也黑名单，避免给"事实抽取"的 task 抽 insight。
pub const EXTRACTOR_ROLE: &str = "extractor";
/// hetu_patterns.source 标签——`fuxi insight list` 区分手工 vs 自动用得到。
pub const CANGJIE_SOURCE: &str = "cangjie-auto";

/// InsightExtractorTask 行为参数。
#[derive(Debug, Clone)]
pub struct InsightExtractorConfig {
    /// 是否启用。env `FUXI_INSIGHT_EXTRACTOR_ENABLED` 解析后填进来。
    pub enabled: bool,
    /// extraction / judge 各自的 spawn 超时。默认 60s——judge 比 extraction 短很多但
    /// 共用一个 knob 简化。
    pub timeout: Duration,
    /// 每 task 最多落库多少条 insight（熔断）。默认 7（仓颉 prompt 写的"3-7 条
    /// 算正常"上限）。
    pub max_insights_per_task: usize,
    /// judge 阈值（< threshold 拒收）。默认 0.6。
    pub judge_threshold: f64,
    /// extraction prompt 模板——`<<role>>` / `<<task_type>>` / `<<task_title>>` /
    /// `<<trajectory>>` 会被替换。
    pub extract_template: String,
    /// judge prompt 模板——`<<role>>` / `<<task_type>>` / `<<pattern>>` 会被替换。
    pub judge_template: String,
}

impl Default for InsightExtractorConfig {
    fn default() -> Self {
        Self {
            enabled: true, // v2 default true：论文支持开
            timeout: Duration::from_secs(60),
            max_insights_per_task: 7,
            judge_threshold: 0.6,
            extract_template: DEFAULT_EXTRACT_TEMPLATE.to_string(),
            judge_template: DEFAULT_JUDGE_TEMPLATE.to_string(),
        }
    }
}

/// 默认 extraction prompt——细节请去 `roles/cangjie/instructions/extraction.md`。
/// 这里只放最薄一层装配：注入 role / task_type / task_title / trajectory，让仓颉
/// 自己读 ROLE.md + extraction.md 干活。
pub const DEFAULT_EXTRACT_TEMPLATE: &str = "[INSIGHT_EXTRACT] role=<<role>> task_type=<<task_type>> task_title=<<task_title>>\n\n按你 instructions/extraction.md 的契约从下面 trajectory 抽出**跨任务可迁移**的原则。\n论文公理：abstraction dictates transferability；低层 trace 是 negative transfer 不要写。\n\n输出**严格 JSON 数组**：[{\"role\":\"...\",\"task_type\":\"...\",\"pattern\":\"≤80字\"}]\n不抽就 `[]`。不要 markdown 围栏。不要任何解释。\n\n--- TRAJECTORY ---\n<<trajectory>>";

/// 默认 judge prompt——细节去 `roles/cangjie/instructions/judge.md`。
pub const DEFAULT_JUDGE_TEMPLATE: &str = "[INSIGHT_JUDGE] 候选 insight：role=<<role>> task_type=<<task_type>>\npattern=<<pattern>>\n\n按你 instructions/judge.md 的五档尺度（1.0/0.7/0.4/0.1/0.0）评分，阈值 0.6。\n输出**严格 JSON**：{\"score\": 0.X, \"reason\": \"≤40字判语\"}\n单行 JSON 不加任何文字、不要 markdown 围栏。";

/// 从 env 读 `FUXI_INSIGHT_EXTRACTOR_ENABLED`——v2 default **true**（v1 extractor 是 false）。
pub fn config_from_env() -> InsightExtractorConfig {
    let mut cfg = InsightExtractorConfig::default();
    match std::env::var("FUXI_INSIGHT_EXTRACTOR_ENABLED").as_deref() {
        // 显式 0/false/off/no 才关
        Ok("0") | Ok("false") | Ok("off") | Ok("no") | Ok("FALSE") => cfg.enabled = false,
        _ => {} // 其他全保持 true（含 unset）
    }
    cfg
}

/// 解析得到的 insight 候选——内部用，不对外暴露。
#[derive(Debug, Clone, serde::Deserialize)]
struct InsightCandidate {
    role: String,
    #[serde(default)]
    task_type: String,
    pattern: String,
}

/// judge 返回。
#[derive(Debug, serde::Deserialize)]
struct JudgeVerdict {
    score: f64,
    #[serde(default)]
    reason: String,
}

/// InsightExtractorTask —— 装配体；调用方 `new(...).spawn()` 拿 JoinHandle。
pub struct InsightExtractorTask {
    bus: EventBus,
    hetu: Arc<HetuStore>,
    spawner: Arc<dyn CangjieSpawner>,
    cfg: InsightExtractorConfig,
}

impl InsightExtractorTask {
    pub fn new(
        bus: EventBus,
        hetu: Arc<HetuStore>,
        spawner: Arc<dyn CangjieSpawner>,
        cfg: InsightExtractorConfig,
    ) -> Self {
        Self {
            bus,
            hetu,
            spawner,
            cfg,
        }
    }

    /// 起后台订阅 loop。drop JoinHandle 立即停。
    ///
    /// WHY 同步 subscribe 再 spawn：跟 fuxi_memory::Extractor 同款——若 subscribe
    /// 放后台 task，调用方返回后 publish 的事件会在订阅建立前广播走，订阅者
    /// 漏接。同步建立保证 `spawn()` 一返回就在听。
    pub fn spawn(self) -> JoinHandle<()> {
        let Self {
            bus,
            hetu,
            spawner,
            cfg,
        } = self;
        info!(
            enabled = cfg.enabled,
            max_insights = cfg.max_insights_per_task,
            timeout_secs = cfg.timeout.as_secs(),
            judge_threshold = cfg.judge_threshold,
            "insight_extractor: 开始订阅简册（仓颉路径）"
        );
        if !cfg.enabled {
            info!("insight_extractor: disabled，不起订阅循环");
            return tokio::spawn(async {});
        }
        let mut stream = bus.subscribe();
        tokio::spawn(async move {
            while let Some(item) = stream.next().await {
                let Ok(ev) = item else {
                    // Lagged 已被底层吸收，这里只是 store recv 错——继续。
                    continue;
                };
                let Some(task_id) = task_done_trigger(&ev) else {
                    continue;
                };
                let bus_c = bus.clone();
                let hetu_c = hetu.clone();
                let spawner_c = spawner.clone();
                let cfg_c = cfg.clone();
                let task_agent = ev.meta.agent;
                tokio::spawn(async move {
                    if let Err(e) =
                        process_task(&bus_c, &hetu_c, spawner_c, &cfg_c, task_id, task_agent).await
                    {
                        // 单 task 抽取失败吞掉——不让一次坏 trajectory 拖垮订阅链。
                        warn!(%task_id, error = %e, "insight_extractor: 本轮抽取失败（吞掉）");
                    }
                });
            }
            info!("insight_extractor: 订阅结束，退出");
        })
    }
}

/// `ev` 若是 "任务落至 Done"，返回 task_id。仅 Done 触发——Cancelled / Blocked 没"经验"可抽。
fn task_done_trigger(ev: &Event) -> Option<TaskId> {
    match &ev.kind {
        EventKind::TaskStateChanged {
            to: TaskState::Done,
            ..
        } => ev.meta.task,
        _ => None,
    }
}

/// 处理单个 Done task：防递归 → 拉 trajectory → extract → judge per insight → record。
async fn process_task(
    bus: &EventBus,
    hetu: &HetuStore,
    spawner: Arc<dyn CangjieSpawner>,
    cfg: &InsightExtractorConfig,
    task_id: TaskId,
    task_agent: Option<AgentId>,
) -> std::result::Result<(), String> {
    // 1) 防递归：cangjie / extractor 自己跑的 task 直接跳过。
    if let Some(agent) = task_agent
        && let Some(role) = spawner.role_of(agent).await
        && (role == CANGJIE_ROLE || role == EXTRACTOR_ROLE)
    {
        debug!(%task_id, %agent, %role, "insight_extractor: 内部 role 的 task，跳过");
        return Ok(());
    }

    // 2) 拉 trajectory。同 extractor.rs 的退避：broadcast 立即到达但 SQLite mpsc
    //    异步落库，极短时间窗里 history 可能读不到 Done。等到 history 里能看到 Done
    //    再继续，保证整段 trajectory 已落地。
    let hist = {
        let mut out = Vec::new();
        for _ in 0..50 {
            let h = bus
                .history_for_task(task_id)
                .await
                .map_err(|e| format!("history: {e}"))?;
            if h.iter().any(|ev| {
                matches!(
                    &ev.kind,
                    EventKind::TaskStateChanged {
                        to: TaskState::Done,
                        ..
                    }
                )
            }) {
                out = h;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if out.is_empty() {
            debug!(%task_id, "insight_extractor: 未等到 Done 落库，超时放弃");
            return Ok(());
        }
        out
    };

    // 3) 拼 trajectory text + 推 task_type / role / title。
    let trajectory_text = build_trajectory(&hist);
    if trajectory_text.trim().is_empty() {
        debug!(%task_id, "insight_extractor: trajectory 空，跳过");
        return Ok(());
    }
    // role：取 trajectory 里第一个干活门客的 role；查不到 fallback "unknown"。
    // 仓颉 prompt 还会要求自己再判断 role——这里给 hint 即可。
    let hint_role = match task_agent {
        Some(a) => spawner
            .role_of(a)
            .await
            .unwrap_or_else(|| "unknown".to_string()),
        None => "unknown".to_string(),
    };
    let task_title = task_title_from_history(&hist).unwrap_or_else(|| "unknown".to_string());
    let task_type_hint = ""; // 仓颉自己分类——这里给空字符串，不强加先验。

    // 4) 跑 extraction。
    let extract_prompt = cfg
        .extract_template
        .replace("<<role>>", &hint_role)
        .replace("<<task_type>>", task_type_hint)
        .replace("<<task_title>>", &task_title)
        .replace("<<trajectory>>", &trajectory_text);
    let extract_text = match spawner.spawn_and_run(extract_prompt, cfg.timeout).await {
        Ok(t) => t,
        Err(e) => return Err(format!("extraction spawn: {e}")),
    };
    let candidates = match parse_candidates(&extract_text) {
        Ok(c) => c,
        Err(e) => {
            warn!(
                %task_id,
                error = %e,
                preview = extract_text.chars().take(80).collect::<String>(),
                "insight_extractor: extraction JSON 解析失败，跳过"
            );
            return Ok(());
        }
    };
    if candidates.is_empty() {
        debug!(%task_id, "insight_extractor: 候选 0 条（仓颉返 []）");
        return Ok(());
    }

    // 5) 熔断到 max_insights_per_task。
    let capped: &[InsightCandidate] = if candidates.len() > cfg.max_insights_per_task {
        warn!(
            %task_id,
            got = candidates.len(),
            cap = cfg.max_insights_per_task,
            "insight_extractor: 候选超上限，截断"
        );
        &candidates[..cfg.max_insights_per_task]
    } else {
        &candidates[..]
    };

    // 6) 逐条 judge → 过阈值 record。
    let mut accepted = 0usize;
    let mut rejected = 0usize;
    for cand in capped {
        // 字段空 → 直接拒（judge 也会拒，但前置省一次 spawn 钱）。
        if cand.role.trim().is_empty() || cand.pattern.trim().is_empty() {
            warn!(
                %task_id,
                "insight_extractor: 候选字段空（role 或 pattern），跳过 judge"
            );
            rejected += 1;
            continue;
        }
        let judge_prompt = cfg
            .judge_template
            .replace("<<role>>", &cand.role)
            .replace("<<task_type>>", &cand.task_type)
            .replace("<<pattern>>", &cand.pattern);
        let judge_text = match spawner.spawn_and_run(judge_prompt, cfg.timeout).await {
            Ok(t) => t,
            Err(e) => {
                warn!(%task_id, error = %e, "insight_extractor: judge spawn 失败，跳过该条");
                rejected += 1;
                continue;
            }
        };
        let verdict = match parse_verdict(&judge_text) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    %task_id,
                    error = %e,
                    preview = judge_text.chars().take(80).collect::<String>(),
                    "insight_extractor: judge JSON 解析失败，跳过该条"
                );
                rejected += 1;
                continue;
            }
        };
        if verdict.score < cfg.judge_threshold {
            debug!(
                %task_id,
                score = verdict.score,
                threshold = cfg.judge_threshold,
                reason = %verdict.reason,
                pattern = %cand.pattern,
                "insight_extractor: judge 低于阈值，拒收"
            );
            rejected += 1;
            continue;
        }
        // record。NewPattern::insight 默认 source=cangjie-auto，task_type 空串
        // 表示 task-agnostic——这里若 cangjie 给了非空 task_type 就用上。
        let mut np = NewPattern::insight(&cand.role, &cand.pattern)
            .with_abstraction_score(verdict.score)
            .with_derived_from_task(task_id.to_string());
        if !cand.task_type.trim().is_empty() {
            np = np.with_task_type(cand.task_type.trim());
        }
        match hetu.record(np).await {
            Ok(_) => accepted += 1,
            Err(e) => {
                warn!(%task_id, error = %e, "insight_extractor: 单条 record 失败");
                rejected += 1;
            }
        }
    }
    info!(
        %task_id,
        accepted,
        rejected,
        "insight_extractor: 本轮抽取完成"
    );
    Ok(())
}

/// 把 task 历史里的 UserPrompted / AgentResponded / ToolCallStarted / ToolCallFinished
/// 拼成自然语言 trajectory——比 extractor.rs 多带工具调用，因为 insight 提取经常依赖
/// "门客做了什么 → 结果如何"的细节。
fn build_trajectory(events: &[Event]) -> String {
    let mut out = String::new();
    for ev in events {
        match &ev.kind {
            EventKind::UserPrompted { text } => {
                out.push_str("用户：");
                out.push_str(text);
                out.push('\n');
            }
            EventKind::AgentResponded { text } => {
                out.push_str("门客：");
                out.push_str(text);
                out.push('\n');
            }
            EventKind::ToolCallStarted { tool, .. } => {
                out.push_str(&format!("[工具调用 {tool}]\n"));
            }
            EventKind::ToolCallFinished {
                tool,
                ok,
                output_preview,
            } => {
                let status = if *ok { "ok" } else { "fail" };
                out.push_str(&format!("[工具结果 {tool} {status}] {output_preview}\n"));
            }
            _ => {}
        }
    }
    out
}

/// 从历史里抓第一个 UserPrompted 的文本前 60 字作 task_title hint。
fn task_title_from_history(events: &[Event]) -> Option<String> {
    events.iter().find_map(|ev| {
        if let EventKind::UserPrompted { text } = &ev.kind {
            Some(text.chars().take(60).collect::<String>())
        } else {
            None
        }
    })
}

fn parse_candidates(text: &str) -> std::result::Result<Vec<InsightCandidate>, serde_json::Error> {
    let trimmed = strip_code_fence(text.trim());
    serde_json::from_str::<Vec<InsightCandidate>>(trimmed)
}

fn parse_verdict(text: &str) -> std::result::Result<JudgeVerdict, serde_json::Error> {
    let trimmed = strip_code_fence(text.trim());
    serde_json::from_str::<JudgeVerdict>(trimmed)
}

/// 容忍 ```json ... ``` 围栏——尽管 prompt 明文禁止，仓颉偶尔仍会带，宽松解析。
fn strip_code_fence(s: &str) -> &str {
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s);
    s.trim()
}

/// `Result<()>` shim——内部 process_task 用 String 错误，对外用 OrchestratorError。
/// 当前 spawn loop 只 warn 不传播；保留以便将来想暴露失败点时切换。
#[allow(dead_code)]
fn lift(s: String) -> crate::error::OrchestratorError {
    crate::error::OrchestratorError::Other(s)
}

// 让 `Result<T>` 类型可用于潜在的 pub API 扩展（当前模块用 String 内错足够）。
#[allow(dead_code)]
type _Aux<T> = Result<T>;

#[cfg(test)]
mod tests {
    use super::*;
    use fuxi_core::event::EventMeta;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    // ─── mock spawner ─────────────────────────────────────

    /// MockSpawner：按调用顺序返回预设 response 队列；超出队列尾部 → "[]"。
    /// 也记录每次的 prompt 用于断言（比如确认 judge 真的被调用）。
    struct MockSpawner {
        responses: StdMutex<Vec<String>>,
        prompts: StdMutex<Vec<String>>,
        roles: StdMutex<HashMap<AgentId, String>>,
        /// 让 spawn 失败的次数（前 N 次返 Err）；用于"spawn 失败也吞"测试。
        fail_first_n: StdMutex<usize>,
    }

    impl MockSpawner {
        fn new(responses: Vec<String>) -> Arc<Self> {
            Arc::new(Self {
                responses: StdMutex::new(responses),
                prompts: StdMutex::new(Vec::new()),
                roles: StdMutex::new(HashMap::new()),
                fail_first_n: StdMutex::new(0),
            })
        }

        fn set_role(&self, agent: AgentId, role: &str) {
            self.roles.lock().unwrap().insert(agent, role.to_string());
        }

        fn prompts(&self) -> Vec<String> {
            self.prompts.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl CangjieSpawner for MockSpawner {
        async fn spawn_and_run(
            &self,
            prompt: String,
            _timeout: Duration,
        ) -> CangjieSpawnerResult<String> {
            self.prompts.lock().unwrap().push(prompt);
            let mut fail = self.fail_first_n.lock().unwrap();
            if *fail > 0 {
                *fail -= 1;
                return Err("mock spawn fail".into());
            }
            drop(fail);
            let mut q = self.responses.lock().unwrap();
            if q.is_empty() {
                Ok("[]".to_string())
            } else {
                Ok(q.remove(0))
            }
        }

        async fn role_of(&self, agent: AgentId) -> Option<String> {
            self.roles.lock().unwrap().get(&agent).cloned()
        }
    }

    // ─── helpers ──────────────────────────────────────────

    async fn make_setup() -> (EventBus, Arc<HetuStore>) {
        let bus = EventBus::with_memory_store().await.unwrap();
        let hetu = Arc::new(HetuStore::connect_memory().await.unwrap());
        (bus, hetu)
    }

    /// 推一段完整 trajectory（user → 门客 → tool → tool-result → done）到 bus，
    /// 等 history_for_task 能读到 Done。
    async fn publish_trajectory(bus: &EventBus, agent: AgentId, task: TaskId) {
        let mk_meta = || {
            let mut m = EventMeta::now();
            m.agent = Some(agent);
            m.task = Some(task);
            m
        };
        bus.publish(Event {
            meta: mk_meta(),
            kind: EventKind::UserPrompted {
                text: "修一个 Cargo.lock 撞冲突的 bug".into(),
            },
        })
        .unwrap();
        bus.publish(Event {
            meta: mk_meta(),
            kind: EventKind::AgentResponded {
                text: "查到 lockfile 多 worktree 改 Cargo.toml 时合并失败".into(),
            },
        })
        .unwrap();
        bus.publish(Event {
            meta: mk_meta(),
            kind: EventKind::ToolCallStarted {
                tool: "Bash".into(),
                args: serde_json::json!({"cmd": "rm Cargo.lock"}),
            },
        })
        .unwrap();
        bus.publish(Event {
            meta: mk_meta(),
            kind: EventKind::ToolCallFinished {
                tool: "Bash".into(),
                ok: true,
                output_preview: "ok".into(),
            },
        })
        .unwrap();
        bus.publish(Event {
            meta: mk_meta(),
            kind: EventKind::TaskStateChanged {
                from: TaskState::Delivering,
                to: TaskState::Done,
            },
        })
        .unwrap();
    }

    /// 等到 hetu 里至少有 N 条 record；超时 panic。
    async fn wait_records(hetu: &HetuStore, at_least: usize) {
        for _ in 0..200 {
            let n = hetu.list_active(100).await.unwrap().len();
            if n >= at_least {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("等 hetu record 至少 {at_least} 条超时");
    }

    /// 给后台 spawn 留点窗口，再断言 hetu 仍为 0（用于"应跳过"测试）。
    async fn assert_no_records(hetu: &HetuStore, wait_ms: u64) {
        tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        let n = hetu.list_active(100).await.unwrap().len();
        assert_eq!(n, 0, "应不写入 hetu，但写了 {n} 条");
    }

    // ─── tests ────────────────────────────────────────────

    /// extraction 返 3 条 + judge 全过 → 3 条入库，source/score/derived_from_task 齐。
    #[tokio::test]
    async fn extracts_three_insights_all_pass_judge() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        // 第 1 次 spawn = extraction（3 条）；之后每 candidate 一次 judge（共 3 次 0.85 通过）。
        let extraction = r#"[
            {"role":"luban","task_type":"bugfix","pattern":"Cargo.lock 撞冲突直接 rm 重生比手工解快"},
            {"role":"luban","task_type":"bugfix","pattern":"reproduce 不出来的 bug 先怀疑环境差异"},
            {"role":"luban","task_type":"bugfix","pattern":"测试加 println debug 完毕必须删干净再 commit"}
        ]"#;
        let pass = r#"{"score": 0.85, "reason": "通用守则"}"#;
        let spawner = MockSpawner::new(vec![
            extraction.to_string(),
            pass.to_string(),
            pass.to_string(),
            pass.to_string(),
        ]);
        spawner.set_role(agent, "luban");

        let cfg = InsightExtractorConfig::default();
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        // 让订阅就位再 publish。
        tokio::time::sleep(Duration::from_millis(20)).await;

        publish_trajectory(&bus, agent, task).await;
        wait_records(&hetu, 3).await;

        let active = hetu.list_active(10).await.unwrap();
        assert_eq!(active.len(), 3);
        for p in &active {
            assert_eq!(p.role, "luban");
            assert_eq!(p.source, CANGJIE_SOURCE);
            assert_eq!(p.abstraction_score, Some(0.85));
            assert_eq!(
                p.derived_from_task.as_deref(),
                Some(task.to_string().as_str())
            );
            assert_eq!(p.task_type, "bugfix");
        }
        // 4 次 spawn：1 extraction + 3 judge。
        assert_eq!(spawner.prompts().len(), 4);
    }

    /// judge 给 0.4（< 0.6 阈值）→ 全拒收，hetu 0 条。
    #[tokio::test]
    async fn judge_below_threshold_rejects_all() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        let extraction = r#"[
            {"role":"luban","task_type":"bugfix","pattern":"在 src/foo.rs 第 45 行加了 retry"}
        ]"#;
        let reject = r#"{"score": 0.1, "reason": "trajectory 复述含具体路径"}"#;
        let spawner = MockSpawner::new(vec![extraction.to_string(), reject.to_string()]);
        spawner.set_role(agent, "luban");

        let cfg = InsightExtractorConfig::default();
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        tokio::time::sleep(Duration::from_millis(20)).await;

        publish_trajectory(&bus, agent, task).await;
        // judge 拒收 → hetu 应保持 0
        assert_no_records(&hetu, 200).await;
    }

    /// extraction JSON 损坏 → 不 panic，0 条入库。
    #[tokio::test]
    async fn invalid_extraction_json_does_not_panic() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        let spawner = MockSpawner::new(vec!["not a json".to_string()]);
        spawner.set_role(agent, "luban");

        let cfg = InsightExtractorConfig::default();
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        tokio::time::sleep(Duration::from_millis(20)).await;

        publish_trajectory(&bus, agent, task).await;
        assert_no_records(&hetu, 200).await;
    }

    /// extraction 返 `[]`（仓颉判空）→ 0 条入库；judge 不应被调（节省成本）。
    #[tokio::test]
    async fn empty_extraction_array_skips_judge() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        let spawner = MockSpawner::new(vec!["[]".to_string()]);
        spawner.set_role(agent, "luban");

        let cfg = InsightExtractorConfig::default();
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        tokio::time::sleep(Duration::from_millis(20)).await;

        publish_trajectory(&bus, agent, task).await;
        assert_no_records(&hetu, 100).await;
        // 仅 1 次 spawn（extraction），judge 没跑。
        assert_eq!(spawner.prompts().len(), 1);
    }

    /// trajectory 无 user/agent/tool 内容 → trajectory 空 → 直接跳过，extraction 也不跑。
    #[tokio::test]
    async fn empty_trajectory_skips_extraction() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        let spawner = MockSpawner::new(vec![]);
        spawner.set_role(agent, "luban");

        let cfg = InsightExtractorConfig::default();
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        tokio::time::sleep(Duration::from_millis(20)).await;

        // 只发 Done，不发 user/agent，trajectory 是空。
        let mut meta = EventMeta::now();
        meta.agent = Some(agent);
        meta.task = Some(task);
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: TaskState::Delivering,
                to: TaskState::Done,
            },
        })
        .unwrap();

        assert_no_records(&hetu, 200).await;
        // extraction 也不应跑——history 进过滤后判空就退出。
        assert_eq!(spawner.prompts().len(), 0);
    }

    /// task agent role == cangjie → 跳过（防递归）；extraction 不跑。
    #[tokio::test]
    async fn cangjie_role_task_is_skipped() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        let spawner = MockSpawner::new(vec![]);
        spawner.set_role(agent, CANGJIE_ROLE);

        let cfg = InsightExtractorConfig::default();
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        tokio::time::sleep(Duration::from_millis(20)).await;

        publish_trajectory(&bus, agent, task).await;
        assert_no_records(&hetu, 100).await;
        assert_eq!(spawner.prompts().len(), 0, "防递归应直接跳过");
    }

    /// extractor role 同样跳过。
    #[tokio::test]
    async fn extractor_role_task_is_skipped() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        let spawner = MockSpawner::new(vec![]);
        spawner.set_role(agent, EXTRACTOR_ROLE);

        let cfg = InsightExtractorConfig::default();
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        tokio::time::sleep(Duration::from_millis(20)).await;

        publish_trajectory(&bus, agent, task).await;
        assert_no_records(&hetu, 100).await;
        assert_eq!(spawner.prompts().len(), 0);
    }

    /// disabled 时不订阅；publish Done 也无反应。
    #[tokio::test]
    async fn disabled_does_nothing() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        let spawner = MockSpawner::new(vec!["[]".to_string()]);
        spawner.set_role(agent, "luban");

        let cfg = InsightExtractorConfig {
            enabled: false,
            ..Default::default()
        };
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        tokio::time::sleep(Duration::from_millis(20)).await;

        publish_trajectory(&bus, agent, task).await;
        assert_no_records(&hetu, 100).await;
        assert_eq!(spawner.prompts().len(), 0);
    }

    /// extraction 返 8 条（超 max=7）→ 截断到 7，记 7 条。
    #[tokio::test]
    async fn caps_to_max_insights_per_task() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        let mut arr = String::from("[");
        for i in 0..8 {
            if i > 0 {
                arr.push(',');
            }
            arr.push_str(&format!(
                r#"{{"role":"luban","task_type":"bugfix","pattern":"原则 {i}"}}"#
            ));
        }
        arr.push(']');
        let pass = r#"{"score": 0.8, "reason": "ok"}"#;
        let mut responses = vec![arr];
        for _ in 0..7 {
            responses.push(pass.to_string());
        }
        let spawner = MockSpawner::new(responses);
        spawner.set_role(agent, "luban");

        let cfg = InsightExtractorConfig::default();
        assert_eq!(cfg.max_insights_per_task, 7);
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        tokio::time::sleep(Duration::from_millis(20)).await;

        publish_trajectory(&bus, agent, task).await;
        wait_records(&hetu, 7).await;
        // 给一点窗口防迟到的第 8 条。
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(hetu.list_active(20).await.unwrap().len(), 7);
        // 1 extraction + 7 judge = 8 次 spawn（截断后只 judge 7 条）。
        assert_eq!(spawner.prompts().len(), 8);
    }

    /// judge 返坏 JSON → 该条拒，其他正常的仍接受（不让一条烂判把整批废了）。
    #[tokio::test]
    async fn malformed_judge_rejects_only_that_one() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        let extraction = r#"[
            {"role":"luban","task_type":"bugfix","pattern":"a"},
            {"role":"luban","task_type":"bugfix","pattern":"b"}
        ]"#;
        let pass = r#"{"score": 0.8, "reason": "ok"}"#;
        let spawner = MockSpawner::new(vec![
            extraction.to_string(),
            "garbage".to_string(), // 第一条 judge 坏
            pass.to_string(),      // 第二条 judge 过
        ]);
        spawner.set_role(agent, "luban");

        let cfg = InsightExtractorConfig::default();
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        tokio::time::sleep(Duration::from_millis(20)).await;

        publish_trajectory(&bus, agent, task).await;
        wait_records(&hetu, 1).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let active = hetu.list_active(10).await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].pattern, "b", "只第二条该入库");
    }

    /// 候选字段空（role 或 pattern）→ 前置拒，judge 都不调。
    #[tokio::test]
    async fn empty_field_candidates_rejected_pre_judge() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        let extraction = r#"[
            {"role":"","task_type":"bugfix","pattern":"a"},
            {"role":"luban","task_type":"bugfix","pattern":""}
        ]"#;
        let spawner = MockSpawner::new(vec![extraction.to_string()]);
        spawner.set_role(agent, "luban");

        let cfg = InsightExtractorConfig::default();
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        tokio::time::sleep(Duration::from_millis(20)).await;

        publish_trajectory(&bus, agent, task).await;
        assert_no_records(&hetu, 200).await;
        // 仅 1 次 extraction spawn，judge 没跑。
        assert_eq!(spawner.prompts().len(), 1);
    }

    /// extraction prompt 含 markdown 围栏（仓颉偶尔会带）→ 仍能正确解析。
    #[tokio::test]
    async fn extraction_with_code_fence_is_parsed() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        let wrapped =
            "```json\n[{\"role\":\"luban\",\"task_type\":\"bugfix\",\"pattern\":\"x\"}]\n```";
        let pass = r#"{"score": 0.8, "reason": "ok"}"#;
        let spawner = MockSpawner::new(vec![wrapped.to_string(), pass.to_string()]);
        spawner.set_role(agent, "luban");

        let cfg = InsightExtractorConfig::default();
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        tokio::time::sleep(Duration::from_millis(20)).await;

        publish_trajectory(&bus, agent, task).await;
        wait_records(&hetu, 1).await;
    }

    /// task_type 空字符串 → record 时保持 "" (task-agnostic)，不强加 hint。
    #[tokio::test]
    async fn empty_task_type_record_remains_task_agnostic() {
        let (bus, hetu) = make_setup().await;
        let agent = AgentId::new();
        let task = TaskId::new();

        let extraction = r#"[{"role":"luban","task_type":"","pattern":"通用守则"}]"#;
        let pass = r#"{"score": 0.9, "reason": "ok"}"#;
        let spawner = MockSpawner::new(vec![extraction.to_string(), pass.to_string()]);
        spawner.set_role(agent, "luban");

        let cfg = InsightExtractorConfig::default();
        let _h = InsightExtractorTask::new(bus.clone(), hetu.clone(), spawner.clone(), cfg).spawn();
        tokio::time::sleep(Duration::from_millis(20)).await;

        publish_trajectory(&bus, agent, task).await;
        wait_records(&hetu, 1).await;
        let active = hetu.list_active(10).await.unwrap();
        assert_eq!(active[0].task_type, "");
    }

    /// config_from_env 默认开（v2 翻转）；FUXI_INSIGHT_EXTRACTOR_ENABLED=0 才关。
    #[test]
    fn config_from_env_defaults_enabled() {
        // 显式 unset 保证不被外部 env 干扰。
        // SAFETY: 单线程 #[test] 内无并发——这是测试隔离不是生产线程安全问题。
        unsafe { std::env::remove_var("FUXI_INSIGHT_EXTRACTOR_ENABLED") };
        let cfg = config_from_env();
        assert!(cfg.enabled, "v2 default true（论文支持开）");

        unsafe { std::env::set_var("FUXI_INSIGHT_EXTRACTOR_ENABLED", "0") };
        let cfg = config_from_env();
        assert!(!cfg.enabled);

        unsafe { std::env::set_var("FUXI_INSIGHT_EXTRACTOR_ENABLED", "false") };
        let cfg = config_from_env();
        assert!(!cfg.enabled);

        unsafe { std::env::set_var("FUXI_INSIGHT_EXTRACTOR_ENABLED", "1") };
        let cfg = config_from_env();
        assert!(cfg.enabled);

        unsafe { std::env::remove_var("FUXI_INSIGHT_EXTRACTOR_ENABLED") };
    }

    /// strip_code_fence 单测——确保中间内容/无围栏/纯 JSON 三种都不破。
    #[test]
    fn strip_code_fence_handles_variants() {
        assert_eq!(strip_code_fence("```json\n[]\n```"), "[]");
        assert_eq!(strip_code_fence("```\n[]\n```"), "[]");
        assert_eq!(strip_code_fence("[]"), "[]");
        assert_eq!(
            strip_code_fence("```json\n{\"score\": 0.5}\n```"),
            "{\"score\": 0.5}"
        );
    }
}

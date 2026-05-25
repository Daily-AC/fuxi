//! 简册 → 甲骨（M2.5 实装）——任务结束后自动抽取 S/P/O 三元组入库。
//!
//! 设计（对应 `docs/architecture-v1.1-roadmap.md §M2.5`）：
//!
//! - 订阅 `EventBus` 的 `TaskStateChanged { to: Done }`。
//! - 从 `EventBus::history_for_task(task_id)` 拉该任务的事件，筛
//!   `UserPrompted` + `AgentResponded` 拼成 transcript。
//! - 通过注入的 [`FactExtractorSpawner`] 派一个短寿命 extractor 门客跑
//!   prompt，等它的最终文本返回。
//! - 解析 JSON list 为若干 [`NewFact`]，逐条 `oracle.insert`。
//! - 防递归：任务所属 agent 的 role == "extractor" 时直接跳过。
//! - 熔断：每 task 最多落库 `max_facts_per_task` 条，超出丢弃并 `warn!` 一次。
//!
//! 为什么把 spawner 作为 trait 注入：fuxi-memory 不能依赖 fuxi-orchestrator
//! （那边会反过来依赖这里）。调用方在主线 `Fuxi` 里实现这个 trait（spawn 一个
//! role="extractor" 的短寿命 cc 门客）并把它传进来。

use crate::oracle::{NewFact, OracleStore};
use async_trait::async_trait;
use futures_util::StreamExt;
use fuxi_core::{AgentId, Event, EventKind, TaskId, TaskState};
use fuxi_events::EventBus;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Spawner 返回的通用错误——调用方可以包任意错误类型，Extractor 只关心
/// "成功拿到最终文本" vs "超时/失败"。
pub type SpawnerError = Box<dyn std::error::Error + Send + Sync>;
pub type SpawnerResult<T> = std::result::Result<T, SpawnerError>;

/// 抽取门客 spawner 契约。
///
/// - `spawn_and_run`：按 prompt 起一个短寿命门客，阻塞等它的最终文本。
///   超时 / 门客异常都应返 `Err`——Extractor 会把失败吞掉（warn! 即可，
///   不能影响其他订阅者）。
/// - `role_of`：给 `AgentId` 返回它的 role 字符串，用来做防递归过滤。
///   不存在（AgentId 已注销）返回 `None`——Extractor 按"未知"处理（会抽）。
///
/// 为什么两个能力合在一个 trait：调用端实现时共享同一份 shelf 引用，没必要
/// 拆两个注入点。未来要拆，拆起来也便宜。
#[async_trait]
pub trait FactExtractorSpawner: Send + Sync {
    /// 派一个短寿命 extractor 门客跑 prompt，等它的最终文本返回。
    /// 超时/失败都应返 Err。
    async fn spawn_and_run(&self, prompt: String, timeout: Duration) -> SpawnerResult<String>;

    /// 按 agent_id 返回 role 字符串。找不到返回 `None`。
    async fn role_of(&self, agent: AgentId) -> Option<String>;
}

/// Extractor 行为参数。
#[derive(Debug, Clone)]
pub struct ExtractorConfig {
    /// 是否启用。false 时 `spawn` 起的订阅循环啥都不做，只日志。
    /// env `FUXI_EXTRACTOR_ENABLED=0` 由上层解析，这里只看 bool。
    pub enabled: bool,
    /// 跑 extractor 门客的超时。默认 60s。
    pub timeout: Duration,
    /// 每 task 最多落库多少条事实。默认 10。
    pub max_facts_per_task: usize,
    /// 抽取 prompt 模板——`<<transcript>>` 会被替换成对话文本。
    /// 默认是 [`DEFAULT_PROMPT_TEMPLATE`]。
    pub prompt_template: String,
}

impl Default for ExtractorConfig {
    fn default() -> Self {
        // 2026-04-21 翻转默认：自动每 task 抽取太烧 cc API + 噪音 fact 多。
        // 改让玄女按 prompt 判断时机直接用 `fuxi memory record` 手工入。
        // 想恢复自动行为：`FUXI_EXTRACTOR_ENABLED=1`。
        Self {
            enabled: false,
            timeout: Duration::from_secs(60),
            max_facts_per_task: 10,
            prompt_template: DEFAULT_PROMPT_TEMPLATE.to_string(),
        }
    }
}

/// 抽取器内置 prompt——尽量短、明确要求只输出 JSON。
pub const DEFAULT_PROMPT_TEMPLATE: &str = "你是事实抽取器。从以下对话里抽出**跨会话持久事实**——用户身份、偏好、约定规矩、项目技术栈。不抽：情绪、玩笑、当下的临时状态。\n\n输出 JSON array，每条 {\"subject\": \"...\", \"predicate\": \"...\", \"object\": \"...\"}。不抽就输出 `[]`。**只输出 JSON 不加任何解释**。\n\n对话：\n<<transcript>>";

/// Extractor——持有订阅、spawner、oracle 的装配。
///
/// 调用方：`Extractor::new(...).spawn()` 拿到 `JoinHandle`；drop handle 自然
/// 结束（订阅流会随 bus close 而断）。
pub struct Extractor {
    bus: EventBus,
    oracle: Arc<OracleStore>,
    spawner: Arc<dyn FactExtractorSpawner>,
    cfg: ExtractorConfig,
}

impl Extractor {
    pub fn new(
        bus: EventBus,
        oracle: Arc<OracleStore>,
        spawner: Arc<dyn FactExtractorSpawner>,
        cfg: ExtractorConfig,
    ) -> Self {
        Self {
            bus,
            oracle,
            spawner,
            cfg,
        }
    }

    /// 起后台订阅循环。返回 `JoinHandle` —— drop 它会立即停掉整个循环。
    ///
    /// WHY 同步 subscribe 再 spawn：若 subscribe 放到后台 task 里，调用方返回后
    /// 立刻 publish 的事件会在订阅建立前就广播走，订阅者漏接。把 subscribe 放在
    /// 调用线程同步完成，保证 `spawn()` 一返回就已经在听。
    pub fn spawn(self) -> JoinHandle<()> {
        let Self {
            bus,
            oracle,
            spawner,
            cfg,
        } = self;
        info!(
            enabled = cfg.enabled,
            max_facts = cfg.max_facts_per_task,
            timeout_secs = cfg.timeout.as_secs(),
            "extractor: 开始订阅简册"
        );
        if !cfg.enabled {
            info!("extractor: disabled，不起订阅循环");
            return tokio::spawn(async {});
        }
        let mut stream = bus.subscribe();
        tokio::spawn(async move {
            while let Some(item) = stream.next().await {
                let Ok(ev) = item else { continue };
                let Some(task_id) = task_done_trigger(&ev) else {
                    continue;
                };
                // 每个 Done 触发独立的后台处理——extractor 门客慢不应影响 bus。
                let bus_c = bus.clone();
                let oracle_c = oracle.clone();
                let spawner_c = spawner.clone();
                let cfg_c = cfg.clone();
                let task_agent = ev.meta.agent;
                tokio::spawn(async move {
                    if let Err(e) =
                        process_task(&bus_c, &oracle_c, spawner_c, &cfg_c, task_id, task_agent)
                            .await
                    {
                        warn!(%task_id, error = %e, "extractor: 本轮抽取失败（吞掉）");
                    }
                });
            }
            info!("extractor: 订阅结束，退出");
        })
    }
}

async fn process_task(
    bus: &EventBus,
    oracle: &OracleStore,
    spawner: Arc<dyn FactExtractorSpawner>,
    cfg: &ExtractorConfig,
    task_id: TaskId,
    task_agent: Option<AgentId>,
) -> Result<(), String> {
    // 1) 防递归：task 所属 agent role == extractor 直接跳过。
    //    若 task_agent 为 None（极罕见），按未知处理，放行——上层主线不会 spawn
    //    不带 agent 的 extractor task，所以这里多做也没代价。
    if let Some(agent) = task_agent
        && let Some(role) = spawner.role_of(agent).await
        && role == EXTRACTOR_ROLE
    {
        debug!(%task_id, %agent, "extractor: 自身 role 的 task，跳过");
        return Ok(());
    }

    // 2) 拉 transcript。
    // WHY 重试：Done 事件通过 broadcast 立即到达订阅者，但 SQLite 落库走 mpsc
    // → writer task 的异步路径——极短时间窗口里 `history_for_task` 可能读不到
    // 前序 UserPrompted/AgentResponded。writer 串行处理，我们等到 `history` 里
    // 能看到 Done 事件本身再继续，保证整段 transcript 都已落地。退避上限 500ms
    // 是保守值；正常不会碰到。
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
            debug!(%task_id, "extractor: 未等到 Done 落库，超时放弃");
            return Ok(());
        }
        out
    };
    let transcript = build_transcript(&hist);
    if transcript.trim().is_empty() {
        debug!(%task_id, "extractor: transcript 空，跳过");
        return Ok(());
    }

    // 3) 跑 extractor 门客。
    let prompt = cfg.prompt_template.replace("<<transcript>>", &transcript);
    let text = spawner
        .spawn_and_run(prompt, cfg.timeout)
        .await
        .map_err(|e| format!("spawner: {e}"))?;

    // 4) 解析 JSON。坏 JSON 只 warn + 0 条。
    let Ok(triples) = parse_facts(&text) else {
        warn!(%task_id, preview = text.chars().take(80).collect::<String>(), "extractor: 返回的 JSON 解析失败，跳过");
        return Ok(());
    };

    // 5) 熔断 + 入库。
    let capped = if triples.len() > cfg.max_facts_per_task {
        warn!(
            %task_id,
            got = triples.len(),
            cap = cfg.max_facts_per_task,
            "extractor: 条数超过上限，截断"
        );
        &triples[..cfg.max_facts_per_task]
    } else {
        &triples[..]
    };
    let mut inserted = 0usize;
    for t in capped {
        let fact = NewFact::new(&t.subject, &t.predicate, &t.object)
            .with_source(EXTRACTOR_SOURCE)
            .with_confidence(0.7);
        match oracle.insert(fact).await {
            Ok(_) => inserted += 1,
            Err(e) => warn!(%task_id, error = %e, "extractor: 单条 fact 入库失败"),
        }
    }
    info!(%task_id, inserted, "extractor: 本轮抽取完成");
    Ok(())
}

/// 从 task 历史事件里筛出 UserPrompted / AgentResponded 拼成 transcript 文本。
///
/// 排序已由 `history_for_task` 保证（按 rowid 升序）；不再排。
fn build_transcript(events: &[Event]) -> String {
    let mut out = String::new();
    for ev in events {
        match &ev.kind {
            EventKind::UserPrompted { text } => {
                out.push_str("用户：");
                out.push_str(text);
                out.push('\n');
            }
            EventKind::AgentResponded { text, .. } => {
                out.push_str("门客：");
                out.push_str(text);
                out.push('\n');
            }
            _ => {}
        }
    }
    out
}

/// 一条 S/P/O。内部用；不对外暴露（调用方不需要）。
#[derive(Debug, serde::Deserialize)]
struct Triple {
    subject: String,
    predicate: String,
    object: String,
}

fn parse_facts(text: &str) -> std::result::Result<Vec<Triple>, serde_json::Error> {
    // 容忍门客在 JSON 两端包 ```json fence——去掉再 parse。
    let trimmed = strip_code_fence(text.trim());
    serde_json::from_str::<Vec<Triple>>(trimmed)
}

/// 简单去 markdown 代码围栏：`\`\`\`json ... \`\`\``。
/// 只处理首尾两端；中间的忽略（极少见）。
fn strip_code_fence(s: &str) -> &str {
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s);
    let s = s.strip_suffix("```").unwrap_or(s);
    s.trim()
}

/// `ev` 若是 "任务落至 Done" 的状态变更事件，返回该 task_id。
fn task_done_trigger(ev: &Event) -> Option<TaskId> {
    match &ev.kind {
        EventKind::TaskStateChanged {
            to: TaskState::Done,
            ..
        } => ev.meta.task,
        _ => None,
    }
}

/// Extractor 门客的 role 标签——跟 `skills/extractor/SKILL.md` 里的 `name` 对齐。
pub const EXTRACTOR_ROLE: &str = "extractor";
/// 抽取来源标签——入库时打进 `source` 列，便于审计/GC 区分手工 vs 自动。
pub const EXTRACTOR_SOURCE: &str = "extractor";

#[cfg(test)]
mod tests {
    use super::*;
    use fuxi_core::{EventMeta, TaskId};

    fn done_event(task: TaskId) -> Event {
        let mut meta = EventMeta::now();
        meta.task = Some(task);
        Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: TaskState::Delivering,
                to: TaskState::Done,
            },
        }
    }

    #[test]
    fn task_done_trigger_fires_only_on_done() {
        let task = TaskId::new();
        let ev = done_event(task);
        assert_eq!(task_done_trigger(&ev), Some(task));

        let mut meta = EventMeta::now();
        meta.task = Some(task);
        let other = Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: TaskState::InProgress,
                to: TaskState::Delivering,
            },
        };
        assert!(task_done_trigger(&other).is_none());
    }

    #[test]
    fn strip_code_fence_removes_markdown_json_block() {
        assert_eq!(strip_code_fence("```json\n[]\n```"), "[]");
        assert_eq!(strip_code_fence("```\n[]\n```"), "[]");
        assert_eq!(strip_code_fence("[]"), "[]");
    }
}

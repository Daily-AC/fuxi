//! 系统事件 → 玄女唤醒桥。
//!
//! 订阅 EventBus，把"玄女该知情"的系统事件转成自然语言 prompt，通过
//! [`Intervener::intervene`] 追加到玄女对话。
//!
//! 公理对应：
//! - #1 显式沟通 —— Trigger 到期/门客死亡这类系统事件默认没人告诉玄女，
//!   桥补上这条通路。
//! - #2 玄女永远有知情权 —— 门客下线必须让玄女知道，由她判断是否续派。
//! - #3 真实时不轮询 —— 桥通过 bus.subscribe() 被动接收推送。
//!
//! ## 为什么接 `OrchestratorCcReceived`（2026-04-20 修 Bug 7）
//!
//! 历史设计注释说"TUI 已订阅就够了"——**对人类坐在屏幕前成立，对 headless
//! 玄女不成立**。公理 #1「不显式沟通 = 没做」意味着：TUI 只是给用户看的
//! presentation，玄女 cc 实例的 stdin 没收到就是没收到。必须经过 intervene
//! 把抄送文本注入她下一轮对话。不会自环——`from_user_to` 这个事件字段已经
//! 说明 from 是 user、不是 xuannv，玄女对门客的 intervene 不触发此事件。
//!
//! ## 为什么不用 `Arc<Fuxi>` 做测试依赖
//!
//! `Fuxi` 实体需要 workspace + bus 等等复杂依赖。桥逻辑是纯事件 → prompt
//! 映射，抽 [`Intervener`] trait 让单测可以用 Mock 覆盖所有分支。

use crate::Result;
use crate::fuxi::Fuxi;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use fuxi_core::event::{Event, EventKind};
use fuxi_core::id::AgentId;
use fuxi_core::trigger_lookup::TriggerLookup;
use fuxi_events::EventBus;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// 内部 role 白名单：这些门客的生命周期事件**不抄送**给玄女。
///
/// 为什么：extractor 是 M2.5 自动后台跑的"幕后工"——每个 task Done 都
/// spawn 一个 extractor → extractor 自己 Done 又被桥抄送给玄女 → 玄女被
/// 自递归噪音淹没（2026-04-20 用户实测发现，玄女自己在 transcript 里诊
/// 断出"bridge 过滤漏洞"）。这类 role 玄女不需要逐个知道完成。
const INTERNAL_ROLES: &[&str] = &["extractor"];

fn is_internal_role(role: &str) -> bool {
    INTERNAL_ROLES.contains(&role)
}

const BRIDGE_INTERRUPT_LAG_MS_DEFAULT: u64 = 3000;

fn parse_bool_token(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| parse_bool_token(&v))
        .unwrap_or(false)
}

fn bridge_interrupt_worker_reports() -> bool {
    env_flag("FUXI_BRIDGE_INTERRUPT_WORKER_REPORTS")
}

fn bridge_interrupt_lag_ms() -> u64 {
    std::env::var("FUXI_BRIDGE_INTERRUPT_LAG_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(BRIDGE_INTERRUPT_LAG_MS_DEFAULT)
}

fn bridge_delivery_lag_ms(at: DateTime<Utc>) -> u64 {
    let ms = Utc::now().signed_duration_since(at).num_milliseconds();
    ms.max(0) as u64
}

/// 桥需要的 orchestrator 能力——仅 `intervene` + `role_of`，方便测试用 Mock。
///
/// 生产实装由 [`Fuxi`] 提供；单测里用一个小 struct 覆盖即可。
#[async_trait]
pub trait Intervener: Send + Sync {
    async fn intervene(&self, agent_id: AgentId, interrupt_first: bool, text: &str) -> Result<()>;

    /// 查门客当前登记的 role 标签——主要拿来给玄女读（"codex 门客下线"比裸 id 可读）。
    /// 未找到返回 None；调用方自行 fallback。
    async fn role_of(&self, agent_id: AgentId) -> Option<String>;
}

#[async_trait]
impl Intervener for Fuxi {
    async fn intervene(&self, agent_id: AgentId, interrupt_first: bool, text: &str) -> Result<()> {
        Fuxi::intervene(self, agent_id, interrupt_first, text).await
    }

    async fn role_of(&self, agent_id: AgentId) -> Option<String> {
        self.list_workers()
            .await
            .into_iter()
            .find(|c| c.id == agent_id)
            .map(|c| c.profile.role.clone())
    }
}

/// 三段式 TriggerFired 唤醒 prompt——契约见 `docs/architecture-v1.md` §M1.3。
///
/// 和 `fuxi-scheduler::prompt::build_trigger_prompt` 保持字节级一致；fuxi-orchestrator
/// 不能依赖 fuxi-scheduler（会引入循环），所以这里各自持有一份。
fn build_trigger_prompt(id: &str, fired_at: DateTime<Utc>, cause: &str, intent: &str) -> String {
    format!(
        "[TRIGGER_FIRED id={id} fired_at={ts} cause={cause}]\n\n{intent}\n\n[INSTRUCTION: 判断当前环境是否适合执行此触发。适合则调度门客，不适合则回报原因]",
        ts = fired_at.to_rfc3339(),
    )
}

fn build_death_prompt(agent_id: AgentId, role: &str, cause: &str) -> String {
    format!("门客 {agent_id}（role={role}）已下线，原因：{cause}。请判断是否续派或告知用户。")
}

fn build_task_done_prompt(agent_id: AgentId, role: &str, done: bool) -> String {
    let verb = if done { "完成" } else { "被取消" };
    format!(
        "门客 {agent_id}（role={role}）任务已{verb}。请直接向用户汇报结果或派新活，不要额外轮询状态。"
    )
}

fn build_cc_prompt(to_worker: AgentId, role: &str, text: &str) -> String {
    format!(
        "[CC] 用户直接对门客 {to_worker}（role={role}）说：「{text}」。\n\n你未被点名，仅为抄送留痕（公理 #2：你永远有知情权）。无需主动回话，除非判断需介入。"
    )
}

/// 系统事件 → 玄女唤醒桥。
pub struct SystemEventBridge;

impl SystemEventBridge {
    /// 生产路径——接入 `Arc<Fuxi>`。
    ///
    /// `trigger_lookup` 由 CLI 启动处注入（通常是 `Arc<TriggerStore>`），
    /// 避免 fuxi-orchestrator 直接依赖 fuxi-scheduler。
    pub fn spawn(
        fuxi: Arc<Fuxi>,
        bus: EventBus,
        xuannv_id: AgentId,
        trigger_lookup: Arc<dyn TriggerLookup>,
    ) -> JoinHandle<()> {
        let intervener: Arc<dyn Intervener> = fuxi;
        Self::spawn_with(intervener, bus, xuannv_id, trigger_lookup)
    }

    /// 测试/内部路径——任意 [`Intervener`] 注入。
    pub fn spawn_with(
        intervener: Arc<dyn Intervener>,
        bus: EventBus,
        xuannv_id: AgentId,
        trigger_lookup: Arc<dyn TriggerLookup>,
    ) -> JoinHandle<()> {
        let mut sub = bus.subscribe();
        tokio::spawn(async move {
            while let Some(item) = sub.next().await {
                let Ok(ev) = item else {
                    // 即使底层出错也尽量继续；具体 Lagged 已被 subscribe 过滤。
                    continue;
                };
                handle_event(&*intervener, &*trigger_lookup, xuannv_id, ev).await;
            }
            debug!("SystemEventBridge: 订阅流结束，退出");
        })
    }
}

/// 事件分派——拆出单函数方便直接单测（不用 spawn 真任务）。
async fn handle_event(
    intervener: &dyn Intervener,
    trigger_lookup: &dyn TriggerLookup,
    xuannv_id: AgentId,
    ev: Event,
) {
    match ev.kind {
        EventKind::TriggerFired {
            id,
            fired_at,
            cause,
        } => {
            let Some(intent) = trigger_lookup.intent(&id).await else {
                warn!(trigger = id, "TriggerFired 对应的 trigger 已不在库中，跳过");
                return;
            };
            let prompt = build_trigger_prompt(&id, fired_at, &cause, &intent);
            if let Err(e) = intervener.intervene(xuannv_id, false, &prompt).await {
                warn!(error = %e, "bridge: intervene(TriggerFired) 失败——玄女可能已下线");
            }
        }
        EventKind::AgentDead { cause } => {
            // 过滤：只处理非玄女门客的死亡——玄女自己死了没法再 intervene，且会造成回响。
            let Some(agent_id) = ev.meta.agent else {
                debug!("AgentDead 缺 meta.agent（平台级死亡），跳过");
                return;
            };
            if agent_id == xuannv_id {
                debug!("AgentDead 目标是玄女自身，跳过——否则就对死人说话");
                return;
            }
            let role = intervener
                .role_of(agent_id)
                .await
                .unwrap_or_else(|| "unknown".to_string());
            // 内部 role（如 extractor）的死亡事件不抄给玄女——是后台自动管理的，
            // 噪音 > 价值。日志里仍记，便于排错。
            if is_internal_role(&role) {
                debug!(%agent_id, %role, "AgentDead 内部 role，跳过抄送");
                return;
            }
            let lag_ms = bridge_delivery_lag_ms(ev.meta.at);
            let interrupt_first =
                bridge_interrupt_worker_reports() && lag_ms >= bridge_interrupt_lag_ms();
            info!(
                %agent_id,
                %role,
                lag_ms,
                interrupt_first,
                "bridge: 转发门客下线回报到玄女"
            );
            let prompt = build_death_prompt(agent_id, &role, &cause);
            if let Err(e) = intervener
                .intervene(xuannv_id, interrupt_first, &prompt)
                .await
            {
                warn!(error = %e, "bridge: intervene(AgentDead) 失败");
            }
        }
        EventKind::TaskStateChanged {
            to: fuxi_core::task::TaskState::Done,
            ..
        }
        | EventKind::TaskStateChanged {
            to: fuxi_core::task::TaskState::Cancelled,
            ..
        } => {
            // 2026-04-20 用户实测发现：鲁班完活不通知玄女（公理 #2 漏洞）。
            // 桥在这里补上——但只处理非玄女门客的任务完成事件。
            let Some(agent_id) = ev.meta.agent else {
                return;
            };
            if agent_id == xuannv_id {
                return; // 玄女自己的 task done 不触发（会回响）
            }
            let done = matches!(
                ev.kind,
                EventKind::TaskStateChanged {
                    to: fuxi_core::task::TaskState::Done,
                    ..
                }
            );
            let role = intervener
                .role_of(agent_id)
                .await
                .unwrap_or_else(|| "unknown".to_string());
            // 内部 role（如 extractor）的 Task Done 不抄玄女——自动后台工作，
            // 每轮抄送会递归淹没她（2026-04-20 用户实测 + 玄女自诊）。
            if is_internal_role(&role) {
                debug!(%agent_id, %role, ?ev.meta.task, "TaskStateChanged 内部 role，跳过抄送");
                return;
            }
            let lag_ms = bridge_delivery_lag_ms(ev.meta.at);
            let interrupt_first =
                bridge_interrupt_worker_reports() && lag_ms >= bridge_interrupt_lag_ms();
            info!(
                %agent_id,
                %role,
                task = ?ev.meta.task,
                done,
                lag_ms,
                interrupt_first,
                "bridge: 转发门客任务终态回报到玄女"
            );
            let prompt = build_task_done_prompt(agent_id, &role, done);
            if let Err(e) = intervener
                .intervene(xuannv_id, interrupt_first, &prompt)
                .await
            {
                warn!(error = %e, "bridge: intervene(TaskDone) 失败");
            }
        }
        EventKind::OrchestratorCcReceived {
            from_user_to, text, ..
        } => {
            // 过滤条件：抄送的目标（meta.agent）必须就是玄女；否则这不是发给她的抄送。
            let Some(target) = ev.meta.agent else {
                return;
            };
            if target != xuannv_id {
                debug!("OrchestratorCcReceived meta.agent != xuannv，跳过");
                return;
            }
            // 安全网：若某种代码路径把 xuannv 自己塞进 from_user_to（不该发生）就跳过，
            // 避免"玄女抄送给玄女"的回响。
            if from_user_to == xuannv_id {
                return;
            }
            let role = intervener
                .role_of(from_user_to)
                .await
                .unwrap_or_else(|| "unknown".to_string());
            let prompt = build_cc_prompt(from_user_to, &role, &text);
            if let Err(e) = intervener.intervene(xuannv_id, false, &prompt).await {
                warn!(error = %e, "bridge: intervene(OrchestratorCcReceived) 失败");
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use fuxi_core::event::EventMeta;
    use fuxi_events::EventBus;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::Mutex;

    #[test]
    fn parse_bool_token_recognizes_common_truthy_values() {
        assert!(parse_bool_token("1"));
        assert!(parse_bool_token("true"));
        assert!(parse_bool_token("YES"));
        assert!(parse_bool_token(" on "));
        assert!(!parse_bool_token("0"));
        assert!(!parse_bool_token("false"));
        assert!(!parse_bool_token("no"));
        assert!(!parse_bool_token(""));
    }

    #[test]
    fn bridge_delivery_lag_ms_never_negative() {
        let future = Utc::now() + chrono::Duration::seconds(1);
        assert_eq!(bridge_delivery_lag_ms(future), 0);
    }

    // ─── mock intervener ─────────────────────────────────────

    #[derive(Default)]
    struct MockIntervener {
        calls: Mutex<Vec<(AgentId, bool, String)>>,
        roles: Mutex<HashMap<AgentId, String>>,
    }

    impl MockIntervener {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }

        async fn set_role(&self, id: AgentId, role: &str) {
            self.roles.lock().await.insert(id, role.to_string());
        }

        async fn snapshot(&self) -> Vec<(AgentId, bool, String)> {
            self.calls.lock().await.clone()
        }
    }

    #[async_trait]
    impl Intervener for MockIntervener {
        async fn intervene(
            &self,
            agent_id: AgentId,
            interrupt_first: bool,
            text: &str,
        ) -> Result<()> {
            self.calls
                .lock()
                .await
                .push((agent_id, interrupt_first, text.to_string()));
            Ok(())
        }
        async fn role_of(&self, agent_id: AgentId) -> Option<String> {
            self.roles.lock().await.get(&agent_id).cloned()
        }
    }

    // ─── mock trigger lookup ─────────────────────────────────

    struct MockLookup(HashMap<String, String>);

    #[async_trait]
    impl TriggerLookup for MockLookup {
        async fn intent(&self, id: &str) -> Option<String> {
            self.0.get(id).cloned()
        }
    }

    fn empty_lookup() -> Arc<dyn TriggerLookup> {
        Arc::new(MockLookup(HashMap::new()))
    }

    // ─── 小工具：等桥消化完一批事件 ───────────────────────────

    async fn wait_call(mock: &Arc<MockIntervener>, at_least: usize) {
        for _ in 0..100 {
            if mock.snapshot().await.len() >= at_least {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("等 intervene 被调至少 {at_least} 次超时");
    }

    // ─── tests ────────────────────────────────────────────────

    /// 门客 AgentDead → 桥 intervene 玄女一次，prompt 含 role + cause。
    #[tokio::test]
    async fn agent_dead_of_worker_wakes_xuannv() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "codex-coder").await;

        let _handle =
            SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        // 等桥完成 subscribe（broadcast 漏发给未订阅者）——发一条 ping 再推真事件
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "cc stream closed".into(),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1, "应且仅 intervene 一次");
        let (target, interrupt_first, text) = &calls[0];
        assert_eq!(*target, xuannv);
        assert!(!*interrupt_first, "追加式，不要打断玄女");
        assert!(text.contains("下线"), "prompt 含下线: {text}");
        assert!(text.contains("codex-coder"), "prompt 含 role: {text}");
        assert!(text.contains("cc stream closed"), "prompt 含 cause: {text}");
    }

    /// 玄女自己 AgentDead → 桥不 intervene（否则就是对死人说话 + 回响）。
    #[tokio::test]
    async fn agent_dead_of_xuannv_is_ignored() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();

        let mock = MockIntervener::new();
        let _handle =
            SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(xuannv);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "fuxi shutdown".into(),
            },
        })
        .expect("publish");

        // 给桥 50ms 处理；应无 intervene。
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            mock.snapshot().await.is_empty(),
            "玄女自己死不应触发 intervene"
        );
    }

    /// TriggerFired → 桥查 TriggerLookup 拿 intent → 拼三段式 prompt → intervene。
    #[tokio::test]
    async fn trigger_fired_wakes_xuannv_with_three_stage_prompt() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();

        let mock = MockIntervener::new();
        let mut map = HashMap::new();
        map.insert("trg_abc".to_string(), "每周五 9 点 review PR".to_string());
        let lookup: Arc<dyn TriggerLookup> = Arc::new(MockLookup(map));

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, lookup);
        tokio::time::sleep(Duration::from_millis(20)).await;

        let at = Utc::now();
        bus.publish(Event {
            meta: EventMeta::now(),
            kind: EventKind::TriggerFired {
                id: "trg_abc".into(),
                fired_at: at,
                cause: "scheduled".into(),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1);
        let (target, interrupt_first, text) = &calls[0];
        assert_eq!(*target, xuannv);
        assert!(!*interrupt_first);
        assert!(
            text.starts_with("[TRIGGER_FIRED id=trg_abc "),
            "head: {text}"
        );
        assert!(text.contains("cause=scheduled"), "cause: {text}");
        assert!(text.contains("每周五 9 点 review PR"), "intent: {text}");
        assert!(text.contains("[INSTRUCTION:"), "instruction: {text}");
    }

    /// TriggerFired 但 TriggerLookup 找不到 intent → 跳过，不 intervene。
    #[tokio::test]
    async fn trigger_fired_without_intent_is_skipped() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let mock = MockIntervener::new();

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        bus.publish(Event {
            meta: EventMeta::now(),
            kind: EventKind::TriggerFired {
                id: "trg_missing".into(),
                fired_at: Utc::now(),
                cause: "manual".into(),
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(mock.snapshot().await.is_empty(), "找不到 intent 应静默跳过");
    }

    /// OrchestratorCcReceived 目标=玄女 → 桥必 intervene 玄女一次（公理 #1 显式沟通）。
    #[tokio::test]
    async fn orchestrator_cc_received_wakes_xuannv() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let worker = AgentId::new();
        let mock = MockIntervener::new();
        mock.set_role(worker, "luban").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(xuannv);
        bus.publish(Event {
            meta,
            kind: EventKind::OrchestratorCcReceived {
                from_user_to: worker,
                text: "你好门客".into(),
                original_intervention_id: uuid::Uuid::new_v4(),
            },
        })
        .expect("publish");

        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert_eq!(calls.len(), 1, "抄送应 intervene 玄女一次");
        let (target, interrupt_first, text) = &calls[0];
        assert_eq!(*target, xuannv);
        assert!(!*interrupt_first, "抄送是追加式，不打断当前 turn");
        assert!(text.contains("[CC]"), "prompt 应含 [CC] 标识: {text}");
        assert!(text.contains("luban"), "prompt 应含 role: {text}");
        assert!(text.contains("你好门客"), "prompt 应含原文: {text}");
    }

    /// 抄送目标 meta.agent 不是玄女 → 跳过（避免误触）。
    #[tokio::test]
    async fn orchestrator_cc_received_for_other_agent_is_ignored() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let other = AgentId::new();
        let worker = AgentId::new();
        let mock = MockIntervener::new();

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(other); // 抄送对象不是玄女
        bus.publish(Event {
            meta,
            kind: EventKind::OrchestratorCcReceived {
                from_user_to: worker,
                text: "别人的抄送".into(),
                original_intervention_id: uuid::Uuid::new_v4(),
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            mock.snapshot().await.is_empty(),
            "非玄女的抄送不应触发 intervene"
        );
    }

    /// broadcast Lagged 不打断桥——丢几条后续事件仍要被处理。
    #[tokio::test]
    async fn bridge_survives_broadcast_lag() {
        // 极小 buffer 迫使未消费 receiver 触发 Lagged。
        let store = fuxi_events::EventStore::connect_memory()
            .await
            .expect("store");
        let cfg = fuxi_events::EventBusConfig {
            buffer: 2,
            writer_queue: 4096,
            lag_threshold: 10000,
        };
        let bus = EventBus::new(store, cfg);
        let xuannv = AgentId::new();
        let worker = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(worker, "painter").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        // 先狂推 200 条桥不关心的事件（UserPrompted）——buffer=2 保证桥被 Lag。
        for i in 0..200 {
            bus.publish(Event {
                meta: EventMeta::now(),
                kind: EventKind::UserPrompted {
                    text: format!("noise-{i}"),
                },
            })
            .expect("publish noise");
        }

        // 再给一条真 AgentDead——Lag 后桥仍应响应。
        let mut meta = EventMeta::now();
        meta.agent = Some(worker);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "late but real".into(),
            },
        })
        .expect("publish dead");

        // 必须能在短时间内拿到这条 intervene。
        wait_call(&mock, 1).await;
        let calls = mock.snapshot().await;
        assert!(
            calls.iter().any(|(_, _, t)| t.contains("late but real")),
            "Lag 后桥仍应处理 AgentDead: {calls:?}"
        );
    }

    /// 内部 role（extractor）的 Task Done **不**触发 intervene。
    /// 修 2026-04-21 雪崩：M2.5 extractor 自递归 → 玄女 transcript 被噪音淹没。
    #[tokio::test]
    async fn extractor_task_done_is_not_copied_to_xuannv() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let extractor = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(extractor, "extractor").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(extractor);
        meta.task = Some(fuxi_core::id::TaskId::new());
        bus.publish(Event {
            meta,
            kind: EventKind::TaskStateChanged {
                from: fuxi_core::task::TaskState::Delivering,
                to: fuxi_core::task::TaskState::Done,
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            mock.snapshot().await.is_empty(),
            "extractor Done 不应抄送给玄女（自递归保护）"
        );
    }

    /// 内部 role（extractor）AgentDead 也**不**触发——后台自管理生命周期。
    #[tokio::test]
    async fn extractor_agent_dead_is_not_copied_to_xuannv() {
        let bus = EventBus::with_memory_store().await.expect("bus");
        let xuannv = AgentId::new();
        let extractor = AgentId::new();

        let mock = MockIntervener::new();
        mock.set_role(extractor, "extractor").await;

        let _h = SystemEventBridge::spawn_with(mock.clone(), bus.clone(), xuannv, empty_lookup());
        tokio::time::sleep(Duration::from_millis(20)).await;

        let mut meta = EventMeta::now();
        meta.agent = Some(extractor);
        bus.publish(Event {
            meta,
            kind: EventKind::AgentDead {
                cause: "fuxi shutdown".into(),
            },
        })
        .expect("publish");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            mock.snapshot().await.is_empty(),
            "extractor 死不应抄送给玄女"
        );
    }
}

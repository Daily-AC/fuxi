//! 玄女上下文水位监控（task #8 / 用户拍 Q1 = 半自动 B+C）。
//!
//! ## 它在做什么
//!
//! 订阅 EventBus 上的 [`EventKind::UsageReport`]——只关心 `meta.agent` 等于
//! 玄女 id 的那些。每条到达就累加到 per-spawn 总量；跨过 35% / 45% 阈值时**只
//! 触发一次**对应动作：
//!
//! - **35%**：emit `XuannvContextWatermark(action="addendum")` 同时经
//!   [`Intervener`] 注入一条 `system_origin="ctx_addendum"` 的提示消息：
//!   告诉玄女后续长话短说。
//! - **45%**：emit `XuannvContextWatermark(action="handoff_offer")` 同时注入
//!   `system_origin="context_handoff_offer"`：让玄女主动问用户「要不要重启
//!   副本」。fuxi-cli/im 那侧另有订阅者把这条 Watermark 翻成 PWA 通知。
//!
//! ## 重要约束
//!
//! - 玄女 spawn 周期重置：`xuannv_id` 变化（kill+respawn）时归零累加 + 已触
//!   阈值集合，下一个副本从 0 开始。
//! - **不**重复触发：同一 spawn 周期内 35% 跨过一次后不会再发 addendum，即使
//!   累加值在阈值附近抖动。
//! - 玄女自身的 cc adapter 也会发 UsageReport（user-turn task 也是 dispatch
//!   走 cc）——所以累加是真实的玄女 turn 总量，不是别的门客的。
//! - 公理 #3「真实时不轮询」：本模块走 `bus.subscribe()` 推送，无 polling。
//!
//! ## 公式
//!
//! 累加用 `UsageReport.total_tokens`（= input + cache_creation + output，**不**
//! 含 cache_read，见 [`UsageInfo::total`](fuxi_agent_cc::parser::UsageInfo)）。
//! 这条公式让 turn-by-turn 累加 ≈ 当前 context window 占用率。

use crate::Result;
use crate::bridge::Intervener;
use futures_util::StreamExt;
use fuxi_core::event::{Event, EventKind};
use fuxi_core::id::AgentId;
use fuxi_events::EventBus;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// 跨过即触发的阈值百分比（按 `UsageReport.pct * 100` 比对）。
/// 写死非动态：35% / 45% 是用户拍板的产品决策，不该从 env 暴露用户调（公理
/// #4 简单 > 灵活）；改阈值得改这数组 + 配套 addendum 文案。
const WATERMARK_THRESHOLDS: &[u8] = &[35, 45];

/// 35% 注入文案——给玄女的"长话短说"提示。
///
/// 措辞参考 cc `teammatePromptAddendum.ts` 风格：明示 + 给操作建议而非空喊。
const ADDENDUM_TEXT: &str = "[CTX_ADDENDUM] 你的 context 已用 ~35%（长尾 turn 多了几次）。\
    后续请长话短说：未必每件事都展开论证；\
    必要细节优先 Bash + Read 现取，不要复述全文。";

/// 45% 注入文案——让玄女主动问用户。
const HANDOFF_OFFER_TEXT: &str = "[CTX_HANDOFF_OFFER] 你的 context 已用 ~45%。\
    建议交接：请现在主动跟用户说一句「我 context 用了 X%（Y tokens / Z 总窗口），要不要重启副本？」。\
    若用户回「换」→ 你跑 `fuxi xuannv handoff write '<≤500字 markdown>'` 写交接信，后端会自动 kill 我并 spawn 新副本注入 prelude；\
    若用户回「继续」→ 暂不动作，下一轮再视情况。";

/// `system_origin` 字段值——让 PWA reducer 把这两条 intervene 渲染成"系统消息"
/// 灰底气泡而不是右侧 user bubble。
const ORIGIN_ADDENDUM: &str = "ctx_addendum";
const ORIGIN_HANDOFF_OFFER: &str = "context_handoff_offer";

/// 启动玄女上下文水位监控背景任务。
///
/// `xuannv_id_watch`：[`crate::Fuxi::xuannv_id_watch`] 给的 watch；玄女 id 变化
/// 时会重置内部累加状态。
/// `intervener`：注入 system_origin 消息到玄女对话；通常 = `Arc<Fuxi>`。
/// `bus`：用来 subscribe UsageReport 流 + publish XuannvContextWatermark。
///
/// 返回 JoinHandle 让调用方（fuxi-cli daemon）能在关闭路径 abort 它。
pub fn start_watcher(
    xuannv_id_watch: watch::Receiver<Option<AgentId>>,
    intervener: Arc<dyn Intervener>,
    bus: EventBus,
) -> JoinHandle<()> {
    let mut sub = bus.subscribe();
    tokio::spawn(async move {
        let mut state = WatcherState::default();
        info!("玄女上下文水位监控启动");
        while let Some(maybe_ev) = sub.next().await {
            let ev = match maybe_ev {
                Ok(e) => e,
                Err(err) => {
                    // broadcast Lagged 不致命——继续订阅丢几条；其它错（比如 Closed）
                    // 也走 continue，外层 next() 后续返 None 会自动退循环
                    debug!(?err, "玄女上下文水位 sub 收到错误事件，跳过");
                    continue;
                }
            };
            // xuannv_id 在 watch 里——每条事件取一次最新值；变化即重置。
            let current_xn = *xuannv_id_watch.borrow();
            if state.last_xuannv_id != current_xn {
                if state.last_xuannv_id.is_some() {
                    debug!(
                        old = ?state.last_xuannv_id,
                        new = ?current_xn,
                        "玄女 id 变化，重置上下文累加状态"
                    );
                }
                state.last_xuannv_id = current_xn;
                state.cumulative_total = 0;
                state.fired_thresholds.clear();
            }
            if let Err(err) =
                handle_event(&ev, &mut state, current_xn, intervener.as_ref(), &bus).await
            {
                warn!(?err, "玄女上下文水位 handler 报错——继续订阅");
            }
        }
        let _ = state;
        info!("玄女上下文水位监控退出（bus 关闭）");
    })
}

#[derive(Default)]
struct WatcherState {
    last_xuannv_id: Option<AgentId>,
    /// 当前 spawn 周期累计 total_tokens（不算 cache_read）。
    cumulative_total: u64,
    /// 本周期内已触发过的阈值——避免抖动重发。
    fired_thresholds: HashSet<u8>,
}

async fn handle_event(
    ev: &Event,
    state: &mut WatcherState,
    xuannv_id: Option<AgentId>,
    intervener: &dyn Intervener,
    bus: &EventBus,
) -> Result<()> {
    let Some(xn) = xuannv_id else {
        return Ok(());
    };
    let EventKind::UsageReport {
        total_tokens,
        window_size,
        ..
    } = &ev.kind
    else {
        return Ok(());
    };
    if ev.meta.agent != Some(xn) {
        return Ok(());
    }
    state.cumulative_total = state.cumulative_total.saturating_add(*total_tokens);
    let pct_now = state.cumulative_total as f64 / (*window_size).max(1) as f64;
    let pct_int = (pct_now * 100.0).floor() as u8;

    for &th in WATERMARK_THRESHOLDS {
        if pct_int < th || state.fired_thresholds.contains(&th) {
            continue;
        }
        state.fired_thresholds.insert(th);
        let action = match th {
            35 => "addendum",
            45 => "handoff_offer",
            _ => "unknown",
        };
        info!(
            xuannv = %xn,
            cumulative = state.cumulative_total,
            window = window_size,
            pct = pct_int,
            action,
            "玄女上下文跨越水位，触发动作"
        );
        // 1. emit Watermark 事件（给 PWA / Firehose 订阅）
        let mut meta = fuxi_core::event::EventMeta::now();
        meta.agent = Some(xn);
        let _ = bus.publish(Event {
            meta,
            kind: EventKind::XuannvContextWatermark {
                threshold_pct: th,
                total_tokens: state.cumulative_total,
                window_size: *window_size,
                action: action.to_string(),
            },
        });
        // 2. 注入 intervene 给玄女自己
        let (text, origin) = match th {
            35 => (ADDENDUM_TEXT.to_string(), ORIGIN_ADDENDUM),
            45 => (HANDOFF_OFFER_TEXT.to_string(), ORIGIN_HANDOFF_OFFER),
            _ => continue,
        };
        if let Err(err) = intervener.intervene_system(xn, false, &text, origin).await {
            warn!(?err, threshold = th, "玄女上下文水位 intervene 失败");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use fuxi_core::event::EventMeta;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::time::sleep;

    /// 测试用 Mock：记录每次 intervene 的 (target, text, origin) 三元组。
    #[derive(Default)]
    struct RecordingIntervener {
        calls: Mutex<Vec<(AgentId, String, Option<String>)>>,
    }

    #[async_trait]
    impl Intervener for RecordingIntervener {
        async fn intervene(&self, _target: AgentId, _interrupt: bool, _text: &str) -> Result<()> {
            unreachable!("水位监控只走 intervene_system")
        }
        async fn intervene_system(
            &self,
            target: AgentId,
            _interrupt: bool,
            text: &str,
            origin: &str,
        ) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((target, text.to_string(), Some(origin.to_string())));
            Ok(())
        }
        async fn role_of(&self, _agent_id: AgentId) -> Option<String> {
            None
        }
    }

    /// 给单测起一个 in-memory bus —— EventBus::new 现在需要 EventStore +
    /// EventBusConfig（参考 bridge.rs::bridge_survives_broadcast_lag）。
    async fn mk_test_bus() -> EventBus {
        let store = fuxi_events::EventStore::connect_memory()
            .await
            .expect("memory store");
        let cfg = fuxi_events::EventBusConfig {
            buffer: 64,
            writer_queue: 256,
            lag_threshold: 1024,
        };
        EventBus::new(store, cfg)
    }

    fn mk_usage(agent: AgentId, total: u64, window: u64) -> Event {
        let mut meta = EventMeta::now();
        meta.agent = Some(agent);
        Event {
            meta,
            kind: EventKind::UsageReport {
                input_tokens: 0,
                cache_creation_tokens: total,
                cache_read_tokens: 0,
                output_tokens: 0,
                total_tokens: total,
                window_size: window,
                pct: total as f32 / window as f32,
            },
        }
    }

    /// 主路径：单玄女单 spawn 周期，35% 跨过一次发 addendum；50% 时已不会
    /// 再触发 35（已在 fired_thresholds）。
    #[tokio::test]
    async fn crossing_35pct_fires_addendum_once() {
        let bus = mk_test_bus().await;
        let xn = AgentId::new();
        let (tx, rx) = watch::channel(Some(xn));
        let intv = Arc::new(RecordingIntervener::default());
        let handle = start_watcher(rx.clone(), intv.clone() as Arc<dyn Intervener>, bus.clone());
        // 让 watcher subscribe 起来——subscribe 是 sync 但 run 在 spawn 后
        sleep(Duration::from_millis(20)).await;

        // 第一条 30% → 不触发
        let _ = bus.publish(mk_usage(xn, 300_000, 1_000_000));
        sleep(Duration::from_millis(50)).await;
        assert!(intv.calls.lock().unwrap().is_empty(), "30% 不应触发");

        // 第二条把 cumulative 推到 360k → 跨 35% 触发 addendum
        let _ = bus.publish(mk_usage(xn, 60_000, 1_000_000));
        sleep(Duration::from_millis(50)).await;
        // 把数据 clone 出来立刻释放锁，避免 clippy::await_holding_lock 假警
        let calls = intv.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1, "35% 应触发一次");
        assert_eq!(calls[0].0, xn);
        assert!(calls[0].1.contains("CTX_ADDENDUM"));
        assert_eq!(calls[0].2.as_deref(), Some("ctx_addendum"));

        // 再发一条让 cumulative 到 380k（仍 < 45%）→ 不该重发 35%
        let _ = bus.publish(mk_usage(xn, 20_000, 1_000_000));
        sleep(Duration::from_millis(50)).await;
        assert_eq!(intv.calls.lock().unwrap().len(), 1, "已触发的 35% 不应重复");

        // 推到 460k → 跨 45% 触发 handoff_offer
        let _ = bus.publish(mk_usage(xn, 100_000, 1_000_000));
        sleep(Duration::from_millis(50)).await;
        let calls = intv.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2, "45% 应触发第二次");
        assert!(calls[1].1.contains("CTX_HANDOFF_OFFER"));
        assert_eq!(calls[1].2.as_deref(), Some("context_handoff_offer"));

        // 让出闭合
        let _ = tx;
        handle.abort();
    }

    /// 一上来直接 50% → 35 + 45 同时触发，按阈值升序各一次。
    #[tokio::test]
    async fn first_event_above_45pct_fires_both() {
        let bus = mk_test_bus().await;
        let xn = AgentId::new();
        let (tx, rx) = watch::channel(Some(xn));
        let intv = Arc::new(RecordingIntervener::default());
        let handle = start_watcher(rx.clone(), intv.clone() as Arc<dyn Intervener>, bus.clone());
        sleep(Duration::from_millis(20)).await;

        let _ = bus.publish(mk_usage(xn, 500_000, 1_000_000));
        sleep(Duration::from_millis(80)).await;
        let calls = intv.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 2, "首次 50% 应触发 35+45 各一次");
        assert!(calls[0].1.contains("CTX_ADDENDUM"));
        assert!(calls[1].1.contains("CTX_HANDOFF_OFFER"));
        let _ = tx;
        handle.abort();
    }

    /// 玄女 id 切换（kill+respawn）→ 累加归零；新副本要重新跨阈值才触发。
    #[tokio::test]
    async fn xuannv_respawn_resets_counter() {
        let bus = mk_test_bus().await;
        let xn1 = AgentId::new();
        let (tx, rx) = watch::channel(Some(xn1));
        let intv = Arc::new(RecordingIntervener::default());
        let handle = start_watcher(rx.clone(), intv.clone() as Arc<dyn Intervener>, bus.clone());
        sleep(Duration::from_millis(20)).await;
        // 老副本跨 35%
        let _ = bus.publish(mk_usage(xn1, 360_000, 1_000_000));
        sleep(Duration::from_millis(50)).await;
        assert_eq!(intv.calls.lock().unwrap().len(), 1);

        // 切到新玄女
        let xn2 = AgentId::new();
        tx.send(Some(xn2)).unwrap();
        sleep(Duration::from_millis(20)).await;
        // 新副本只发 200k → 不跨 35%（计数归零，cumulative=200k=20%）
        let _ = bus.publish(mk_usage(xn2, 200_000, 1_000_000));
        sleep(Duration::from_millis(50)).await;
        assert_eq!(
            intv.calls.lock().unwrap().len(),
            1,
            "新副本 20% 不应触发——计数已归零"
        );
        handle.abort();
    }

    /// 非玄女 agent 的 UsageReport 不影响累加（如鲁班）。
    #[tokio::test]
    async fn non_xuannv_usage_is_ignored() {
        let bus = mk_test_bus().await;
        let xn = AgentId::new();
        let other = AgentId::new();
        let (tx, rx) = watch::channel(Some(xn));
        let intv = Arc::new(RecordingIntervener::default());
        let handle = start_watcher(rx.clone(), intv.clone() as Arc<dyn Intervener>, bus.clone());
        sleep(Duration::from_millis(20)).await;
        let _ = bus.publish(mk_usage(other, 800_000, 1_000_000));
        sleep(Duration::from_millis(50)).await;
        assert!(
            intv.calls.lock().unwrap().is_empty(),
            "非玄女 UsageReport 应忽略"
        );
        let _ = tx;
        handle.abort();
    }
}

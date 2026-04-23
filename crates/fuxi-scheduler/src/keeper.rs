//! 更漏 Keeper —— tokio tick loop，驱动 cron / once trigger fire。
//!
//! 设计：
//! - 持有 `Arc<dyn Clock>`（可注入假时钟）+ `TriggerStore` + `EventBus`。
//! - 一个 `tokio::spawn` 无限 loop，按 `KeeperConfig.tick_interval` 滴答。
//! - 每 tick：读 `list_enabled_cron`，对每条算 `next_fire(last_fired_at | created_at, tz)`；
//!   若 `<= now` → 发 `EventKind::TriggerFired { cause: scheduled }` + append fire + `mark_success`。
//! - `TriggerFired` 事件由 orchestrator 下游消费（调度玄女）；Keeper 不做 agent 生命周期。
//!
//! 熔断语义：Keeper 本身不抛出失败——是 orchestrator 消费事件后若 spawn/派单失败，
//! 再通过 `TriggerStore::mark_failure` 记失败。Keeper 只"成功 fire"或"skip"。

use crate::spec::TriggerSpec;
use crate::store::{FireCause, FireRecord, FireStatus, TriggerRow, TriggerStore};
use crate::{Error, Result, new_fire_id};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use croner::Cron;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_events::EventBus;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, error, trace, warn};

/// 测试可注入的时钟抽象。production 用 `SystemClock`。
pub trait Clock: Send + Sync + std::fmt::Debug {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Keeper 可调参数。
#[derive(Debug, Clone)]
pub struct KeeperConfig {
    /// 两次 tick 间隔。默认 1s；测试可调小。
    pub tick_interval: Duration,
}

impl Default for KeeperConfig {
    fn default() -> Self {
        Self {
            tick_interval: Duration::from_secs(1),
        }
    }
}

/// Keeper——由 [`Keeper::spawn`] 启动，返回 `JoinHandle` 便于测试 abort。
pub struct Keeper {
    store: TriggerStore,
    bus: EventBus,
    clock: Arc<dyn Clock>,
    cfg: KeeperConfig,
}

impl Keeper {
    pub fn new(store: TriggerStore, bus: EventBus, clock: Arc<dyn Clock>) -> Self {
        Self {
            store,
            bus,
            clock,
            cfg: KeeperConfig::default(),
        }
    }

    pub fn with_config(mut self, cfg: KeeperConfig) -> Self {
        self.cfg = cfg;
        self
    }

    /// 起后台 tick loop。`JoinHandle::abort()` 可以立即停。
    ///
    /// 为什么消费 `Arc<Self>`：up.rs / repl.rs 需要同一个 Keeper instance
    /// 既起 tick loop 又挂在 Daemon 上走手动 fire；用 Arc clone 共享，避免双 instance
    /// 将来加内存状态（in-flight dedup set）时各自为政。
    pub fn spawn(self: Arc<Self>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(self.cfg.tick_interval);
            // 跳第一次 tick——interval 首次立即 ready。
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if let Err(e) = self.tick_once().await {
                    error!(error = %e, "Keeper tick 错误，继续下轮");
                }
            }
        })
    }

    /// 一次扫描 —— 暴露给测试直接调用。
    pub async fn tick_once(&self) -> Result<usize> {
        let now = self.clock.now();
        let triggers = self.store.list_enabled_cron().await?;
        let mut fired = 0usize;
        for row in triggers {
            if should_fire(&row, now)? {
                trace!(trigger_id = %row.id, "cron 到期，fire");
                if let Err(e) = self.fire_scheduled(&row, now).await {
                    warn!(trigger_id = %row.id, error = %e, "scheduled fire 失败");
                } else {
                    fired += 1;
                }
            }
        }
        // once 触发——到期就 fire 一次并 disable。
        for row in self.store.list_enabled().await? {
            if !matches!(row.spec, TriggerSpec::Once { .. }) {
                continue;
            }
            if should_fire(&row, now)? {
                trace!(trigger_id = %row.id, "once 到期，fire");
                if let Err(e) = self.fire_scheduled(&row, now).await {
                    warn!(trigger_id = %row.id, error = %e, "once fire 失败");
                    continue;
                }
                // one-shot：发完 disable
                if let Err(e) = self.store.set_enabled(&row.id, false).await {
                    warn!(trigger_id = %row.id, error = %e, "once disable 失败");
                }
                fired += 1;
            }
        }
        Ok(fired)
    }

    /// 写 fire 行 + 发 `TriggerFired`——对外暴露给 Watcher/Webhook/手动 fire 复用。
    pub async fn record_and_emit_fire(
        &self,
        trigger_id: &str,
        fired_at: DateTime<Utc>,
        cause: FireCause,
        payload: Option<serde_json::Value>,
    ) -> Result<()> {
        let record = FireRecord {
            id: new_fire_id(),
            trigger_id: trigger_id.to_string(),
            fired_at,
            cause,
            status: FireStatus::Dispatched,
            error: None,
            payload,
        };
        self.store.append_fire(&record).await?;
        self.store.mark_success(trigger_id, fired_at).await?;
        let ev = Event {
            meta: EventMeta::now(),
            kind: EventKind::TriggerFired {
                id: trigger_id.to_string(),
                fired_at,
                cause: cause.as_str().to_string(),
            },
        };
        // publish 失败通常意味着 bus 已关——不 panic，上层决定
        if let Err(e) = self.bus.publish(ev) {
            return Err(Error::Other(format!("publish TriggerFired: {e}")));
        }
        debug!(trigger_id, cause = cause.as_str(), "TriggerFired 已广播");
        Ok(())
    }

    async fn fire_scheduled(&self, row: &TriggerRow, now: DateTime<Utc>) -> Result<()> {
        self.record_and_emit_fire(&row.id, now, FireCause::Scheduled, None)
            .await
    }
}

/// 判断 trigger 在 `now` 时刻是否该 fire——纯函数，方便单测。
///
/// 语义：
/// - Cron：用 `last_fired_at | created_at` 作为"上一次 anchor"；`find_next_occurrence(anchor, false)`
///   给出下一次应触发时刻；若 `<= now` → fire。
/// - Once：`spec.at <= now && last_fired_at.is_none()` → fire。
/// - FsWatch / Webhook：Keeper 不负责（由响应式通路触发），返回 false。
pub fn should_fire(row: &TriggerRow, now: DateTime<Utc>) -> Result<bool> {
    match &row.spec {
        TriggerSpec::Cron { expr, tz } => {
            // `with_seconds_optional` 允许 5 字段 or 6 字段两种表达式（后者含秒）。
            let cron = Cron::new(expr.as_str())
                .with_seconds_optional()
                .parse()
                .map_err(|e| Error::Cron(format!("parse {expr:?}: {e}")))?;
            let anchor = row.last_fired_at.unwrap_or(row.created_at);
            if let Some(tz_name) = tz.as_deref()
                && !tz_name.trim().is_empty()
            {
                let timezone: Tz = tz_name
                    .parse()
                    .map_err(|e| Error::Cron(format!("invalid tz {tz_name:?}: {e}")))?;
                let anchor_tz = anchor.with_timezone(&timezone);
                let now_tz = now.with_timezone(&timezone);
                let next = cron
                    .find_next_occurrence(&anchor_tz, false)
                    .map_err(|e| Error::Cron(format!("next after {anchor_tz}: {e}")))?;
                return Ok(next <= now_tz);
            }
            let next = cron
                .find_next_occurrence(&anchor, false)
                .map_err(|e| Error::Cron(format!("next after {anchor}: {e}")))?;
            Ok(next <= now)
        }
        TriggerSpec::Once { at } => Ok(row.last_fired_at.is_none() && *at <= now),
        TriggerSpec::FsWatch { .. } | TriggerSpec::Webhook { .. } => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_trigger_id;
    use crate::store::NewTrigger;
    use chrono::{Duration as ChronoDuration, TimeZone, Timelike};
    use futures_util::StreamExt;
    use std::sync::Mutex;

    #[derive(Debug)]
    struct FakeClock {
        t: Mutex<DateTime<Utc>>,
    }

    impl FakeClock {
        fn new(t: DateTime<Utc>) -> Self {
            Self { t: Mutex::new(t) }
        }
        fn set(&self, t: DateTime<Utc>) {
            *self.t.lock().unwrap() = t;
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> DateTime<Utc> {
            *self.t.lock().unwrap()
        }
    }

    async fn fresh() -> (TriggerStore, EventBus) {
        let store = TriggerStore::connect_memory().await.expect("store");
        let bus = EventBus::with_memory_store().await.expect("bus");
        (store, bus)
    }

    #[test]
    fn parses_every_five_minutes() {
        // `*/5 * * * *` = 5 字段：minute hour dom month dow；每 5 分钟。
        let c = Cron::new("*/5 * * * *")
            .with_seconds_optional()
            .parse()
            .expect("parse");
        let at = Utc.with_ymd_and_hms(2026, 4, 19, 9, 3, 0).unwrap();
        let next = c.find_next_occurrence(&at, false).expect("next");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 4, 19, 9, 5, 0).unwrap());
    }

    #[test]
    fn parses_seconds_expression() {
        let c = Cron::new("*/2 * * * * *")
            .with_seconds_optional()
            .parse()
            .expect("parse");
        let at = Utc.with_ymd_and_hms(2026, 4, 19, 9, 0, 1).unwrap();
        let next = c.find_next_occurrence(&at, false).expect("next");
        assert_eq!(next.second() % 2, 0);
    }

    #[test]
    fn should_fire_cron_respects_last_fired_at() {
        // 用 `*/5 * * * *`；now = 9:08:00；last_fired_at = 9:06:30 —— 下一 tick 9:10，未到 → 不该 fire。
        let now = Utc.with_ymd_and_hms(2026, 4, 19, 9, 8, 0).unwrap();
        let mut row = TriggerRow {
            id: new_trigger_id(),
            spec: TriggerSpec::Cron {
                expr: "*/5 * * * *".into(),
                tz: None,
            },
            intent: "x".into(),
            session_id: None,
            enabled: true,
            consecutive_failures: 0,
            max_failures: 5,
            last_fired_at: Some(Utc.with_ymd_and_hms(2026, 4, 19, 9, 6, 30).unwrap()),
            created_at: now - ChronoDuration::hours(1),
        };
        assert!(
            !should_fire(&row, now).expect("ok"),
            "9:06:30 last, now=9:08, 下次 9:10 未到"
        );

        // 把 now 推到 9:10 —— 应 fire。
        let now2 = Utc.with_ymd_and_hms(2026, 4, 19, 9, 10, 0).unwrap();
        assert!(should_fire(&row, now2).expect("ok"), "9:10 到点应 fire");

        // last_fired_at = 9:10 后再问 9:10：下一 tick 是 9:15，未到 → 不 fire。
        row.last_fired_at = Some(now2);
        assert!(
            !should_fire(&row, now2).expect("ok"),
            "刚 fire 过 9:10，同一秒不再 fire"
        );
    }

    #[test]
    fn should_fire_cron_respects_tz_when_provided() {
        let row = TriggerRow {
            id: new_trigger_id(),
            spec: TriggerSpec::Cron {
                expr: "0 9 * * *".into(),
                tz: Some("Asia/Shanghai".into()),
            },
            intent: "tz".into(),
            session_id: None,
            enabled: true,
            consecutive_failures: 0,
            max_failures: 5,
            last_fired_at: None,
            // UTC 00:00 = 上海 08:00，下一次 09:00 本地应是 UTC 01:00
            created_at: Utc.with_ymd_and_hms(2026, 4, 23, 0, 0, 0).unwrap(),
        };
        let now = Utc.with_ymd_and_hms(2026, 4, 23, 1, 0, 5).unwrap();
        assert!(should_fire(&row, now).expect("ok"));
    }

    #[test]
    fn should_fire_once_only_once() {
        let now = Utc.with_ymd_and_hms(2026, 4, 19, 9, 0, 0).unwrap();
        let at = now - ChronoDuration::seconds(1);
        let mut row = TriggerRow {
            id: new_trigger_id(),
            spec: TriggerSpec::Once { at },
            intent: "x".into(),
            session_id: None,
            enabled: true,
            consecutive_failures: 0,
            max_failures: 5,
            last_fired_at: None,
            created_at: now - ChronoDuration::hours(1),
        };
        assert!(should_fire(&row, now).expect("ok"));
        row.last_fired_at = Some(now);
        assert!(!should_fire(&row, now).expect("ok"), "once 已 fire 过");
    }

    #[test]
    fn should_fire_webhook_always_false() {
        let now = Utc::now();
        let row = TriggerRow {
            id: new_trigger_id(),
            spec: TriggerSpec::Webhook { secret: None },
            intent: "x".into(),
            session_id: None,
            enabled: true,
            consecutive_failures: 0,
            max_failures: 5,
            last_fired_at: None,
            created_at: now - ChronoDuration::hours(1),
        };
        assert!(!should_fire(&row, now).expect("ok"));
    }

    /// 集成测试——假时钟 + 秒级 cron：跳 5 秒应有 2 次 fire（t=2, t=4）。
    /// 公约：`*/2 * * * * *` 每 2 秒一次；anchor 从 created_at 起。
    #[tokio::test]
    async fn keeper_fires_seconds_cron_with_fake_clock() {
        let (store, bus) = fresh().await;
        let base = Utc.with_ymd_and_hms(2026, 4, 19, 9, 0, 0).unwrap();
        let clock = Arc::new(FakeClock::new(base));
        let new = NewTrigger {
            id: new_trigger_id(),
            spec: TriggerSpec::Cron {
                expr: "*/2 * * * * *".into(),
                tz: None,
            },
            intent: "heartbeat".into(),
            session_id: None,
            max_failures: None,
        };
        let row = store.insert(new).await.expect("insert");
        // 手动把 created_at 锚到 base 之前 1 秒——避免 croner 起步时刻的边界模糊。
        sqlx::query("UPDATE triggers SET created_at = ?1 WHERE id = ?2")
            .bind((base - ChronoDuration::seconds(1)).to_rfc3339())
            .bind(&row.id)
            .execute(store_pool(&store))
            .await
            .expect("update created_at");

        let mut sub = bus.subscribe();
        let keeper = Keeper::new(store.clone(), bus.clone(), clock.clone());
        // 连续推进时钟，每步调用 tick_once——比起真 interval 稳定。
        let mut total = 0usize;
        for sec in 1..=5 {
            clock.set(base + ChronoDuration::seconds(sec));
            total += keeper.tick_once().await.expect("tick");
        }
        assert!(total >= 2, "5 秒内至少 2 次 fire，实际 {total}");

        // 消化事件——应见到 >=2 条 TriggerFired，cause=scheduled
        let mut fired_events = 0;
        for _ in 0..10 {
            if let Ok(Some(Ok(ev))) =
                tokio::time::timeout(std::time::Duration::from_millis(100), sub.next()).await
            {
                if let EventKind::TriggerFired { cause, .. } = ev.kind
                    && cause == "scheduled"
                {
                    fired_events += 1;
                }
            } else {
                break;
            }
        }
        assert!(
            fired_events >= 2,
            "bus 上应至少 2 条 TriggerFired，实际 {fired_events}"
        );

        // DB 端应有 >=2 条 fire 行
        let fires = store.list_fires(&row.id).await.expect("fires");
        assert!(fires.len() >= 2, "DB fires: {}", fires.len());
        for f in &fires {
            assert_eq!(f.cause, FireCause::Scheduled);
            assert_eq!(f.status, FireStatus::Dispatched);
        }
    }

    #[tokio::test]
    async fn manual_fire_emits_event_and_appends_record() {
        let (store, bus) = fresh().await;
        let base = Utc.with_ymd_and_hms(2026, 4, 19, 9, 0, 0).unwrap();
        let clock = Arc::new(FakeClock::new(base));
        let new = NewTrigger {
            id: new_trigger_id(),
            spec: TriggerSpec::Cron {
                expr: "0 9 * * *".into(),
                tz: None,
            },
            intent: "daily".into(),
            session_id: None,
            max_failures: None,
        };
        let row = store.insert(new).await.expect("insert");

        let mut sub = bus.subscribe();
        let keeper = Keeper::new(store.clone(), bus.clone(), clock);
        keeper
            .record_and_emit_fire(&row.id, base, FireCause::Manual, None)
            .await
            .expect("manual fire");

        let ev = tokio::time::timeout(std::time::Duration::from_millis(500), sub.next())
            .await
            .expect("timeout")
            .expect("closed")
            .expect("err");
        match ev.kind {
            EventKind::TriggerFired { id, cause, .. } => {
                assert_eq!(id, row.id);
                assert_eq!(cause, "manual");
            }
            other => panic!("期望 TriggerFired，got {other:?}"),
        }

        let fires = store.list_fires(&row.id).await.expect("fires");
        assert_eq!(fires.len(), 1);
        assert_eq!(fires[0].cause, FireCause::Manual);
    }

    /// BUG-5 回归：up.rs / repl.rs 期望 Arc<Keeper> 单 instance
    /// 同时承担 tick loop 和手动 fire 两职责。spawn 必须消费 Arc<Self>——
    /// 若它还是消费 `self`，下面 `Arc::clone(&keeper).spawn()` 就根本编译不过。
    #[tokio::test]
    async fn keeper_shared_arc_tick_and_manual_fire() {
        let (store, bus) = fresh().await;
        let base = Utc.with_ymd_and_hms(2026, 4, 19, 9, 0, 0).unwrap();
        let clock = Arc::new(FakeClock::new(base));
        let new = NewTrigger {
            id: new_trigger_id(),
            spec: TriggerSpec::Cron {
                expr: "*/1 * * * * *".into(),
                tz: None,
            },
            intent: "heartbeat".into(),
            session_id: None,
            max_failures: None,
        };
        let row = store.insert(new).await.expect("insert");
        // anchor created_at 到 base 之前——tick 第一次扫应见 cron 到期。
        sqlx::query("UPDATE triggers SET created_at = ?1 WHERE id = ?2")
            .bind((base - ChronoDuration::seconds(1)).to_rfc3339())
            .bind(&row.id)
            .execute(store_pool(&store))
            .await
            .expect("update created_at");

        // 一个 Arc<Keeper>，clone 两份：一份 spawn tick loop，一份走手动 fire。
        let keeper = Arc::new(
            Keeper::new(store.clone(), bus.clone(), clock.clone()).with_config(KeeperConfig {
                tick_interval: Duration::from_millis(10),
            }),
        );
        // 把 FakeClock 推到 base+2s；tick_once 读到的 now 会让 cron 触发。
        clock.set(base + ChronoDuration::seconds(2));
        let tick_handle = Arc::clone(&keeper).spawn();

        // 等 tick loop 跑几轮——interval=10ms，50ms 足够至少 1 次。
        tokio::time::sleep(Duration::from_millis(80)).await;

        // 同一 Arc<Keeper> 另一份 clone 用来手动 fire。
        keeper
            .record_and_emit_fire(
                &row.id,
                base + ChronoDuration::seconds(2),
                FireCause::Manual,
                None,
            )
            .await
            .expect("manual fire");

        tick_handle.abort();

        // DB 里应同时有 Scheduled 和 Manual 两种 fire——证明共享的是同一 store。
        let fires = store.list_fires(&row.id).await.expect("fires");
        let has_scheduled = fires.iter().any(|f| f.cause == FireCause::Scheduled);
        let has_manual = fires.iter().any(|f| f.cause == FireCause::Manual);
        assert!(
            has_scheduled,
            "tick loop 应产生 Scheduled fire，fires={fires:?}"
        );
        assert!(has_manual, "手动 fire 应入库，fires={fires:?}");
    }

    fn store_pool(store: &TriggerStore) -> &sqlx::SqlitePool {
        // 测试专用——通过反射？不，我们在 store 里没暴露 pool。
        // 曲线救国：测试用的 fresh() 每次新建，所以这里直接用 unsafe transmute 风险太大。
        // 改用 store::TriggerStore::pool() 访问器（只在 test cfg 下开）。
        store.pool_for_test()
    }
}

//! IP 维度的登入失败计数 + 锁定（β · Task #9）。
//!
//! 攻击模型：公网暴露的 `/api/auth/login` 可能被 dictionary attack 撞开。
//! 防御：
//! - **滑动窗口**：每 IP 在 `WINDOW` 内累计失败次数；超 `MAX_FAILS` → 锁定 `LOCKOUT`
//! - **锁定期间** 即使密码对也拒（避免 attacker 蒙到时再用另一台机继续撞）
//! - **成功登入清零**：合法用户偶尔输错不该被锁久
//!
//! 不是全局 rate limit ——nginx 的 `limit_req` 做总流量防滥用；本模块专攻"单 IP
//! 暴力撞密码"。
//!
//! ## 时钟注入
//!
//! 测试要快进时间，所以接口接受 `now` 参数；生产用 `Instant::now()`。

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 滑动窗口长度——窗内失败次数累计。
pub const WINDOW: Duration = Duration::from_secs(60);

/// 触发锁定的失败次数阈值。
pub const MAX_FAILS: u32 = 5;

/// 锁定持续时间。
pub const LOCKOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy)]
struct Record {
    /// 当前窗口内失败计数。
    fails: u32,
    /// 当前窗口起点。
    window_start: Instant,
    /// 锁定到期时刻（None = 未锁）。
    locked_until: Option<Instant>,
}

/// IP 失败计数表。
#[derive(Default)]
pub struct LoginGuard {
    inner: Mutex<HashMap<IpAddr, Record>>,
}

/// 检查结果：允许进入还是被锁。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardDecision {
    /// 允许尝试登入。
    Allow,
    /// 已锁定——剩余多少秒到解禁。
    Locked { remaining_secs: u64 },
}

impl LoginGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登入尝试前调一次：如果该 IP 当前被锁则返 `Locked`，handler 应回 401 + 提示。
    pub fn check(&self, ip: IpAddr, now: Instant) -> GuardDecision {
        let mut map = self.inner.lock().expect("login guard mutex poisoned");
        if let Some(rec) = map.get(&ip)
            && let Some(until) = rec.locked_until
        {
            if now < until {
                let remaining_secs = until.saturating_duration_since(now).as_secs();
                return GuardDecision::Locked { remaining_secs };
            }
            // 锁定到期：清除锁，重置计数（让该 IP 再试）
            if let Some(rec) = map.get_mut(&ip) {
                rec.locked_until = None;
                rec.fails = 0;
                rec.window_start = now;
            }
        }
        GuardDecision::Allow
    }

    /// 登入失败时调一次：累计计数，达到阈值则锁定。
    pub fn record_failure(&self, ip: IpAddr, now: Instant) {
        let mut map = self.inner.lock().expect("login guard mutex poisoned");
        let rec = map.entry(ip).or_insert(Record {
            fails: 0,
            window_start: now,
            locked_until: None,
        });
        // 滑动窗口：窗口外的旧失败重置
        if now.saturating_duration_since(rec.window_start) >= WINDOW {
            rec.fails = 0;
            rec.window_start = now;
        }
        rec.fails = rec.fails.saturating_add(1);
        if rec.fails >= MAX_FAILS {
            rec.locked_until = Some(now + LOCKOUT);
        }
    }

    /// 登入成功时调一次：清零，给合法用户偶尔输错的容忍。
    pub fn record_success(&self, ip: IpAddr) {
        let mut map = self.inner.lock().expect("login guard mutex poisoned");
        map.remove(&ip);
    }

    /// 测试 / 调试用：当前在册 IP 数。
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn fresh_ip_is_allowed() {
        let g = LoginGuard::new();
        let now = Instant::now();
        assert_eq!(g.check(ip(1, 2, 3, 4), now), GuardDecision::Allow);
    }

    #[test]
    fn under_threshold_still_allowed() {
        let g = LoginGuard::new();
        let now = Instant::now();
        let i = ip(1, 2, 3, 4);
        for _ in 0..(MAX_FAILS - 1) {
            g.record_failure(i, now);
        }
        // 4 次失败后还没到 5 次阈值——仍 Allow
        assert_eq!(g.check(i, now), GuardDecision::Allow);
    }

    #[test]
    fn fifth_failure_triggers_lockout() {
        let g = LoginGuard::new();
        let now = Instant::now();
        let i = ip(1, 2, 3, 4);
        for _ in 0..MAX_FAILS {
            g.record_failure(i, now);
        }
        match g.check(i, now) {
            GuardDecision::Locked { remaining_secs } => {
                assert!(
                    remaining_secs > 0 && remaining_secs <= LOCKOUT.as_secs(),
                    "锁定剩余 {remaining_secs}s 应在 (0, {}] 区间",
                    LOCKOUT.as_secs()
                );
            }
            other => panic!("应锁定，得到 {other:?}"),
        }
    }

    #[test]
    fn lockout_expires_after_duration() {
        let g = LoginGuard::new();
        let t0 = Instant::now();
        let i = ip(1, 2, 3, 4);
        for _ in 0..MAX_FAILS {
            g.record_failure(i, t0);
        }
        // 锁定中
        assert!(matches!(g.check(i, t0), GuardDecision::Locked { .. }));

        // 5 分钟后 +1ms：解禁
        let t_after = t0 + LOCKOUT + Duration::from_millis(1);
        assert_eq!(g.check(i, t_after), GuardDecision::Allow);
    }

    #[test]
    fn other_ip_is_not_affected_by_lockout() {
        let g = LoginGuard::new();
        let now = Instant::now();
        let attacker = ip(1, 2, 3, 4);
        let bystander = ip(5, 6, 7, 8);
        for _ in 0..MAX_FAILS {
            g.record_failure(attacker, now);
        }
        assert!(matches!(
            g.check(attacker, now),
            GuardDecision::Locked { .. }
        ));
        // 旁观 IP 不受影响
        assert_eq!(g.check(bystander, now), GuardDecision::Allow);
    }

    #[test]
    fn success_clears_failure_count() {
        let g = LoginGuard::new();
        let now = Instant::now();
        let i = ip(1, 2, 3, 4);
        // 4 次失败（差一次锁定）
        for _ in 0..(MAX_FAILS - 1) {
            g.record_failure(i, now);
        }
        // 成功清零
        g.record_success(i);
        // 再失败 4 次还是 Allow（计数从 0 重起）
        for _ in 0..(MAX_FAILS - 1) {
            g.record_failure(i, now);
        }
        assert_eq!(g.check(i, now), GuardDecision::Allow);
    }

    #[test]
    fn old_failures_outside_window_dont_count() {
        // 时间分布：t=0 累计 4 次失败；窗口 60s 后再失败 1 次——总该在新窗口
        // 计数为 1 而非 5，所以不该锁
        let g = LoginGuard::new();
        let i = ip(1, 2, 3, 4);
        let t0 = Instant::now();
        for _ in 0..(MAX_FAILS - 1) {
            g.record_failure(i, t0);
        }
        let t_later = t0 + WINDOW + Duration::from_millis(1);
        g.record_failure(i, t_later);
        // 应仍未锁——窗口已切，旧 4 次不算
        assert_eq!(g.check(i, t_later), GuardDecision::Allow);
    }
}

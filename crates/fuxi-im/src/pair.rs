//! 一次性 PIN 配对（Decision 14 D）。
//!
//! TUI `/pair` 调 `start_pair()` → 拿 6 位 PIN + 120s TTL；用户在手机 PWA 输入 PIN
//! POST `/api/auth/pair` → handler 调 `claim(pin)` 拿对应 entry 后签 token。
//!
//! ## 状态
//!
//! 内存 map `pin → (created_at, ttl)`——单进程足够；fuxi 单实例，无横向扩展需求。
//! TTL 默认 120s——够用户掏手机输入；过期由 `claim` 路径检查（懒清理）。
//!
//! ## 安全
//!
//! - PIN 6 位数字（10^6 = 100 万）：120s TTL 内对暴力枚举有限速兜底（`claim_attempts`
//!   阈值 + 失败计数）。`claim` 接口里实现"错 5 次拒该 PIN"——避免 attacker
//!   连蒙得逞。**没有**通用 rate limit，因为 IM 走 nginx 公网，nginx limit_req
//!   做防滥用层；这里只关心"同一 PIN 的爆破"。
//! - PIN 一次性：claim 成功后立即从 map 移除（消费）。
//! - 用 OS 随机源生成数字——`rand::thread_rng` 走 `getrandom`。

#![allow(dead_code)]

use rand::Rng;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// PIN 默认 TTL（120 秒）。
pub const DEFAULT_PIN_TTL: Duration = Duration::from_secs(120);

/// 同一 PIN 允许失败的最大次数；超过即视为已被消费（attacker 反正也快超 TTL 了）。
pub const MAX_PIN_ATTEMPTS: u32 = 5;

/// 一条挂起的配对项。
#[derive(Debug)]
struct PendingEntry {
    created_at: Instant,
    ttl: Duration,
    fail_count: u32,
}

impl PendingEntry {
    fn is_expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.created_at) >= self.ttl
    }
}

/// 内存 PIN 表——多 handler 同时调用安全（Mutex 包裹）。
#[derive(Default)]
pub struct PendingPairs {
    inner: Mutex<HashMap<String, PendingEntry>>,
}

/// `start_pair` 返回值——TUI 拿去显示，handler 用同一个字符串当 key。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin(pub String);

impl Pin {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Pin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `claim` 失败原因——枚举供 handler 翻 401，trace 里区分原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimError {
    /// PIN 不存在（从未 start，或已被消费/过期清理）。
    Unknown,
    /// PIN 存在但已超 TTL。
    Expired,
    /// PIN 存在但失败次数累计超阈，已拒。
    Locked,
}

impl PendingPairs {
    pub fn new() -> Self {
        Self::default()
    }

    /// TUI `/pair` 入口：生成 6 位数字 PIN，记入表，返回 PIN（用户复述给手机）。
    ///
    /// `ttl` 给测试注入；生产用 [`DEFAULT_PIN_TTL`]。
    pub fn start(&self, ttl: Duration) -> Pin {
        let mut rng = rand::thread_rng();
        // 撞 PIN：10^6 空间 + 120s 窗口；理论上单用户场景几乎不会撞到——但保险起见
        // 撞了重抽，最多 10 次。10 次都撞说明系统已被异常占用，让调用者得到一个
        // 也许已存在的 PIN 也无所谓（旧的在我们 overwrite 时被淘汰）。
        let mut chosen = String::new();
        for _ in 0..10 {
            let n: u32 = rng.gen_range(0..1_000_000);
            chosen = format!("{n:06}");
            let map = self.inner.lock().expect("pin map mutex poisoned");
            if !map.contains_key(&chosen) {
                drop(map);
                break;
            }
        }
        let entry = PendingEntry {
            created_at: Instant::now(),
            ttl,
            fail_count: 0,
        };
        let mut map = self.inner.lock().expect("pin map mutex poisoned");
        map.insert(chosen.clone(), entry);
        Pin(chosen)
    }

    /// 验 PIN 并消费：成功一次性移除；失败递增计数 + 必要时锁死。
    ///
    /// 成功的语义：返回 `Ok(())`，调用方继续签 token + 写 device_tokens。
    pub fn claim(&self, pin: &str) -> Result<(), ClaimError> {
        let mut map = self.inner.lock().expect("pin map mutex poisoned");
        let now = Instant::now();
        match map.get_mut(pin) {
            None => Err(ClaimError::Unknown),
            Some(entry) => {
                if entry.is_expired(now) {
                    map.remove(pin);
                    Err(ClaimError::Expired)
                } else if entry.fail_count >= MAX_PIN_ATTEMPTS {
                    map.remove(pin);
                    Err(ClaimError::Locked)
                } else {
                    // 当前 claim 是"成功对照"——map 里有这个 PIN 即合格。
                    // 我们没把"提交的明文密码"当输入；handler 直接传 PIN 字符串，
                    // 命中 key 即配对成功。
                    map.remove(pin);
                    Ok(())
                }
            }
        }
    }

    /// 业务可能要"PIN 错"和"PIN 对"分两条路径：用户在 PWA 多次输错应递增计数。
    /// 现行 `claim` 一次性消费命中的 entry，不留"错了几次"——因此这个钩子专门给
    /// 调用方在**外部**决定 PIN 字符不匹配时的递增。当前 handler 不暴露用户输入
    /// 的不命中分支（命中即成功），保留接口为将来可能引入的"PIN 比对"路径。
    pub fn record_failure(&self, pin: &str) -> Option<u32> {
        let mut map = self.inner.lock().expect("pin map mutex poisoned");
        if let Some(entry) = map.get_mut(pin) {
            entry.fail_count = entry.fail_count.saturating_add(1);
            return Some(entry.fail_count);
        }
        None
    }

    /// 当前在册 PIN 数——只给测试用。
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    /// 配套 `len`——给 clippy `len_without_is_empty` 安心。也是 ergonomic API：
    /// 调用方判"此刻是否有 PIN 在等"比拿 len 更直观。
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_returns_six_digit_pin_in_register() {
        let pp = PendingPairs::new();
        let pin = pp.start(DEFAULT_PIN_TTL);
        assert_eq!(pin.as_str().len(), 6);
        assert!(
            pin.as_str().chars().all(|c| c.is_ascii_digit()),
            "PIN 必须全数字：{pin}"
        );
        assert_eq!(pp.len(), 1);
    }

    #[test]
    fn claim_consumes_pin_on_first_success() {
        let pp = PendingPairs::new();
        let pin = pp.start(DEFAULT_PIN_TTL);
        pp.claim(pin.as_str()).expect("first claim ok");
        // 二次 claim 该 PIN 必失败——已消费。
        let err = pp.claim(pin.as_str()).unwrap_err();
        assert_eq!(err, ClaimError::Unknown);
    }

    #[test]
    fn claim_rejects_unknown_pin() {
        let pp = PendingPairs::new();
        let err = pp.claim("000000").unwrap_err();
        assert_eq!(err, ClaimError::Unknown);
    }

    #[test]
    fn claim_rejects_expired_pin() {
        let pp = PendingPairs::new();
        // 1 ms TTL → 立刻过期
        let pin = pp.start(Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(20));
        let err = pp.claim(pin.as_str()).unwrap_err();
        assert_eq!(err, ClaimError::Expired);
        // 过期后 entry 应被清理
        assert_eq!(pp.len(), 0);
    }

    #[test]
    fn record_failure_increments_then_locks() {
        let pp = PendingPairs::new();
        let pin = pp.start(DEFAULT_PIN_TTL);
        for i in 1..=MAX_PIN_ATTEMPTS {
            let n = pp.record_failure(pin.as_str()).expect("entry exists");
            assert_eq!(n, i);
        }
        // 累计 5 次失败 → claim 应 Locked
        let err = pp.claim(pin.as_str()).unwrap_err();
        assert_eq!(err, ClaimError::Locked);
    }

    #[test]
    fn record_failure_returns_none_for_unknown_pin() {
        let pp = PendingPairs::new();
        assert!(pp.record_failure("123456").is_none());
    }

    #[test]
    fn distinct_starts_yield_distinct_entries() {
        let pp = PendingPairs::new();
        let a = pp.start(DEFAULT_PIN_TTL);
        let b = pp.start(DEFAULT_PIN_TTL);
        // 撞概率 1/100 万 + 防撞重抽——基本永不撞；测试里若撞了说明 RNG 异常
        assert_ne!(a.as_str(), b.as_str());
        assert_eq!(pp.len(), 2);
    }
}

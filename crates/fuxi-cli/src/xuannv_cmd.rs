//! `fuxi xuannv refresh` —— 让玄女下次 fresh session 加载最新 dispatch-routing 教学。
//!
//! ## 为什么需要
//!
//! 玄女是长跑 cc 进程：`ensure_xuannv` 启动时若 oracle 里有 xuannv/session_id
//! record → 走 `cc --resume <session>` 续写。cc 自身行为：resume 时用**老 session
//! 的** system prompt，**忽略** `--append-system-prompt`。
//!
//! 后果：我们更新 `roles/xuannv/instructions/dispatch-routing.md` 教新东西
//! （比如 `fuxi spawn --project / --ephemeral` 用法），玄女永远学不到——除非
//! fresh session。
//!
//! ## 用法
//!
//! ```bash
//! fuxi xuannv refresh         # 清 oracle 里 session_id record
//! systemctl --user restart fuxi-im   # 触发 ensure_xuannv 走 fresh session 路径
//! ```
//!
//! 重启后 oracle 里没 session record → resolve_xuannv_session 走 fresh path →
//! cc 启动加 `--append-system-prompt`（含 dispatch-routing.md 最新版）→ 玄女
//! 学到新教学。
//!
//! ## 代价
//!
//! 玄女失忆——cc session 历史断档。下次启动是全新对话，不知道之前用户说了啥。
//! 这是单次成本：教学更新频率应远低于对话连续性需求。

use anyhow::{Context, Result};
use fuxi_memory::OracleStore;

use crate::session;

/// daemon / IM 默认 events.db 路径——跟 `fuxi im start` 用同一份
/// (`im.rs::default_events_db_path` SoT：`$HOME/.fuxi/events.db`)。
fn default_events_db_path() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".fuxi").join("events.db"))
        .unwrap_or_else(|| std::path::PathBuf::from(".fuxi/events.db"))
}

pub async fn run_refresh() -> Result<()> {
    let path = std::env::var("FUXI_EVENTS_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| default_events_db_path());

    if !path.exists() {
        println!("策府数据库不存在：{}", path.display());
        println!("→ 没历史 session record 可清——下次 fuxi-im 启动自动 fresh.");
        return Ok(());
    }

    let oracle = OracleStore::connect_file(&path)
        .await
        .with_context(|| format!("打开策府 {}", path.display()))?;

    let cleared = session::forget_xuannv_session(&oracle)
        .await
        .context("清 oracle xuannv session record 失败")?;

    if cleared == 0 {
        println!("没找到玄女 session record（之前就是 fresh）。");
    } else {
        println!("已清 {cleared} 条玄女 session record。");
    }
    println!();
    println!("下一步：");
    println!("  systemctl --user restart fuxi-im");
    println!();
    println!("（触发 ensure_xuannv 走 fresh session 路径，cc 重读");
    println!("  `--append-system-prompt`，含 dispatch-routing.md 最新版）");
    Ok(())
}

//! IM 子系统专用 SQLite 数据库。
//!
//! 设计取舍：跟 events 库（事件审计）/ memory 库（记忆）平级独立，理由
//!   - IM 持的是订阅 / 设备 token / silence 状态，跟事件 append-only 日志
//!     是两个域，不该混表
//!   - 多 owner（β/δ）共享一个 pool，schema 变更用 sqlx 迁移文件按字母序合并
//!
//! 文件位置：`~/.fuxi/im.db`（与 `~/.fuxi/im_vapid.json` / `~/.fuxi/im_hmac.key`
//! 同目录）。`init_at()` 给单测注入 tempdir 路径用。

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::error::{Error, Result};

/// `sqlx::migrate!` 在编译期把 `migrations/*.sql` 嵌进二进制；运行时
/// 不依赖路径——和 fuxi-events 用 `include_str!` 同样的部署友好性，但用
/// migration runner 让多个 SQL 文件的 idempotent 应用免去手写。
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// 解析默认 `~/.fuxi/im.db`。返回 None 表示无 `$HOME`——调用方应 fallback
/// 到显式路径或 fail-fast，不要静默写到 cwd。
pub fn default_db_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".fuxi").join("im.db"))
}

/// 用显式路径构造 IM SqlitePool 并跑迁移。父目录不存在则创建。
///
/// WAL 模式 + busy_timeout：跟 fuxi-events 同一套防 BUSY 套路（macOS 偶发）。
pub async fn init_at(path: impl AsRef<Path>) -> Result<SqlitePool> {
    let path = path.as_ref();
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(Error::Io)?;
    }

    let url = format!("sqlite://{}", path.display());
    let opts = SqliteConnectOptions::from_str(&url)
        .map_err(|e| Error::Internal(format!("sqlite opts: {e}")))?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await
        .map_err(|e| Error::Internal(format!("sqlite connect: {e}")))?;

    MIGRATOR
        .run(&pool)
        .await
        .map_err(|e| Error::Internal(format!("im migrate: {e}")))?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// 建库 + 迁移——push_subscriptions 表存在且字段完整。
    #[tokio::test]
    async fn init_creates_push_subscriptions_table() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im.db");
        let pool = init_at(&path).await.expect("init");

        // sqlite_master 查表存在
        let table_name: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='push_subscriptions'",
        )
        .fetch_optional(&pool)
        .await
        .expect("query master");
        assert_eq!(table_name.as_deref(), Some("push_subscriptions"));

        // 列必须有 silence_until（client visibility hook 会写）
        let col: Option<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('push_subscriptions') WHERE name='silence_until'",
        )
        .fetch_optional(&pool)
        .await
        .expect("query columns");
        assert_eq!(col.as_deref(), Some("silence_until"));
    }

    /// 父目录不存在也能建库——`~/.fuxi` 首启时尚未创建。
    #[tokio::test]
    async fn init_creates_parent_dirs() {
        let dir = tempdir().expect("tmp");
        let nested = dir.path().join("nested").join("im.db");
        let _pool = init_at(&nested).await.expect("init");
        assert!(nested.exists(), "db file 应已落盘: {}", nested.display());
    }

    /// 重复 init 同路径幂等——迁移已应用就不重复 apply。
    #[tokio::test]
    async fn init_is_idempotent() {
        let dir = tempdir().expect("tmp");
        let path = dir.path().join("im.db");
        let _p1 = init_at(&path).await.expect("first");
        let _p2 = init_at(&path).await.expect("second");
        // 没 panic 即过——sqlx::migrate 自带 _sqlx_migrations 表去重。
    }
}

//! 贤士录（ledger.jsonl）—— append-only JSON Lines 审计日志。
//!
//! 每次招贤的 stage / approve / reject / activate 都写一行。格式:
//! ```json
//! {"at":"2026-04-19T12:00:00Z","subject":"painter","action":"staged","reason":"...","approver":"xuannv"}
//! ```
//!
//! 为什么 JSONL：一行一条，支持 `tail -f`、grep；不引入 schema 迁移成本。
//! 写入用 OpenOptions::append，多写者原子追加（POSIX `O_APPEND` 保证单 write 不交错）。

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// 招贤审计的动作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerAction {
    Staged,
    Approved,
    Rejected,
    Activated,
}

/// 一条贤士录记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// UTC 时间。
    pub at: DateTime<Utc>,
    /// 主体：role 名。
    pub subject: String,
    pub action: LedgerAction,
    /// 原因（reject 时必填，其它可空）。
    pub reason: Option<String>,
    /// 谁批的——玄女 / 用户 / 自动策略。留空即系统触发。
    pub approver: Option<String>,
}

impl LedgerEntry {
    /// 常见构造：subject + action + 可选 reason。
    pub fn new(
        subject: impl Into<String>,
        action: LedgerAction,
        reason: Option<impl Into<String>>,
    ) -> Self {
        Self {
            at: Utc::now(),
            subject: subject.into(),
            action,
            reason: reason.map(Into::into),
            approver: None,
        }
    }

    /// 链式加 approver。
    pub fn approver(mut self, approver: impl Into<String>) -> Self {
        self.approver = Some(approver.into());
        self
    }
}

/// `$HOME/.fuxi/ledger.json`——默认位置。
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".fuxi").join("ledger.json"))
}

/// 追加一条记录到文件。文件不存在时自动创建（含父目录）。
pub fn append(path: &Path, entry: &LedgerEntry) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建贤士录目录 {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("打开贤士录 {}", path.display()))?;
    let line = serde_json::to_string(entry)?;
    // 一个 write_all + '\n'：`O_APPEND` 保证这次 write 对其他追加者不交错。
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

/// 按行读全部记录。文件不存在返回空。
pub fn read_all(path: &Path) -> Result<Vec<LedgerEntry>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = std::fs::File::open(path).with_context(|| format!("读贤士录 {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: LedgerEntry = serde_json::from_str(&line)
            .with_context(|| format!("贤士录第 {} 行解析失败", i + 1))?;
        out.push(entry);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_action_roundtrip() {
        for a in [
            LedgerAction::Staged,
            LedgerAction::Approved,
            LedgerAction::Rejected,
            LedgerAction::Activated,
        ] {
            let s = serde_json::to_string(&a).unwrap();
            let back: LedgerAction = serde_json::from_str(&s).unwrap();
            assert_eq!(a, back);
        }
    }
}

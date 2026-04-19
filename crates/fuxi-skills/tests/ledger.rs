//! 贤士录（ledger.json）测试：append-only、多条并存、回读解析。

use fuxi_skills::{LedgerAction, LedgerEntry, ledger};

#[test]
fn append_and_read_roundtrip_preserves_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");

    let e1 = LedgerEntry::new("painter", LedgerAction::Staged, Some("玄女发起")).approver("xuannv");
    let e2 = LedgerEntry::new("painter", LedgerAction::Approved, Some("用户点头"));
    let e3 = LedgerEntry::new("painter", LedgerAction::Activated, None::<&str>);

    ledger::append(&path, &e1).expect("append 1");
    ledger::append(&path, &e2).expect("append 2");
    ledger::append(&path, &e3).expect("append 3");

    let got = ledger::read_all(&path).expect("read");
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].action, LedgerAction::Staged);
    assert_eq!(got[1].action, LedgerAction::Approved);
    assert_eq!(got[2].action, LedgerAction::Activated);
    assert_eq!(got[0].subject, "painter");
    assert_eq!(got[0].approver.as_deref(), Some("xuannv"));
    assert_eq!(got[1].reason.as_deref(), Some("用户点头"));
}

#[test]
fn append_to_nonexistent_file_creates_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/ledger.json");
    let entry = LedgerEntry::new("dev", LedgerAction::Rejected, Some("格式不对"));
    ledger::append(&path, &entry).expect("append creates file");
    assert!(path.exists());
    let got = ledger::read_all(&path).expect("read");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].action, LedgerAction::Rejected);
}

#[test]
fn read_all_on_missing_file_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("never-written.json");
    let got = ledger::read_all(&path).expect("read empty");
    assert!(got.is_empty());
}

#[test]
fn ledger_lines_are_valid_json_each() {
    // 合约：append-only JSON Lines。每行独立解析。
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    ledger::append(
        &path,
        &LedgerEntry::new("a", LedgerAction::Staged, None::<&str>),
    )
    .unwrap();
    ledger::append(
        &path,
        &LedgerEntry::new("b", LedgerAction::Approved, None::<&str>),
    )
    .unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2);
    for line in lines {
        serde_json::from_str::<serde_json::Value>(line).expect("每行必须是合法 JSON");
    }
}

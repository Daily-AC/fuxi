//! staging 集成测试：榜文 → 玉牒的三态原子操作。

use fuxi_skills::{SkillState, approve, list_all, reject, stage_write};

fn simple_skill(role: &str) -> String {
    format!(
        "---\nname: {role}\ndescription: 新门客\nallowed-tools: Read\n---\n\n# {role} 玉牒\n\n你是 {role}。\n"
    )
}

#[test]
fn stage_write_creates_staging_dir() {
    let dir = tempfile::tempdir().unwrap();
    let path = stage_write(dir.path(), "painter", &simple_skill("painter")).expect("stage");
    assert!(
        path.exists(),
        "staging SKILL.md 应当存在: {}",
        path.display()
    );
    assert!(
        path.ends_with("painter.staging/SKILL.md"),
        "实际路径 = {}",
        path.display()
    );
}

#[test]
fn approve_renames_staging_to_active_atomic() {
    let dir = tempfile::tempdir().unwrap();
    stage_write(dir.path(), "painter", &simple_skill("painter")).expect("stage");
    let active = approve(dir.path(), "painter").expect("approve");

    assert!(active.ends_with("painter/SKILL.md"), "{}", active.display());
    assert!(active.exists());
    // 榜文应该消失
    assert!(
        !dir.path().join("painter.staging").exists(),
        "staging dir 仍在"
    );
}

#[test]
fn reject_removes_staging_without_touching_active() {
    let dir = tempfile::tempdir().unwrap();
    // 先造一个已入册的同名 role——确保 reject 不误伤
    std::fs::create_dir_all(dir.path().join("scribe")).unwrap();
    std::fs::write(dir.path().join("scribe/SKILL.md"), simple_skill("scribe")).unwrap();

    stage_write(dir.path(), "scribe", &simple_skill("scribe")).expect("stage");
    reject(dir.path(), "scribe").expect("reject");

    assert!(!dir.path().join("scribe.staging").exists());
    assert!(dir.path().join("scribe/SKILL.md").exists(), "active 被误删");
}

#[test]
fn list_all_distinguishes_active_and_staging() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("dev")).unwrap();
    std::fs::write(dir.path().join("dev/SKILL.md"), simple_skill("dev")).unwrap();
    stage_write(dir.path(), "painter", &simple_skill("painter")).expect("stage");

    let mut entries = list_all(dir.path()).expect("list");
    entries.sort_by(|a, b| a.role.cmp(&b.role));
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].role, "dev");
    assert_eq!(entries[0].state, SkillState::Active);
    assert_eq!(entries[1].role, "painter");
    assert_eq!(entries[1].state, SkillState::Staging);
}

#[test]
fn approve_fails_without_staging() {
    let dir = tempfile::tempdir().unwrap();
    let err = approve(dir.path(), "ghost").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("ghost"), "实际错误: {msg}");
}

#[test]
fn approve_replaces_existing_active_with_bak() {
    let dir = tempfile::tempdir().unwrap();
    // 已入册
    std::fs::create_dir_all(dir.path().join("scribe")).unwrap();
    std::fs::write(dir.path().join("scribe/SKILL.md"), "旧版本\n").unwrap();
    // 改版入榜
    stage_write(dir.path(), "scribe", "新版本\n").expect("stage");

    approve(dir.path(), "scribe").expect("approve");
    let active_body = std::fs::read_to_string(dir.path().join("scribe/SKILL.md")).unwrap();
    assert!(active_body.contains("新版本"));

    // 旧版本应当被挪到 *.bak 目录（保留一份）
    let has_bak = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with("scribe.bak"));
    assert!(has_bak, "旧版本未留 .bak 备份");
}

//! loader 集成测试：YAML frontmatter（含嵌套 metadata）+ 查找顺序。

use std::io::Write;

#[test]
fn loads_nested_metadata_via_yaml() {
    let dir = tempfile::tempdir().unwrap();
    let role_dir = dir.path().join("dev");
    std::fs::create_dir_all(&role_dir).unwrap();
    let path = role_dir.join("ROLE.md");
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(
        f,
        "---\n\
         name: dev\n\
         description: 代码门客\n\
         license: Proprietary\n\
         compatibility: fuxi worktree only\n\
         metadata:\n  \
           role: dev\n  \
           tier: worker\n  \
           fuxi-version: \"0.1\"\n\
         allowed-tools: Read Edit Bash(git:*)\n\
         ---\n\
         \n\
         你是 dev 门客。"
    )
    .unwrap();
    drop(f);

    let loaded = fuxi_skills::load_from_file(&path, "dev").expect("load");
    let fm = &loaded.frontmatter;
    assert_eq!(fm.name, "dev");
    assert_eq!(fm.description, "代码门客");
    assert_eq!(fm.license.as_deref(), Some("Proprietary"));
    // 关键：嵌套 metadata 能取到
    assert_eq!(
        fm.metadata
            .get("tier")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        Some("worker".to_string())
    );
    assert_eq!(
        fm.metadata
            .get("fuxi-version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        Some("0.1".to_string())
    );
    let tools = loaded.allowed_tools.clone().unwrap();
    assert_eq!(tools, vec!["Read", "Edit", "Bash(git:*)"]);
    assert!(loaded.append_system_prompt.contains("dev 门客"));
}

#[test]
fn missing_frontmatter_still_returns_body() {
    let dir = tempfile::tempdir().unwrap();
    let role_dir = dir.path().join("plain");
    std::fs::create_dir_all(&role_dir).unwrap();
    let path = role_dir.join("ROLE.md");
    std::fs::write(&path, "just plain body without yaml\n").unwrap();

    let loaded = fuxi_skills::load_from_file(&path, "plain").expect("load");
    assert_eq!(loaded.frontmatter.name, "plain"); // 兜底 role_hint
    assert!(
        loaded
            .append_system_prompt
            .contains("just plain body without yaml")
    );
}

#[test]
fn skills_root_honors_env_var() {
    let dir = tempfile::tempdir().unwrap();
    // 放一个占位 role 让目录存在——skills_root 要求目录真实存在
    std::fs::create_dir_all(dir.path().join("_placeholder")).unwrap();

    // SAFETY: 测试进程内独占的环境变量——tempfile 的路径唯一保证互不踩。
    // 注意不并行别的同变量测试（cargo test 默认会，但当前 crate 只此一处）。
    unsafe {
        std::env::set_var("FUXI_SKILLS_DIR", dir.path());
    }
    let got = fuxi_skills::skills_root();
    unsafe {
        std::env::remove_var("FUXI_SKILLS_DIR");
    }
    assert_eq!(got.as_deref(), Some(dir.path()));
}

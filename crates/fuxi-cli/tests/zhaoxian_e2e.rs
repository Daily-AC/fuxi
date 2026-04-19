//! 招贤 E2E（gated by `FUXI_RUN_ZHAOXIAN_E2E=1`）—— 通过 `fuxi skill {stage,list,approve}`
//! 子命令走一遍全流程，不依赖 daemon / cc。
//!
//! 这是"轻量 E2E"：故意避开真起门客（那需要 API key），把招贤的**文件层流水线**
//! 端到端串通。如果未来要起真铸牒司门客，再加一个 FUXI_RUN_ZHAOXIAN_CC_E2E。

use std::path::PathBuf;
use std::process::Command;

const CRATE_ROOT: &str = env!("CARGO_MANIFEST_DIR");
const BIN: &str = env!("CARGO_BIN_EXE_fuxi");

fn should_run() -> bool {
    std::env::var("FUXI_RUN_ZHAOXIAN_E2E").ok().as_deref() == Some("1")
}

/// 把项目根下的 templates/ 路径算出来——tests 在 crates/fuxi-cli 里跑，
/// 项目根是往上两级。
fn project_root() -> PathBuf {
    PathBuf::from(CRATE_ROOT)
        .parent()
        .and_then(|p| p.parent())
        .expect("project root")
        .to_path_buf()
}

fn run_fuxi(
    skills_dir: &std::path::Path,
    templates_dir: &std::path::Path,
    args: &[&str],
) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .env("FUXI_SKILLS_DIR", skills_dir)
        .env("FUXI_TEMPLATES_DIR", templates_dir)
        // daemon 不在跑——IPC 调用会失败，skill 子命令内部已 `let _ =`
        .env("FUXI_SOCK", "/tmp/fuxi-zhaoxian-nonexistent.sock")
        .env("HOME", skills_dir) // 把 ledger.json 写到隔离目录
        .output()
        .expect("run fuxi")
}

#[test]
fn zhaoxian_full_pipeline() {
    if !should_run() {
        eprintln!("skip: 设 FUXI_RUN_ZHAOXIAN_E2E=1 开启");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    std::fs::create_dir_all(&skills).unwrap();
    let templates = project_root().join("templates");
    assert!(
        templates.exists(),
        "项目 templates/ 不在: {}",
        templates.display()
    );

    // 1. 触发需要——模拟玄女发 NoRoleMatched（daemon 不在也不影响，stage 能直接走）
    let brief = "画 SVG / dot 图的门客，输出到 docs/arch/";

    // 2. 铸牒司视角的 stage——CLI 直接填槽写榜文
    let out = run_fuxi(
        &skills,
        &templates,
        &[
            "skill",
            "stage",
            "--role",
            "painter",
            "--template",
            "dev",
            "--brief",
            brief,
            "--tools",
            "Read Write Bash",
        ],
    );
    assert!(
        out.status.success(),
        "stage 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("staging"), "stage 返回异常: {stdout}");

    let staged = skills.join("painter.staging/SKILL.md");
    assert!(staged.exists(), "榜文未写出: {}", staged.display());
    let staged_body = std::fs::read_to_string(&staged).unwrap();
    assert!(staged_body.contains("name: painter"));
    assert!(staged_body.contains(brief), "brief 未填入: {staged_body}");
    assert!(!staged_body.contains("{{"), "占位符残留: {staged_body}");

    // 3. list 应当看到榜文
    let out = run_fuxi(&skills, &templates, &["skill", "list"]);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("painter"), "list 未见 painter: {stdout}");
    assert!(
        stdout.contains("staging"),
        "list 未见 staging 状态: {stdout}"
    );

    // 4. approve —— 模拟用户点头
    let out = run_fuxi(
        &skills,
        &templates,
        &[
            "skill",
            "approve",
            "--role",
            "painter",
            "--approver",
            "user",
        ],
    );
    assert!(
        out.status.success(),
        "approve 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        skills.join("painter/SKILL.md").exists(),
        "active 玉牒未就位"
    );
    assert!(!skills.join("painter.staging").exists(), "staging 未清理");

    // 5. list 再次 —— painter 应当是 active
    let out = run_fuxi(&skills, &templates, &["skill", "list"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("active"), "approve 后仍非 active: {stdout}");

    // 6. 新 role 的 SKILL.md 能被 loader 加载（最终验证"能 spawn"的前置条件）
    let loaded =
        fuxi_skills::load_from_file(&skills.join("painter/SKILL.md"), "painter").expect("loader");
    assert_eq!(loaded.profile.name, "painter");
    assert!(loaded.profile.system_prompt.contains(brief));

    // 7. 贤士录 ledger.json 至少 2 条（staged + approved）
    let ledger = skills.join(".fuxi/ledger.json"); // HOME=skills_dir → $HOME/.fuxi/ledger.json
    assert!(ledger.exists(), "贤士录未生成");
    let ledger_body = std::fs::read_to_string(&ledger).unwrap();
    let lines: Vec<_> = ledger_body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    assert!(lines.len() >= 2, "贤士录行数不足: {lines:?}");
    assert!(
        lines.iter().any(|l| l.contains("staged")),
        "没有 staged 条目"
    );
    assert!(
        lines.iter().any(|l| l.contains("approved")),
        "没有 approved 条目"
    );
}

#[test]
fn zhaoxian_reject_leaves_audit_trail() {
    if !should_run() {
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let skills = tmp.path().join("skills");
    std::fs::create_dir_all(&skills).unwrap();
    let templates = project_root().join("templates");

    run_fuxi(
        &skills,
        &templates,
        &[
            "skill",
            "stage",
            "--role",
            "dubious",
            "--template",
            "research",
            "--brief",
            "一个格式不对的草稿",
        ],
    );

    let out = run_fuxi(
        &skills,
        &templates,
        &[
            "skill",
            "reject",
            "--role",
            "dubious",
            "--reason",
            "frontmatter-bad",
        ],
    );
    assert!(
        out.status.success(),
        "reject 失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!skills.join("dubious.staging").exists(), "staging 未清");

    let ledger = skills.join(".fuxi/ledger.json");
    let body = std::fs::read_to_string(&ledger).unwrap();
    assert!(body.contains("rejected"));
    assert!(body.contains("frontmatter-bad"));
}

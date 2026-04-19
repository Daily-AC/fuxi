//! `fuxi skill {list, stage, approve, reject, activate}` ——招贤工具子命令。
//!
//! 语义：
//! - `list`：纯 FS 读取，不走 daemon——玄女或人类随时查点将台。
//! - `stage`：按 `templates/<template>.archetype.md` 填槽写 `skills/<role>.staging/`。
//!   这是铸牒司通过 `Bash(fuxi:*)` 调的工具**也**是人类的快速入口。
//! - `approve`：staging → active rename + 记贤士录。若 daemon 在跑，再推
//!   `SkillApproved` + `SkillActivated` 事件（best-effort）。
//! - `reject`：删 staging + 记贤士录。
//! - `activate`：单独补发 `SkillActivated` 事件（daemon 必须在跑）。
//!
//! 成功：stdout 一行 JSON；失败：stderr + 非零 exit。

use anyhow::{Context, Result, anyhow};
use clap::{Args as ClapArgs, Subcommand};
use fuxi_skills::template::{RenderArgs, render};
use fuxi_skills::{LedgerAction, LedgerEntry, SkillState, ledger, skills_root, staging};
use std::path::{Path, PathBuf};

#[derive(Debug, ClapArgs)]
pub struct SkillArgs {
    #[command(subcommand)]
    pub cmd: SkillCmd,
}

#[derive(Debug, Subcommand)]
pub enum SkillCmd {
    /// 列出点将台上的玉牒（active）和榜文（staging）。
    List,
    /// 按模板造一枚榜文，暂挂 `skills/<role>.staging/`。
    Stage(StageArgs),
    /// 榜文入册——rename 到 `skills/<role>/`，写贤士录。
    Approve(ApproveArgs),
    /// 驳回榜文——删 staging，写贤士录。
    Reject(RejectArgs),
    /// 发 `SkillActivated` 事件（需 daemon 在跑）。
    Activate(ActivateArgs),
}

#[derive(Debug, ClapArgs)]
pub struct StageArgs {
    /// 新 role 名（ASCII lowercase / hyphen）。
    #[arg(long)]
    pub role: String,
    /// archetype 模板名：查 `templates/<template>.archetype.md`。
    #[arg(long)]
    pub template: String,
    /// 一句话描述这个 role 要干嘛——填到 `description`。
    #[arg(long)]
    pub brief: String,
    /// soul 一句——可选。不给时 brief 当兜底。
    #[arg(long)]
    pub soul: Option<String>,
    /// allowed-tools 列表（空格分隔），例 `"Read Write Bash"`。
    #[arg(long, default_value = "Read Write Bash")]
    pub tools: String,
}

#[derive(Debug, ClapArgs)]
pub struct ApproveArgs {
    #[arg(long)]
    pub role: String,
    /// 谁批的——默认"user"。
    #[arg(long, default_value = "user")]
    pub approver: String,
}

#[derive(Debug, ClapArgs)]
pub struct RejectArgs {
    #[arg(long)]
    pub role: String,
    #[arg(long)]
    pub reason: String,
}

#[derive(Debug, ClapArgs)]
pub struct ActivateArgs {
    #[arg(long)]
    pub role: String,
}

pub async fn run(args: SkillArgs) -> Result<()> {
    match args.cmd {
        SkillCmd::List => run_list().await,
        SkillCmd::Stage(a) => run_stage(a).await,
        SkillCmd::Approve(a) => run_approve(a).await,
        SkillCmd::Reject(a) => run_reject(a).await,
        SkillCmd::Activate(a) => run_activate(a).await,
    }
}

async fn run_list() -> Result<()> {
    let root = resolve_skills_root()?;
    let entries = staging::list_all(&root)?;
    let payload: Vec<_> = entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "role": e.role,
                "state": match e.state { SkillState::Active => "active", SkillState::Staging => "staging" },
                "path": e.path.display().to_string(),
            })
        })
        .collect();
    println!("{}", serde_json::to_string(&payload)?);
    Ok(())
}

async fn run_stage(args: StageArgs) -> Result<()> {
    let root = resolve_skills_root()?;
    let templates_dir = templates_root(&root)
        .context("找不到 templates/ 目录（与 skills 同级查，或设 FUXI_TEMPLATES_DIR）")?;
    let tpl_path = templates_dir.join(format!("{}.archetype.md", args.template));
    let raw = std::fs::read_to_string(&tpl_path)
        .with_context(|| format!("读取模板 {}", tpl_path.display()))?;

    let soul = args.soul.as_deref().unwrap_or(&args.brief);
    let generated_at = chrono::Utc::now().to_rfc3339();
    let body = render(
        &raw,
        &RenderArgs {
            name: &args.role,
            description: &args.brief,
            soul,
            allowed_tools: &args.tools,
            generated_at: &generated_at,
        },
    )?;

    let staged_path = staging::stage_write(&root, &args.role, &body)?;

    // 写贤士录——staging 是可审计起点。
    if let Some(ledger_path) = ledger::default_path() {
        let entry = LedgerEntry::new(
            args.role.clone(),
            LedgerAction::Staged,
            Some(format!("template={}", args.template)),
        )
        .approver("zhudiesi");
        let _ = ledger::append(&ledger_path, &entry);
    }

    // 尝试推 `SkillStaged` 事件（daemon 未跑则默默跳过）。
    let _ = try_emit_skill_staged(&args.role, &args.template, &staged_path).await;

    let out = serde_json::json!({
        "role": args.role,
        "template": args.template,
        "staging": staged_path.display().to_string(),
    });
    println!("{}", out);
    Ok(())
}

async fn run_approve(args: ApproveArgs) -> Result<()> {
    let root = resolve_skills_root()?;
    let active_path = staging::approve(&root, &args.role)?;

    if let Some(ledger_path) = ledger::default_path() {
        let entry = LedgerEntry::new(args.role.clone(), LedgerAction::Approved, None::<&str>)
            .approver(args.approver.clone());
        let _ = ledger::append(&ledger_path, &entry);
    }

    let _ = try_emit_skill_approved(&args.role).await;

    let out = serde_json::json!({
        "role": args.role,
        "active": active_path.display().to_string(),
    });
    println!("{}", out);
    Ok(())
}

async fn run_reject(args: RejectArgs) -> Result<()> {
    let root = resolve_skills_root()?;
    staging::reject(&root, &args.role)?;

    if let Some(ledger_path) = ledger::default_path() {
        let entry = LedgerEntry::new(
            args.role.clone(),
            LedgerAction::Rejected,
            Some(args.reason.clone()),
        );
        let _ = ledger::append(&ledger_path, &entry);
    }

    let _ = try_emit_skill_rejected(&args.role, &args.reason).await;

    let out = serde_json::json!({ "role": args.role, "rejected": true });
    println!("{}", out);
    Ok(())
}

async fn run_activate(args: ActivateArgs) -> Result<()> {
    // activate = 只发事件，不碰文件——提示订阅者"这个 role 现在可用"。
    try_emit_skill_activated(&args.role)
        .await
        .context("activate 需要 daemon 在跑（fuxi up）")?;

    if let Some(ledger_path) = ledger::default_path() {
        let entry = LedgerEntry::new(args.role.clone(), LedgerAction::Activated, None::<&str>);
        let _ = ledger::append(&ledger_path, &entry);
    }

    let out = serde_json::json!({ "role": args.role, "activated": true });
    println!("{}", out);
    Ok(())
}

fn resolve_skills_root() -> Result<PathBuf> {
    skills_root().ok_or_else(|| {
        anyhow!(
            "找不到 skills 目录：$FUXI_SKILLS_DIR / git-root/skills / ./skills / ~/.fuxi/skills 都不在"
        )
    })
}

/// 在 skills 同级寻找 `templates/`。也支持 `FUXI_TEMPLATES_DIR` 覆盖。
fn templates_root(skills: &Path) -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FUXI_TEMPLATES_DIR") {
        let pp = PathBuf::from(p);
        if pp.exists() {
            return Some(pp);
        }
    }
    if let Some(parent) = skills.parent() {
        let p = parent.join("templates");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

// ── 事件抄送 ──
//
// daemon 未跑时静默失败（best-effort）。这是"CLI 可以脱机用"的约束。

async fn try_emit_skill_staged(role: &str, template: &str, path: &Path) -> Result<()> {
    let resp = crate::client::send(crate::ipc::Command::EmitEvent {
        kind: crate::ipc::EventKindPayload::SkillStaged {
            role: role.to_string(),
            template: template.to_string(),
            path: path.display().to_string(),
        },
    })
    .await?;
    matches!(resp, crate::ipc::Response::Ok { .. })
        .then_some(())
        .ok_or_else(|| anyhow!("daemon 拒绝了 SkillStaged"))
}

async fn try_emit_skill_approved(role: &str) -> Result<()> {
    let resp = crate::client::send(crate::ipc::Command::EmitEvent {
        kind: crate::ipc::EventKindPayload::SkillApproved {
            role: role.to_string(),
        },
    })
    .await?;
    matches!(resp, crate::ipc::Response::Ok { .. })
        .then_some(())
        .ok_or_else(|| anyhow!("daemon 拒绝了 SkillApproved"))
}

async fn try_emit_skill_rejected(role: &str, reason: &str) -> Result<()> {
    let resp = crate::client::send(crate::ipc::Command::EmitEvent {
        kind: crate::ipc::EventKindPayload::SkillRejected {
            role: role.to_string(),
            reason: reason.to_string(),
        },
    })
    .await?;
    matches!(resp, crate::ipc::Response::Ok { .. })
        .then_some(())
        .ok_or_else(|| anyhow!("daemon 拒绝了 SkillRejected"))
}

async fn try_emit_skill_activated(role: &str) -> Result<()> {
    let resp = crate::client::send(crate::ipc::Command::EmitEvent {
        kind: crate::ipc::EventKindPayload::SkillActivated {
            role: role.to_string(),
        },
    })
    .await?;
    matches!(resp, crate::ipc::Response::Ok { .. })
        .then_some(())
        .ok_or_else(|| anyhow!("daemon 拒绝了 SkillActivated"))
}

//! `fuxi project` 子命令家族（Decision 21 phase 1）。
//!
//! 子命令：
//! - `fuxi project add <canonical-path> [--name <slug>] [--branch <name>]`
//! - `fuxi project list`
//! - `fuxi project rm <id>`
//!
//! 走 `FileSystemProjectRegistry`，root 默认 `$HOME/.fuxi/projects/`，
//! `--registry-root` 覆写（测试 / 替代部署用）。
//!
//! 输出：人类可读纯文本（不上 banner / 颜色）——这是工具型命令，PWA 后续
//! 会有自己的 GUI 注册流。

use anyhow::{Context, Result, anyhow};
use clap::Args;
use fuxi_core::{DeliverableKind, Event, EventKind, EventMeta, ProjectId, TaskId};
use fuxi_events::EventStore;
use fuxi_workspace::{
    DeliverablesManager, EphemeralWorkspaceManager, FileSystemProjectRegistry,
    PersistentSandboxManager,
};
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Debug, Args)]
pub struct ProjectAddArgs {
    /// 用户真项目的绝对或相对路径（必须是已存在的 git repo）。
    pub canonical_path: PathBuf,
    /// 项目 slug。不传从 canonical 末段 basename 派生；非 ASCII basename
    /// 派生失败时必须显式传。
    #[arg(long)]
    pub name: Option<String>,
    /// 默认基线 branch。不传默认 `main`。
    #[arg(long)]
    pub branch: Option<String>,
    /// 注册表 root 覆写。默认 `$HOME/.fuxi/projects/`。测试 / 多账号时用。
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ProjectListArgs {
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
}

/// `fuxi project join`——v2 跨节点 sandbox：worker 端加入 home 已注册的 project。
///
/// 流程：
/// 1. 查 home `GET /api/projects/<slug>` 拿 canonical_path / default_branch
/// 2. `git clone <remote-url> <target>`（git 自带 ssh / https 支持）
/// 3. 本地 `registry.add(target, slug, branch)` 登记
/// 4. POST `/api/projects/<slug>/host_nodes` 通告自己——home 端 dispatch 即可路由
///
/// `--remote-url` 当前必传：mac/其他 worker 没法猜 home repo 的可达路径。
/// 简单形态如 `ssh://home/home/e0-7/demo-site`（`home` 是 ssh config 里的 alias，
/// `/home/e0-7/demo-site` 是 home 端的 canonical_path）。
#[derive(Debug, Args)]
pub struct ProjectJoinArgs {
    /// home 端登记的 project slug。
    #[arg(long)]
    pub slug: String,
    /// home 端 IM controller 基址，如 `https://im.qmledmq.cn:8443`。
    #[arg(long)]
    pub controller: String,
    /// HMAC token——本机签出（`fuxi im issue-token`）或 home 端发的长期 token。
    /// 兼容 PWA cookie 鉴权机制（middleware::cookie_auth_layer）。
    #[arg(long)]
    pub token: String,
    /// 本机 clone 目标路径（绝对 / 相对均可）。
    /// 不传时从 canonical_path 的 basename 派生 + 落 `~`（如 home 上是
    /// `/home/e0-7/demo-site` → 本机落 `~/demo-site`）。
    #[arg(long)]
    pub target: Option<PathBuf>,
    /// home repo 的 git URL——必须可被本机 git 访问。例如 `ssh://home/home/e0-7/demo-site`。
    #[arg(long = "remote-url")]
    pub remote_url: String,
    /// 本机的 dist 节点 id——同 worker register 时声明的 node_id。
    /// 不传走 `$FUXI_NODE_ID` env 或 `hostname`。
    #[arg(long = "node-id")]
    pub node_id: Option<String>,
    /// 注册表 root 覆写（默认 `$HOME/.fuxi/projects/`）。
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ProjectRemoveArgs {
    /// 要删的 project id（slug）。
    pub id: String,
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
}

fn registry_for(root: Option<PathBuf>) -> Result<FileSystemProjectRegistry> {
    match root {
        Some(r) => Ok(FileSystemProjectRegistry::new(r)),
        None => {
            FileSystemProjectRegistry::with_default_root().context("无法构造默认 ProjectRegistry")
        }
    }
}

pub async fn run_add(args: ProjectAddArgs) -> Result<()> {
    let registry = registry_for(args.registry_root)?;
    let project = registry
        .add(args.canonical_path, args.name, args.branch)
        .await
        .context("project add 失败")?;
    println!(
        "已注册 project {} → {}",
        project.id,
        project.canonical_path.display()
    );
    println!("  default branch: {}", project.default_branch);
    println!(
        "  落盘: {}/{}/meta.json",
        registry.root().display(),
        project.id
    );
    Ok(())
}

/// 给 `fuxi project join` 用：从 home canonical_path 派生本机 target 路径。
///
/// 规则：取末段 basename（注意 trailing slash 也兼容），落到 `$HOME` 下。
/// 跟 home canonical_path 同 basename 但路径**不**一致，是有意为之——home 用
/// `/home/e0-7/foo`，mac 用 `/Users/zyl/foo`，两边 canonical_path 不同没关系，
/// dispatch 层只用 project_id 路由，sandbox 起在各自 canonical_path/.fuxi/...。
pub(crate) fn derive_join_target(home_canonical: &Path) -> Option<PathBuf> {
    let base = home_canonical.file_name()?.to_str()?;
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(base))
}

/// `$FUXI_NODE_ID` → `hostname` → fail。worker register 用同样的 fallback 链，
/// 这里复用确保两条路径用同一 node_id。
pub(crate) fn resolve_self_node_id(explicit: Option<String>) -> Result<String> {
    if let Some(s) = explicit
        && !s.trim().is_empty()
    {
        return Ok(s);
    }
    if let Ok(s) = std::env::var("FUXI_NODE_ID")
        && !s.trim().is_empty()
    {
        return Ok(s);
    }
    let out = std::process::Command::new("hostname")
        .output()
        .context("调 hostname 命令失败")?;
    if !out.status.success() {
        return Err(anyhow!(
            "hostname 命令非 0 退出: {:?}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let s = String::from_utf8(out.stdout)
        .context("hostname 输出非 UTF-8")?
        .trim()
        .to_string();
    if s.is_empty() {
        Err(anyhow!("hostname 返回空——请用 --node-id 显式指定"))
    } else {
        // 同 worker register 端：lowercase + 强制小写 ascii，避免 dispatch 路由
        // 时大小写不匹配（"MyMac" vs "mymac"）。
        Ok(s.to_ascii_lowercase())
    }
}

pub async fn run_join(args: ProjectJoinArgs) -> Result<()> {
    use reqwest::Client;
    use serde_json::json;

    let slug = ProjectId::new(args.slug.clone()).context("非法 project slug")?;
    let controller_base = args.controller.trim_end_matches('/').to_string();

    // 1. 查 home /api/projects/<slug>
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("build http client")?;
    let url = format!("{controller_base}/api/projects/{slug}");
    let resp = client
        .get(&url)
        .header("authorization", format!("Bearer {}", args.token))
        .send()
        .await
        .with_context(|| format!("GET {url} 失败"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "GET {url} → {status}: {body}\n\
             检查：1) controller 地址 + token 是否正确  \
             2) home 端是否已 `fuxi project add` 该 slug",
        ));
    }
    let view: serde_json::Value = resp
        .json()
        .await
        .context("home /api/projects 响应解析失败")?;
    let home_canonical = view
        .get("canonical_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("响应缺 canonical_path: {view:?}"))?;
    let default_branch = view
        .get("default_branch")
        .and_then(|v| v.as_str())
        .unwrap_or("main")
        .to_string();

    // 2. 决定 target 路径
    let target = match args.target {
        Some(p) => p,
        None => derive_join_target(Path::new(home_canonical)).ok_or_else(|| {
            anyhow!(
                "无法从 home canonical_path {home_canonical:?} 派生 target——\
                 请显式 --target <path>"
            )
        })?,
    };
    if target.exists() {
        // 已存在：跳过 clone（认为用户已经 clone 过了）
        println!(
            "  target {} 已存在 → 跳过 git clone（假定已是 home 项目的 clone）",
            target.display()
        );
    } else {
        println!("  git clone {} {} ...", args.remote_url, target.display());
        let out = tokio::process::Command::new("git")
            .args([
                "clone",
                "-q",
                "--branch",
                &default_branch,
                args.remote_url.as_str(),
                target.to_string_lossy().as_ref(),
            ])
            .output()
            .await
            .context("起 git clone 失败")?;
        if !out.status.success() {
            return Err(anyhow!(
                "git clone 失败: {}\n  stderr: {}",
                args.remote_url,
                String::from_utf8_lossy(&out.stderr).trim(),
            ));
        }
    }

    // 3. 本地 registry add
    let registry = registry_for(args.registry_root)?;
    // 已存在同 slug 时也算成功——可能用户重跑 `join`；只输出一行
    let canonical_real = target
        .canonicalize()
        .with_context(|| format!("canonicalize {} 失败", target.display()))?;
    if let Some(existing) = registry.get(&slug).await? {
        if existing.canonical_path != canonical_real {
            return Err(anyhow!(
                "本机已有 project {} 但 canonical_path 不一致：\n  已注册: {}\n  本次: {}\n\
                 请先 `fuxi project rm {}` 再 join，或换一个 --target",
                slug,
                existing.canonical_path.display(),
                canonical_real.display(),
                slug,
            ));
        }
        println!(
            "  本机已注册 project {} → {} ：跳过 registry.add",
            slug,
            existing.canonical_path.display()
        );
    } else {
        registry
            .add(
                target.clone(),
                Some(slug.to_string()),
                Some(default_branch.clone()),
            )
            .await
            .context("本机 registry.add 失败")?;
    }

    // 4. POST /api/projects/<slug>/host_nodes
    let node_id = resolve_self_node_id(args.node_id)?;
    let url = format!("{controller_base}/api/projects/{slug}/host_nodes");
    let resp = client
        .post(&url)
        .header("authorization", format!("Bearer {}", args.token))
        .header("content-type", "application/json")
        .body(json!({"node_id": node_id}).to_string())
        .send()
        .await
        .with_context(|| format!("POST {url} 失败"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("POST {url} → {status}: {body}"));
    }

    println!(
        "已 join project {} → 本机 {} ；已通告 home host_node={}",
        slug,
        canonical_real.display(),
        node_id
    );
    println!("  默认 branch: {default_branch}");
    println!("  下一步：home 端 `fuxi dispatch ... --project {slug}`，玄女会按节点负载自动路由。");
    Ok(())
}

pub async fn run_list(args: ProjectListArgs) -> Result<()> {
    let registry = registry_for(args.registry_root)?;
    let projects = registry.list().await.context("project list 失败")?;
    if projects.is_empty() {
        println!("（暂无注册 project；用 `fuxi project add <path>` 注册）");
        return Ok(());
    }
    // 简单两栏对齐：id 和 canonical_path
    let max_id_len = projects
        .iter()
        .map(|p| p.id.as_str().len())
        .max()
        .unwrap_or(0);
    for p in projects {
        println!(
            "  {:width$}  {}",
            p.id,
            p.canonical_path.display(),
            width = max_id_len
        );
    }
    Ok(())
}

/// `fuxi deliverable produce` —— 手动让某 project 的某 task 落一组文件作 deliverable。
///
/// v1 用途：跑测、手工验证 PWA 收件箱、模拟门客交付（agent hook 还没接通时
/// 让用户能端到端走通）。production agent 集成后，门客自己调 produce_deliverable
/// API 不走这个 CLI。
#[derive(Debug, Args)]
pub struct DeliverableProduceArgs {
    /// 项目 slug（必须已 `fuxi project add` 注册过）。
    #[arg(long)]
    pub project: String,
    /// 任务 id。`task-<uuid>` 或裸 uuid。不传走随机新生成（适合纯手工测）。
    #[arg(long)]
    pub task: Option<String>,
    /// deliverable 类型——Decision 13 五类：
    /// research_summary / code_change / test_result / decision_request / error_block。
    #[arg(long, default_value = "research_summary")]
    pub kind: String,
    /// 要交付的文件路径列表（必须是已存在的文件）。
    #[arg(required = true)]
    pub files: Vec<PathBuf>,
    /// 注册表 root 覆写（同 project add）。
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
    /// EventBus SQLite 文件路径覆写。默认 `$HOME/.fuxi/events.db`——跟
    /// `fuxi im start` 的默认路径一致；指定则两进程共享同一文件，DeliverableProduced
    /// 事件可被 firehose TUI / IM 后端 replay 看到。
    /// 缺失（路径不存在或文件不可写）→ warn 但不致命，仍正常 produce 文件。
    #[arg(long = "events-db")]
    pub events_db: Option<PathBuf>,
}

pub async fn run_deliverable_produce(args: DeliverableProduceArgs) -> Result<()> {
    let registry = registry_for(args.registry_root.clone())?;
    let project_id = ProjectId::new(args.project.clone())
        .map_err(|e| anyhow::anyhow!("无效 project id {}: {e}", args.project))?;
    let project = registry
        .get(&project_id)
        .await
        .context("project lookup 失败")?
        .ok_or_else(|| anyhow::anyhow!("project {project_id} 未注册——先跑 fuxi project add"))?;

    let task = parse_task_id(args.task.as_deref())?;
    let kind = parse_deliverable_kind(&args.kind)?;

    // canonicalize sources：相对路径转绝对，文件不存在或不是文件直接报错
    let mut sources = Vec::with_capacity(args.files.len());
    for f in &args.files {
        let canon = f
            .canonicalize()
            .with_context(|| format!("文件无法解析: {}", f.display()))?;
        if !canon.is_file() {
            anyhow::bail!("源不是文件: {}", canon.display());
        }
        sources.push(canon);
    }

    // 落地用 registry root（同样 root 下 deliverables/ 跟 projects/ 同级）
    let mgr = DeliverablesManager::new(project.id.clone(), registry.root());
    let handle = mgr
        .produce(task, kind, &sources)
        .await
        .context("produce_deliverable 失败")?;

    println!("已交付 deliverable");
    println!("  project: {}", handle.project);
    println!("  task: {}", handle.task);
    println!("  kind: {:?}", handle.kind);
    println!("  bucket: {}", handle.bucket_path.display());
    println!("  files:");
    for f in &handle.files {
        println!(
            "    {} ({} bytes, sha256={})",
            f.name,
            f.size_bytes,
            &f.sha256[..16]
        );
    }

    // 发 DeliverableProduced 事件——直写共享 events.db 让 fuxi-im / firehose 能看到。
    // 失败 (events.db 路径不通 / WAL 撞锁) → warn 不致命，本次 produce 已落盘有效。
    let events_db = args.events_db.or_else(default_events_db_path);
    match events_db {
        Some(path) if path.exists() => {
            if let Err(e) = publish_deliverable_produced(&path, &handle).await {
                eprintln!(
                    "⚠ DeliverableProduced 事件未写入 ({path}): {e}",
                    path = path.display()
                );
            } else {
                println!("  事件: DeliverableProduced 已发到 {}", path.display());
            }
        }
        _ => {
            eprintln!(
                "⚠ events.db 不存在或未指定——DeliverableProduced 事件未发；\
                 跑 fuxi im start 后再 produce 才能看到事件流"
            );
        }
    }
    Ok(())
}

/// 默认 events.db 路径——跟 `fuxi im start` 同源（`$HOME/.fuxi/events.db`）。
fn default_events_db_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".fuxi").join("events.db"))
}

async fn publish_deliverable_produced(
    db_path: &std::path::Path,
    handle: &fuxi_workspace::DeliverableHandle,
) -> anyhow::Result<()> {
    let store = EventStore::connect_file(db_path)
        .await
        .with_context(|| format!("connect events.db {} 失败", db_path.display()))?;
    let mut meta = EventMeta::now();
    meta.task = Some(handle.task);
    let ev = Event {
        meta,
        kind: EventKind::DeliverableProduced {
            task: handle.task,
            project: handle.project.clone(),
            deliverable_kind: handle.kind,
            files: handle.files.clone(),
        },
    };
    store.append(&ev).await.context("append 事件失败")?;

    // 顺带探测：若 cwd 在某 L3 sandbox 内（cwd 包含
    // `<root>/projects/<project>/sandboxes/<role>/`），发一条 WorkspaceMutated。
    // 让 firehose / IM 看到「sandbox 又被使用了」的活信号——没探到就 silent
    // skip（CLI 也可能从普通 worktree 跑）。
    if let Some((workspace_id, role)) = detect_l3_sandbox_from_cwd(&handle.project).await {
        let mut meta = EventMeta::now();
        meta.task = Some(handle.task);
        let _ = role; // role 字段已编入 workspace_id；保留作 debug
        let ev = Event {
            meta,
            kind: EventKind::WorkspaceMutated {
                workspace_id,
                files_changed: handle.files.len() as u32,
            },
        };
        // 同 store 已 connect 复用——一次性 produce 两条事件
        let _ = store.append(&ev).await; // 失败 silent skip，不影响 produce 结果
    }

    Ok(())
}

/// 试图从当前 cwd 反查「这个 produce 是从哪个 L3 sandbox 跑的」。
///
/// 启发式：cwd 的祖先路径里若含 `sandboxes/<role>` 段且其上一级 dir 名匹配
/// `<project>/sandboxes`（project 跟传入的 expected_project 一致），就认为
/// 在该 sandbox 内。否则返 None。
///
/// 返回 (WorkspaceId, role)。
async fn detect_l3_sandbox_from_cwd(
    expected_project: &fuxi_core::ProjectId,
) -> Option<(fuxi_core::WorkspaceId, String)> {
    let cwd = std::env::current_dir().ok()?;
    let canon = cwd.canonicalize().unwrap_or(cwd);
    // 走祖先链：找到第一个 parent.parent.file_name() == "sandboxes" 的位置，
    // 那个 dir 名 = role。再往上一层应该是 `<project_id>` (与 expected 比对)。
    for dir in canon.ancestors() {
        let role = match dir.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let parent = match dir.parent() {
            Some(p) => p,
            None => continue,
        };
        if parent.file_name().and_then(|n| n.to_str()) != Some("sandboxes") {
            continue;
        }
        let project_dir = parent.parent()?;
        let project_name = project_dir.file_name().and_then(|n| n.to_str())?;
        if project_name == expected_project.as_str() {
            return Some((fuxi_core::WorkspaceId::l3(expected_project, &role), role));
        }
    }
    None
}

fn parse_task_id(s: Option<&str>) -> Result<TaskId> {
    match s {
        None => Ok(TaskId::new()),
        Some(raw) => {
            let trimmed = raw.strip_prefix("task-").unwrap_or(raw);
            uuid::Uuid::from_str(trimmed)
                .map(TaskId::from)
                .map_err(|e| anyhow::anyhow!("无效 task id {raw}: {e}"))
        }
    }
}

fn parse_deliverable_kind(s: &str) -> Result<DeliverableKind> {
    match s {
        "research_summary" => Ok(DeliverableKind::ResearchSummary),
        "code_change" => Ok(DeliverableKind::CodeChange),
        "test_result" => Ok(DeliverableKind::TestResult),
        "decision_request" => Ok(DeliverableKind::DecisionRequest),
        "error_block" => Ok(DeliverableKind::ErrorBlock),
        other => anyhow::bail!(
            "未知 deliverable kind: {other}（可选：research_summary / code_change / test_result / decision_request / error_block）"
        ),
    }
}

/// `fuxi project info <id>` —— 一屏显示项目元信息 + sandbox 数 + 交付数。
#[derive(Debug, Args)]
pub struct ProjectInfoArgs {
    pub id: String,
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
}

pub async fn run_info(args: ProjectInfoArgs) -> Result<()> {
    let registry = registry_for(args.registry_root)?;
    let project_id = ProjectId::new(args.id.clone())
        .map_err(|e| anyhow::anyhow!("无效 project id {}: {e}", args.id))?;
    let project = registry
        .get(&project_id)
        .await
        .context("project lookup 失败")?
        .ok_or_else(|| anyhow::anyhow!("project {project_id} 未注册"))?;

    println!("project: {}", project.id);
    println!("  canonical:      {}", project.canonical_path.display());
    println!("  default branch: {}", project.default_branch);
    println!(
        "  registered at:  {}",
        project.created_at.format("%Y-%m-%d %H:%M:%S")
    );

    // sandbox 数
    let sandbox_mgr = PersistentSandboxManager::new(project.clone(), registry.root());
    let sandboxes = sandbox_mgr.list().await.context("list sandboxes 失败")?;
    println!("\nL3 持久 sandboxes ({}): ", sandboxes.len());
    if sandboxes.is_empty() {
        println!(
            "  （无；起一个：fuxi spawn --role <role> --project {}）",
            project.id
        );
    } else {
        for h in &sandboxes {
            println!("  - {}  branch: {}", h.role, h.branch);
        }
    }

    // 交付 task 数（扫 deliverables/ 子目录）
    let deliverables_dir = registry
        .root()
        .join(project.id.as_str())
        .join("deliverables");
    let mut task_count = 0_usize;
    if deliverables_dir.exists() {
        let mut iter = tokio::fs::read_dir(&deliverables_dir).await?;
        while let Some(entry) = iter.next_entry().await? {
            if entry.path().join("manifest.json").exists() {
                task_count += 1;
            }
        }
    }
    println!("\n交付 (task buckets): {}", task_count);
    if task_count == 0 {
        println!("  （无；门客 produce 后会出现在 PWA「交付」tab）");
    }
    Ok(())
}

/// `fuxi sandbox sweep [--project <slug>] [--threshold-hours <n>]` —— 扫归档区
/// 删过期的 L2 ephemeral worktree（Decision 21 phase 2 GC）。
///
/// 不传 `--project` → 扫所有已注册项目；不传 `--threshold-hours` → 默认 24h。
#[derive(Debug, Args)]
pub struct SandboxSweepArgs {
    /// 限定扫某项目；不传扫所有。
    #[arg(long)]
    pub project: Option<String>,
    /// 归档过期阈值（小时），默认 24。
    #[arg(long = "threshold-hours", default_value_t = 24_u64)]
    pub threshold_hours: u64,
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
}

pub async fn run_sandbox_sweep(args: SandboxSweepArgs) -> Result<()> {
    let registry = registry_for(args.registry_root)?;
    let projects = match args.project {
        Some(slug) => {
            let id = ProjectId::new(slug.clone())
                .map_err(|e| anyhow::anyhow!("无效 slug {slug}: {e}"))?;
            let p = registry
                .get(&id)
                .await
                .context("project lookup 失败")?
                .ok_or_else(|| anyhow::anyhow!("project {id} 未注册"))?;
            vec![p]
        }
        None => registry.list().await.context("project list 失败")?,
    };

    let threshold = chrono::Duration::hours(args.threshold_hours as i64);
    let mut total = 0_usize;
    // 同 deliverable produce 路径：CLI 跨进程没法走 EventBus.publish，直接
    // append events.db 让 fuxi-im / firehose 能 replay 看到 WorkspaceCollected。
    let events_db = default_events_db_path();
    let store = match &events_db {
        Some(p) if p.exists() => EventStore::connect_file(p).await.ok(),
        _ => None,
    };
    for project in projects {
        let mgr = EphemeralWorkspaceManager::new(project.clone(), registry.root());
        let collected = mgr
            .collect_expired(threshold)
            .await
            .with_context(|| format!("sweep {} 失败", project.id))?;
        if collected.is_empty() {
            continue;
        }
        println!(
            "project {}: 清掉 {} 条过期归档",
            project.id,
            collected.len()
        );
        for (task, meta) in &collected {
            println!(
                "  - task-{} archived_at={} branch={}",
                task.0, meta.archived_at, meta.branch
            );
            if let Some(s) = &store {
                let workspace_id = fuxi_core::WorkspaceId::l2(&project.id, *task);
                let mut ev_meta = EventMeta::now();
                ev_meta.task = Some(*task);
                let ev = Event {
                    meta: ev_meta,
                    kind: EventKind::WorkspaceCollected { workspace_id },
                };
                if let Err(e) = s.append(&ev).await {
                    eprintln!("⚠ WorkspaceCollected 事件未写: {e}");
                }
            }
        }
        total += collected.len();
    }
    if total == 0 {
        println!("（没什么可清；过期阈值 = {} 小时）", args.threshold_hours);
    } else {
        println!("\n共清掉 {} 条 L2 archived workspace", total);
        if events_db.as_ref().is_some_and(|p| p.exists()) {
            println!("  WorkspaceCollected 事件已发到 events.db ({} 条)", total);
        }
    }
    Ok(())
}

/// `fuxi sandbox list --project <slug>` —— 列项目的 L3 持久 sandbox。
#[derive(Debug, Args)]
pub struct SandboxListArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
}

/// `fuxi sandbox retire --project <slug> --role <role>` —— 删项目下某 role 的
/// L3 sandbox（**destructive**：未 commit 的 WIP 一并丢）。
#[derive(Debug, Args)]
pub struct SandboxRetireArgs {
    #[arg(long)]
    pub project: String,
    #[arg(long)]
    pub role: String,
    #[arg(long = "registry-root")]
    pub registry_root: Option<PathBuf>,
}

pub async fn run_sandbox_list(args: SandboxListArgs) -> Result<()> {
    let registry = registry_for(args.registry_root)?;
    let project_id = ProjectId::new(args.project.clone())
        .map_err(|e| anyhow::anyhow!("无效 project id {}: {e}", args.project))?;
    let project = registry
        .get(&project_id)
        .await
        .context("project lookup 失败")?
        .ok_or_else(|| anyhow::anyhow!("project {project_id} 未注册"))?;
    let mgr = PersistentSandboxManager::new(project.clone(), registry.root());
    let handles = mgr.list().await.context("list sandboxes 失败")?;
    if handles.is_empty() {
        println!(
            "（{} 暂无 sandbox；起一个：fuxi spawn --role <role> --project {}）",
            project.id, project.id
        );
        return Ok(());
    }
    let max_role = handles.iter().map(|h| h.role.len()).max().unwrap_or(0);
    for h in handles {
        println!(
            "  {:width$}  {}  →  {}",
            h.role,
            h.branch,
            h.sandbox_path.display(),
            width = max_role
        );
    }
    Ok(())
}

pub async fn run_sandbox_retire(args: SandboxRetireArgs) -> Result<()> {
    let registry = registry_for(args.registry_root)?;
    let project_id = ProjectId::new(args.project.clone())
        .map_err(|e| anyhow::anyhow!("无效 project id {}: {e}", args.project))?;
    let project = registry
        .get(&project_id)
        .await
        .context("project lookup 失败")?
        .ok_or_else(|| anyhow::anyhow!("project {project_id} 未注册"))?;
    let mgr = PersistentSandboxManager::new(project.clone(), registry.root());
    mgr.retire(&args.role)
        .await
        .with_context(|| format!("retire {} sandbox 失败", args.role))?;
    println!("已 retire {}/{} sandbox", project.id, args.role);
    println!("  注意：未 commit 的 WIP 已丢（destructive 操作）");
    Ok(())
}

pub async fn run_remove(args: ProjectRemoveArgs) -> Result<()> {
    let registry = registry_for(args.registry_root)?;
    let id = ProjectId::new(args.id.clone())
        .map_err(|e| anyhow::anyhow!("无效 project id {}: {e}", args.id))?;
    // 先 get 一下：不存在直接告诉用户，避免静默 noop 误以为删了
    if registry.get(&id).await?.is_none() {
        anyhow::bail!("project {id} 不存在");
    }
    registry.remove(&id).await.context("project remove 失败")?;
    println!("已删 project {id}");
    println!("  注意：sandboxes / ephemeral / archive / deliverables 子目录一并清掉");
    Ok(())
}

#[cfg(test)]
mod tests {
    //! e2e 走完整 add → list → remove 路径，验证 CLI 跟 registry 接通。
    //! 不验 stdout 文本格式（那是用户体验，不是契约）。

    use super::*;
    use tempfile::TempDir;

    async fn make_git_repo() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            let out = tokio::process::Command::new("git")
                .current_dir(&path)
                .args(&args)
                .output()
                .await
                .unwrap();
            assert!(out.status.success());
        }
        tokio::fs::write(path.join("README.md"), "x").await.unwrap();
        let _ = tokio::process::Command::new("git")
            .current_dir(&path)
            .args(["add", "-A"])
            .output()
            .await;
        let _ = tokio::process::Command::new("git")
            .current_dir(&path)
            .args(["commit", "-qm", "x"])
            .output()
            .await;
        (dir, path)
    }

    #[tokio::test]
    async fn add_then_list_then_remove() {
        let registry_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;

        run_add(ProjectAddArgs {
            canonical_path: repo,
            name: Some("erp".into()),
            branch: None,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        // list 不该 fail
        run_list(ProjectListArgs {
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        // 物理验证
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let projects = registry.list().await.unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id.as_str(), "erp");

        // remove
        run_remove(ProjectRemoveArgs {
            id: "erp".into(),
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        let projects = registry.list().await.unwrap();
        assert!(projects.is_empty());
    }

    #[tokio::test]
    async fn remove_missing_id_errors() {
        let registry_root = tempfile::tempdir().unwrap();
        let err = run_remove(ProjectRemoveArgs {
            id: "ghost".into(),
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("不存在"), "got: {err}");
    }

    #[tokio::test]
    async fn deliverable_produce_publishes_event_when_events_db_provided() {
        use futures_util::StreamExt;
        use fuxi_events::ReplayCursor;

        let registry_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        run_add(ProjectAddArgs {
            canonical_path: repo,
            name: Some("erp".into()),
            branch: None,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        // 起一个 events.db
        let events_dir = tempfile::tempdir().unwrap();
        let events_db = events_dir.path().join("events.db");
        // 先 connect 一次让 schema 建好
        let _store = EventStore::connect_file(&events_db).await.unwrap();

        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("report.md");
        tokio::fs::write(&src, "# 报告").await.unwrap();

        let task = TaskId::new();
        run_deliverable_produce(DeliverableProduceArgs {
            project: "erp".into(),
            task: Some(task.to_string()),
            kind: "research_summary".into(),
            files: vec![src],
            registry_root: Some(registry_root.path().to_path_buf()),
            events_db: Some(events_db.clone()),
        })
        .await
        .expect("produce");

        // 重 connect 同 events.db 验证事件已落盘
        let store2 = EventStore::connect_file(&events_db).await.unwrap();
        let mut stream = store2.replay(ReplayCursor::Beginning);
        let mut found = false;
        while let Some(item) = stream.next().await {
            let ev = item.unwrap();
            if let EventKind::DeliverableProduced {
                task: t,
                project: p,
                deliverable_kind,
                files,
            } = &ev.kind
            {
                assert_eq!(*t, task);
                assert_eq!(p.as_str(), "erp");
                assert_eq!(*deliverable_kind, DeliverableKind::ResearchSummary);
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].name, "report.md");
                found = true;
                break;
            }
        }
        assert!(found, "DeliverableProduced 事件应已落盘");
    }

    #[tokio::test]
    async fn deliverable_produce_writes_files_to_bucket() {
        let registry_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;

        // 先 add 一个 project
        run_add(ProjectAddArgs {
            canonical_path: repo,
            name: Some("erp".into()),
            branch: None,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        // 准备一个源文件
        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("report.md");
        tokio::fs::write(&src, "# 调研结果").await.unwrap();

        let task = TaskId::new();
        run_deliverable_produce(DeliverableProduceArgs {
            project: "erp".into(),
            task: Some(task.to_string()),
            kind: "research_summary".into(),
            files: vec![src],
            registry_root: Some(registry_root.path().to_path_buf()),
            events_db: None,
        })
        .await
        .expect("produce");

        // 文件应已落到 deliverables/<task>/
        let bucket = registry_root
            .path()
            .join("erp")
            .join("deliverables")
            .join(task.to_string());
        assert!(bucket.is_dir(), "bucket 应建好");
        assert!(bucket.join("report.md").is_file(), "文件应落地");
        assert!(bucket.join("manifest.json").is_file(), "manifest 应写");
    }

    #[tokio::test]
    async fn deliverable_produce_rejects_unknown_project() {
        let registry_root = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("x.md");
        tokio::fs::write(&src, "x").await.unwrap();
        let err = run_deliverable_produce(DeliverableProduceArgs {
            project: "no-such-project".into(),
            task: None,
            kind: "research_summary".into(),
            files: vec![src],
            registry_root: Some(registry_root.path().to_path_buf()),
            events_db: None,
        })
        .await
        .expect_err("未注册 project 应拒");
        assert!(err.to_string().contains("未注册"), "got: {err}");
    }

    #[tokio::test]
    async fn deliverable_produce_rejects_unknown_kind() {
        let registry_root = tempfile::tempdir().unwrap();
        let (_td, repo) = make_git_repo().await;
        run_add(ProjectAddArgs {
            canonical_path: repo,
            name: Some("erp".into()),
            branch: None,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("x.md");
        tokio::fs::write(&src, "x").await.unwrap();

        let err = run_deliverable_produce(DeliverableProduceArgs {
            project: "erp".into(),
            task: None,
            kind: "wrong_kind".into(),
            files: vec![src],
            registry_root: Some(registry_root.path().to_path_buf()),
            events_db: None,
        })
        .await
        .expect_err("未知 kind 应拒");
        let full = format!("{err:#}");
        assert!(
            full.contains("未知 deliverable kind") || full.contains("research_summary"),
            "got: {full}"
        );
    }

    #[tokio::test]
    async fn project_info_shows_sandboxes_and_deliverables_count() {
        use fuxi_workspace::DeliverablesManager;

        let registry_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        run_add(ProjectAddArgs {
            canonical_path: repo,
            name: Some("erp".into()),
            branch: None,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        // 起 1 个 sandbox + 落 2 个 task 的 deliverable
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let project = registry
            .get(&ProjectId::new("erp").unwrap())
            .await
            .unwrap()
            .unwrap();
        let mgr = PersistentSandboxManager::new(project.clone(), registry.root());
        mgr.get_or_create("luban").await.unwrap();
        let dmgr = DeliverablesManager::new(project.id.clone(), registry.root());
        let src_dir = tempfile::tempdir().unwrap();
        for name in ["a.md", "b.md"] {
            let f = src_dir.path().join(name);
            tokio::fs::write(&f, name).await.unwrap();
            dmgr.produce(TaskId::new(), DeliverableKind::ResearchSummary, &[f])
                .await
                .unwrap();
        }

        // info 不该 fail
        run_info(ProjectInfoArgs {
            id: "erp".into(),
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn sandbox_list_then_retire_e2e() {
        let registry_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;

        // 注册 project
        run_add(ProjectAddArgs {
            canonical_path: repo,
            name: Some("erp".into()),
            branch: None,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();

        // 直接用 PersistentSandboxManager 起两个 sandbox（避免 spawn 真起 cc）
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let project = registry
            .get(&ProjectId::new("erp").unwrap())
            .await
            .unwrap()
            .unwrap();
        let mgr = PersistentSandboxManager::new(project, registry.root());
        mgr.get_or_create("luban").await.unwrap();
        mgr.get_or_create("pusong").await.unwrap();

        // sandbox list 不该 fail
        run_sandbox_list(SandboxListArgs {
            project: "erp".into(),
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();
        assert_eq!(mgr.list().await.unwrap().len(), 2);

        // retire luban
        run_sandbox_retire(SandboxRetireArgs {
            project: "erp".into(),
            role: "luban".into(),
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();
        let remaining = mgr.list().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].role, "pusong");
    }

    #[tokio::test]
    async fn sandbox_sweep_collects_expired_archives() {
        use chrono::Duration;
        use fuxi_workspace::EphemeralWorkspaceManager;

        let registry_root = tempfile::tempdir().unwrap();
        let (_repo_td, repo) = make_git_repo().await;
        run_add(ProjectAddArgs {
            canonical_path: repo,
            name: Some("erp".into()),
            branch: None,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();
        let registry = FileSystemProjectRegistry::new(registry_root.path());
        let project = registry
            .get(&ProjectId::new("erp").unwrap())
            .await
            .unwrap()
            .unwrap();
        let mgr = EphemeralWorkspaceManager::new(project.clone(), registry.root());

        // 起 + archive 一个 task，然后手工 backdate 让它过期
        let task = TaskId::new();
        mgr.create(task).await.unwrap();
        mgr.archive(task).await.unwrap();
        let meta_path = mgr
            .archive_root()
            .join(task.to_string())
            .join("fuxi-archive-meta.json");
        let mut meta: fuxi_workspace::ArchiveMeta =
            serde_json::from_slice(&tokio::fs::read(&meta_path).await.unwrap()).unwrap();
        meta.archived_at = chrono::Utc::now() - Duration::hours(48);
        tokio::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap())
            .await
            .unwrap();

        // sweep with 24h threshold → 应清掉
        run_sandbox_sweep(SandboxSweepArgs {
            project: Some("erp".into()),
            threshold_hours: 24,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap();
        assert!(mgr.list_archived().await.unwrap().is_empty(), "应清空");
    }

    #[tokio::test]
    async fn sandbox_list_rejects_unknown_project() {
        let registry_root = tempfile::tempdir().unwrap();
        let err = run_sandbox_list(SandboxListArgs {
            project: "ghost".into(),
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .expect_err("未注册 project 应失败");
        assert!(err.to_string().contains("未注册"), "got: {err}");
    }

    /// derive_join_target：home canonical_path basename 落到 $HOME。
    #[test]
    fn derive_join_target_picks_basename_under_home() {
        // SAFETY：单测内独占设置；其他并发测不读 HOME。
        unsafe { std::env::set_var("HOME", "/Users/test") };
        let target = derive_join_target(Path::new("/home/e0-7/demo-site")).expect("应能派生");
        assert_eq!(target, PathBuf::from("/Users/test/demo-site"));
    }

    /// 末尾斜杠不影响 basename 提取。
    #[test]
    fn derive_join_target_handles_trailing_slash() {
        unsafe { std::env::set_var("HOME", "/Users/test") };
        let target = derive_join_target(Path::new("/home/e0-7/demo-site/")).expect("应能派生");
        assert_eq!(target, PathBuf::from("/Users/test/demo-site"));
    }

    /// 显式 --node-id 优先级最高。
    #[test]
    fn resolve_self_node_id_explicit_wins() {
        let id = resolve_self_node_id(Some("explicit-mac".into())).unwrap();
        assert_eq!(id, "explicit-mac");
    }

    /// $FUXI_NODE_ID 次优先。
    #[test]
    fn resolve_self_node_id_env_used_when_no_explicit() {
        unsafe { std::env::set_var("FUXI_NODE_ID", "env-node") };
        let id = resolve_self_node_id(None).unwrap();
        unsafe { std::env::remove_var("FUXI_NODE_ID") };
        assert_eq!(id, "env-node");
    }

    #[tokio::test]
    async fn add_invalid_slug_errors() {
        let registry_root = tempfile::tempdir().unwrap();
        let (_td, repo) = make_git_repo().await;
        let err = run_add(ProjectAddArgs {
            canonical_path: repo,
            name: Some("BAD-SLUG".into()), // 大写
            branch: None,
            registry_root: Some(registry_root.path().to_path_buf()),
        })
        .await
        .unwrap_err();
        // anyhow::Error 用 chain() 能拿到下游错误链——top-level 是 "project add 失败"，
        // 真因（"[a-z0-9_-]" 提示）在 chain 里。{:#} 也行但这里用 chain 更显式。
        let full = format!("{err:#}");
        assert!(
            full.contains("[a-z0-9_-]") || full.contains("BAD-SLUG"),
            "got: {full}"
        );
    }
}

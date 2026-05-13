//! 玄女的工具子命令——`fuxi spawn/dispatch/intervene/status/list/kill`。
//!
//! 每个都是 daemon client 的薄壳。玄女 CC 实例通过 Bash 调用这些；
//! 人类直接调也 OK（调试、救急）。
//!
//! 设计公约：
//! - 成功：stdout 输出一行可读的 JSON 摘要（方便玄女 parse）
//! - 失败：stderr 打错误，exit code 非零
//! - **用户视角不应该进这些子命令**——这是写在 v0.1 scenario spec §1 铁律里
//!   的。这些是玄女的 Bash 工具而已。

use crate::client;
use crate::ipc::{Command, InterveneMode, Response};
use anyhow::{Result, anyhow};
use clap::Args as ClapArgs;

// ── spawn ──

#[derive(Debug, ClapArgs)]
pub struct SpawnArgs {
    /// 门客角色。必须存在 `roles/<role>/ROLE.md`（旧 `skills/<role>/SKILL.md` 仍兼容）。
    #[arg(long)]
    pub role: String,
    /// 可选名字（默认 role-N）。
    #[arg(long)]
    pub name: Option<String>,
    /// 可选执行节点。`local` 强制本机；例如 `home` 走分布式网关节点。
    #[arg(long)]
    pub node: Option<String>,
    /// 可选 CLI 覆写（`claude-code` / `codex`）。
    #[arg(long)]
    pub cli: Option<String>,
    /// P2 召回：续写指定 task 的 cc session（与 --recall-role 互斥）。
    /// 接受 `task-<uuid>` 或裸 `<uuid>`——daemon 端会标准化成 `task-<uuid>`。
    #[arg(long = "recall-task", conflicts_with = "recall_role")]
    pub recall_task: Option<String>,
    /// P2 召回：续写该 role 最近活动的 session（subject=`role-<role>`）。
    #[arg(long = "recall-role")]
    pub recall_role: Option<String>,
    /// Decision 21 phase 1：在已注册项目的 L3 持久 sandbox 里 spawn 门客。
    /// 项目 slug（必须先 `fuxi project add` 注册过）。给定后走
    /// `Fuxi::spawn_worker_in_project_sandbox` 路径——同 role 跨 task 复用
    /// `~/.fuxi/projects/<slug>/sandboxes/<role>/`（保留 build cache + WIP）。
    /// 不传 = 走旧 generic agent-id worktree 路径。
    #[arg(long)]
    pub project: Option<String>,
    /// Decision 21 phase 2：走 L2 ephemeral worktree 而非 L3 持久 sandbox。
    /// 一次性活用——`~/.fuxi/projects/<slug>/ephemeral/<task>/`，任务完后
    /// 用 `fuxi sandbox archive` 归档；24h 后被 `fuxi sandbox sweep` 清掉。
    /// **必须**配 `--project` 用；同时给 `--task <task-uuid>` 钉死 task id。
    /// 不给 `--task` → 服务端拒。
    #[arg(long, requires = "project")]
    pub ephemeral: bool,
    /// `--ephemeral` 模式下指定 task uuid。task-<uuid> 或裸 uuid 都行。
    /// 跟 P2 召回的 `--recall-task` 互斥（召回是复用旧 task 的 worktree，
    /// ephemeral 是为新 task 起 worktree）。
    #[arg(long = "task", conflicts_with = "recall_task")]
    pub task_for_ephemeral: Option<String>,
    /// 只输出裸 agent_id（便于 shell pipeline 直接接 `fuxi dispatch --to`）。
    /// 默认输出 JSON `{"agent_id":"..."}`（向后兼容）。
    /// issue 0a31de15：玄女实测 `fuxi spawn ... | xargs fuxi dispatch --to` 因
    /// JSON 而炸 `invalid character`，加这个 flag 让她直接 `--id-only` 接 pipe。
    #[arg(long = "id-only", default_value_t = false)]
    pub id_only: bool,
}

pub async fn run_spawn(args: SpawnArgs) -> Result<()> {
    let ephemeral_task =
        if args.ephemeral {
            Some(args.task_for_ephemeral.ok_or_else(|| {
                anyhow!("--ephemeral 必须配 --task <task-uuid>（一次性 task 的 id）")
            })?)
        } else {
            None
        };
    let id_only = args.id_only;
    let resp = client::send(Command::Spawn {
        role: args.role,
        name: args.name,
        node: args.node,
        cli: args.cli,
        recall_task: args.recall_task,
        recall_role: args.recall_role,
        project: args.project,
        ephemeral_task,
    })
    .await?;
    if id_only && let Some(id) = spawn_agent_id_from_response(&resp) {
        // issue 0a31de15：daemon 回 `{"agent_id":"...", ...}`，--id-only 时只
        // 打 id 给玄女 pipeline 接 dispatch --to。失败 / 字段缺退到 print_response
        // 走 JSON 路径——pipeline 至少能看到错。
        println!("{id}");
        return Ok(());
    }
    print_response(resp)
}

/// 从 daemon `Response::Ok` 提取 spawn 的裸 agent_id。--id-only 模式 + 测试用。
fn spawn_agent_id_from_response(resp: &Response) -> Option<String> {
    let Response::Ok { data } = resp else {
        return None;
    };
    data.get("agent_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

// ── dispatch ──

#[derive(Debug, ClapArgs)]
pub struct DispatchArgs {
    /// 目标门客 id（UUID）。
    #[arg(long = "to")]
    pub agent_id: String,
    /// 复用已有任务 id（可选）。同一个 task_id 可派给多个门客。
    #[arg(long = "task")]
    pub task_id: Option<String>,
    /// 任务标题。省略时自动从 body 第一行截前 60 字符派生（避免 PWA 任务列表
    /// 全是「ad-hoc」），玄女应优先显式传 `--title` 给清晰名字。
    #[arg(long)]
    pub title: Option<String>,
    /// 任务正文（prompt）。位置参数——剩余参数拼起来。
    #[arg(trailing_var_arg = true, required = true)]
    pub body: Vec<String>,
    /// 只打印 task_id（便于 shell capture 做同 task fan-out）。
    #[arg(long, default_value_t = false)]
    pub print_task_id: bool,
    /// β · #70 dist 路由 hint：钉到指定 dist node_id（如 `mac-local`）。
    /// `Fuxi::dispatch` 决策树命中 → 走 dist enqueue 而非本地 spawn。
    /// 玄女按 dispatch-routing.md 推断后传，PWA 不直接走 CLI。
    #[arg(long = "pinned-node")]
    pub pinned_node: Option<String>,
    /// β · #70 dist 路由 hint：能力 tag 集合（逗号分隔，如 `local,erp`）。
    /// 非空时走 dist enqueue。pinned_node 优先级高于 tags（玄女只需选其一）。
    #[arg(long = "required-tags", value_delimiter = ',')]
    pub required_tags: Vec<String>,
}

pub async fn run_dispatch(args: DispatchArgs) -> Result<()> {
    let body = args.body.join(" ");
    let title = args
        .title
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| derive_title_from_body(&body));
    let resp = client::send(Command::Dispatch {
        agent_id: args.agent_id,
        task_id: args.task_id,
        title,
        body: if body.is_empty() { None } else { Some(body) },
        pinned_node: args.pinned_node,
        required_tags: args.required_tags,
    })
    .await?;
    if !args.print_task_id {
        return print_response(resp);
    }
    let task_id = dispatch_task_id_from_response(resp)?;
    println!("{task_id}");
    Ok(())
}

/// 从 dispatch body 派生 task title——取首行（trim 后），最多 60 个 Unicode
/// 字符；超长加省略号。空 body 兜底 `"ad-hoc"`（不应正常发生：clap 已 required）。
///
/// 为什么 60：PWA 任务卡片单行宽约 24 中文字符；60 涵盖宽屏 + 切到列表
/// hover 时仍可读。比"前 N 字"截断比"找句号"复杂度低。
fn derive_title_from_body(body: &str) -> String {
    const MAX_TITLE_CHARS: usize = 60;
    let first_line = body.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return "ad-hoc".to_string();
    }
    let chars: Vec<char> = first_line.chars().collect();
    if chars.len() <= MAX_TITLE_CHARS {
        first_line.to_string()
    } else {
        let head: String = chars[..MAX_TITLE_CHARS].iter().collect();
        format!("{head}…")
    }
}

fn dispatch_task_id_from_response(resp: Response) -> Result<String> {
    match resp {
        Response::Ok { data } => data
            .get("task_id")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| anyhow!("dispatch 响应缺 task_id 字段: {data}")),
        Response::Pong => Err(anyhow!("dispatch 返回 pong，响应类型异常")),
        Response::Err { error } => Err(anyhow!(error)),
    }
}

// ── intervene ──

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum InterveneModeArg {
    Append,
    Interrupt,
}

impl From<InterveneModeArg> for InterveneMode {
    fn from(a: InterveneModeArg) -> Self {
        match a {
            InterveneModeArg::Append => InterveneMode::Append,
            InterveneModeArg::Interrupt => InterveneMode::Interrupt,
        }
    }
}

#[derive(Debug, ClapArgs)]
pub struct InterveneArgs {
    /// 目标门客 id。
    #[arg(long = "to")]
    pub agent_id: String,
    /// 介入模式：append（下 turn 看到）/ interrupt（打断当前 turn）。
    #[arg(long, default_value = "append")]
    pub mode: InterveneModeArg,
    /// 介入文本。
    #[arg(trailing_var_arg = true, required = true)]
    pub text: Vec<String>,
}

pub async fn run_intervene(args: InterveneArgs) -> Result<()> {
    let text = args.text.join(" ");
    let resp = client::send(Command::Intervene {
        agent_id: args.agent_id,
        mode: args.mode.into(),
        text,
    })
    .await?;
    print_response(resp)
}

// ── status ──

#[derive(Debug, ClapArgs)]
pub struct StatusArgs {
    /// 查指定 id；省略则返回全局概览。
    #[arg(long)]
    pub id: Option<String>,
}

pub async fn run_status(args: StatusArgs) -> Result<()> {
    let resp = client::send(Command::Status { agent_id: args.id }).await?;
    print_response(resp)
}

// ── list ──

#[derive(Debug, ClapArgs)]
pub struct ListArgs {}

pub async fn run_list(_args: ListArgs) -> Result<()> {
    let resp = client::send(Command::List).await?;
    print_response(resp)
}

// ── nodes ──
//
// `fuxi nodes` —— 看 dist worker 拓扑。controller 端 nodes 表的快照。
//
// 默认 table 格式（人读）；`--json` 出 NodeSnapshot 数组（脚本/玄女吃）；
// `--watch` 周期刷新（默认 1s，`--interval` 调）。
//
// **不订阅事件**——这是 ops 视角的快照命令，不破公理 #3（持续观察用 TUI）。

#[derive(Debug, ClapArgs)]
pub struct NodesArgs {
    /// 输出 JSON（机器可读）；默认是表格。
    #[arg(long, default_value_t = false)]
    pub json: bool,
    /// 持续刷新——每 `--interval` 秒重拉一次 snapshot 重渲染。
    #[arg(long, default_value_t = false)]
    pub watch: bool,
    /// `--watch` 的刷新间隔（秒）。
    #[arg(long, default_value_t = 1)]
    pub interval: u64,
}

pub async fn run_nodes(args: NodesArgs) -> Result<()> {
    if !args.watch {
        return run_nodes_once(args.json).await;
    }
    let interval = args.interval.max(1);
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(interval));
    loop {
        tick.tick().await;
        // 标准 ANSI：cursor 回 home + 清屏。轻量，不进 alt-screen。
        print!("\x1B[2J\x1B[H");
        if let Err(e) = run_nodes_once(args.json).await {
            // watch 短暂错误（daemon 重启等）不退，下一 tick 再试
            eprintln!("nodes 拉取失败: {e}（{interval}s 后重试）");
        }
    }
}

async fn run_nodes_once(as_json: bool) -> Result<()> {
    use crate::ipc::NodeSnapshot;
    let resp = client::send(Command::Nodes).await?;
    let data = match resp {
        Response::Ok { data } => data,
        Response::Pong => return Err(anyhow!("nodes 返回 pong，响应类型异常")),
        Response::Err { error } => return Err(anyhow!(error)),
    };
    let nodes_val = data
        .get("nodes")
        .ok_or_else(|| anyhow!("响应缺 nodes 字段: {data}"))?
        .clone();
    let nodes: Vec<NodeSnapshot> =
        serde_json::from_value(nodes_val).map_err(|e| anyhow!("nodes 解析失败: {e}"))?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&nodes)?);
        return Ok(());
    }
    print_nodes_table(&nodes);
    Ok(())
}

/// 表格输出：6 列固定宽度。无 worker 时打提示语避免误以为 daemon 没回。
pub(crate) fn print_nodes_table(nodes: &[crate::ipc::NodeSnapshot]) {
    if nodes.is_empty() {
        println!("(无 dist worker 节点。worker 起来后 register 会出现在这里。)");
        return;
    }
    println!(
        "{:<14} {:<7} {:<24} {:>8} {:>8} {:>10}",
        "NODE_ID", "STATUS", "TAGS", "INFLIGHT", "CAPACITY", "LAST_SEEN"
    );
    for n in nodes {
        let tags = if n.tags.is_empty() {
            "-".to_string()
        } else {
            n.tags.join(",")
        };
        let last_seen = match n.last_seen_ms_ago {
            None => "?".to_string(),
            Some(ms) => format_ms_ago(ms),
        };
        println!(
            "{:<14} {:<7} {:<24} {:>8} {:>8} {:>10}",
            truncate_cell(&n.node_id, 14),
            n.status,
            truncate_cell(&tags, 24),
            n.inflight_count,
            n.max_concurrency,
            last_seen,
        );
    }
}

/// 把毫秒数渲染成人读相对时间：`<1s` / `300ms` / `5s` / `2m` / `1h`。
pub(crate) fn format_ms_ago(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{}s", ms / 1_000)
    } else if ms < 3_600_000 {
        format!("{}m", ms / 60_000)
    } else {
        format!("{}h", ms / 3_600_000)
    }
}

/// 表格列截断：超长字符串用 `…` 收尾，避免错位。
fn truncate_cell(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ── block ──

#[derive(Debug, ClapArgs)]
pub struct BlockArgs {
    /// 要 block 的 task id。
    #[arg(long)]
    pub task: String,
    /// 原因（例如 "awaiting_commit_approval"）。
    #[arg(long, default_value = "awaiting_user_approval")]
    pub reason: String,
}

pub async fn run_block(args: BlockArgs) -> Result<()> {
    let resp = client::send(Command::BlockTask {
        task_id: args.task,
        reason: args.reason,
    })
    .await?;
    print_response(resp)
}

// ── task unblock（M3.1）+ resume alias ──
//
// 命名重整：`fuxi resume` 动词不当（"恢复"含混），改 `fuxi task unblock`（动宾明确：
// 解锁某 task）。`fuxi resume` 保留一个版本作 deprecated alias，调用时 stderr 警告，
// 走相同 daemon 路径（Command::ResumeTask 不改）。下个 minor 版本删 alias。

/// `fuxi task unblock` / `fuxi resume`（alias）共享的参数。
#[derive(Debug, ClapArgs)]
pub struct TaskUnblockArgs {
    /// 要 unblock 的 task id。
    #[arg(long)]
    pub task: String,
    /// 用户的授权话（"同意" / "同意，但改 X"）。留空也行。
    #[arg(long)]
    pub input: Option<String>,
}

/// 兼容 alias，沿用 ResumeArgs 名字保持 main.rs 旧签名。
pub type ResumeArgs = TaskUnblockArgs;

pub async fn run_task_unblock(args: TaskUnblockArgs) -> Result<()> {
    let resp = client::send(Command::ResumeTask {
        task_id: args.task,
        input: args.input,
    })
    .await?;
    print_response(resp)
}

/// 旧入口——stderr 打 deprecation 然后转走 unblock 同一逻辑。
pub async fn run_resume(args: ResumeArgs) -> Result<()> {
    eprintln!(
        "warning: `fuxi resume` 已弃用，请用 `fuxi task unblock --task <id>`；下个版本删除 alias"
    );
    run_task_unblock(args).await
}

// ── kill ──

#[derive(Debug, ClapArgs)]
pub struct KillArgs {
    #[arg(long = "id")]
    pub agent_id: String,
}

pub async fn run_kill(args: KillArgs) -> Result<()> {
    let resp = client::send(Command::Kill {
        agent_id: args.agent_id,
    })
    .await?;
    print_response(resp)
}

// ── events ──

/// `fuxi events` —— 救急直读 SQLite 看事件流。
///
/// **不走 daemon**：daemon 死了或调试时也要能看。和 `fuxi memory` 一样的策略——
/// 这是 ops 工具不是日常路径。日常路径是 TUI 实时渲染（公理 #3 真实时不轮询），
/// 玄女 skill 教学里明确写"不要把 `fuxi events` 当 poll 用"。
#[derive(Debug, ClapArgs)]
pub struct EventsArgs {
    /// SQLite 路径。省略时读 `$FUXI_DB`，再省则 `~/.fuxi/events.db`——
    /// daemon `fuxi im start` 写的就是这一份。issue d044c089 修复：老默认走
    /// `~/.fuxi/memory.db`（fuxi-memory 子系统的 db）→ `fuxi events` 不带
    /// `--db` 永远空输出，文档与实际不符。
    #[arg(long)]
    pub db: Option<std::path::PathBuf>,
    /// 取尾巴 N 条事件（最新在前的逆序输出，便于 less / grep）。
    #[arg(long, default_value_t = 50)]
    pub tail: i64,
    /// 不退出，每 500ms poll 新事件。
    /// WHY 轮询：这是绕开 daemon 的救急工具——没有 broadcast 通道可订阅。
    /// 用 rowid 游标避免重复输出已 dump 过的事件。
    #[arg(long)]
    pub follow: bool,
    /// 按 agent id 过滤（substring 匹配，前缀/裸 uuid/带 agent- 前缀都行）。
    /// issue c35ef4a0 修：filter 走 SQL where，"先 filter 再 tail N"，避免命中
    /// 事件因落在最近 N 条窗口外被丢。follow 模式仍走后处理 filter（增量流没有
    /// SQL 重过滤的语义）。
    #[arg(long)]
    pub filter: Option<String>,
}

/// `fuxi events` 默认 db 路径——issue d044c089：必须 events.db 不是 memory.db。
/// 跟 `crates/fuxi-cli/src/im.rs::default_events_db_path` 同语义；本地复一份避免
/// 跨模块可见性扩散。
pub(crate) fn default_events_db_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".fuxi").join("events.db"))
}

/// `--db` > `$FUXI_DB` > `~/.fuxi/events.db` 的解析顺序。
fn resolve_events_db_path(explicit: Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    if let Ok(env_path) = std::env::var("FUXI_DB")
        && !env_path.is_empty()
    {
        return Ok(std::path::PathBuf::from(env_path));
    }
    default_events_db_path()
        .ok_or_else(|| anyhow::anyhow!("无法解析 events.db 默认路径：$HOME 未设置"))
}

pub async fn run_events(args: EventsArgs) -> Result<()> {
    use fuxi_events::EventStore;
    let db = resolve_events_db_path(args.db)?;
    let store = EventStore::connect_file(&db).await?;

    // issue c35ef4a0：filter 非空走 SQL where；空走老的 recent。
    let mut events = match args.filter.as_deref() {
        Some(f) if !f.trim().is_empty() => store.recent_filtered(args.tail, f.trim()).await?,
        _ => store.recent(args.tail).await?,
    };
    // 时间正序输出——recent() 是 DESC（最新在前），但人读的时候习惯老→新。
    events.reverse();
    for ev in &events {
        if let Some(line) = format_event_line(ev, args.filter.as_deref()) {
            println!("{line}");
        }
    }

    if !args.follow {
        return Ok(());
    }

    // 轮询模式：记下最后一条 id 当游标，每 500ms 查 replay(FromId(cursor))。
    // WHY 用 FromId 不用 recent：recent 取头 N 倒序拿不到"id 之后所有新事件"；
    // FromId 用 rowid 严格大于 cursor 的语义，正是增量需求。
    let mut cursor = events.last().map(|e| e.meta.id);
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let cursor_or_begin = match cursor {
            Some(id) => fuxi_events::ReplayCursor::FromId(id),
            None => fuxi_events::ReplayCursor::Beginning,
        };
        use futures_util::StreamExt;
        let mut stream = store.replay(cursor_or_begin);
        while let Some(item) = stream.next().await {
            match item {
                Ok(ev) => {
                    if let Some(line) = format_event_line(&ev, args.filter.as_deref()) {
                        println!("{line}");
                    }
                    cursor = Some(ev.meta.id);
                }
                Err(e) => {
                    eprintln!("events follow: {e}");
                    break;
                }
            }
        }
    }
}

/// 把一条事件渲染成单行摘要：`<时间> <kind_tag> agent=<id> task=<id> [overview]`。
///
/// `filter` 命中 agent_id 前缀时输出，未命中返回 None。filter=None 即全部输出。
/// WHY 单独函数：测试可以直接喂 Event 验渲染，不用过 SQLite。
pub(crate) fn format_event_line(ev: &fuxi_core::Event, filter: Option<&str>) -> Option<String> {
    if let Some(f) = filter {
        let agent_match = ev
            .meta
            .agent
            .map(|a| a.to_string().contains(f))
            .unwrap_or(false);
        if !agent_match {
            return None;
        }
    }
    let kind_tag = serde_json::to_value(&ev.kind)
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(|t| t.as_str().map(|s| s.to_string()))
        })
        .unwrap_or_else(|| "unknown".into());
    let agent = ev
        .meta
        .agent
        .map(|a| a.to_string())
        .unwrap_or_else(|| "-".into());
    let task = ev
        .meta
        .task
        .map(|t| t.to_string())
        .unwrap_or_else(|| "-".into());
    let summary = event_summary(&ev.kind);
    let suffix = if summary.is_empty() {
        String::new()
    } else {
        format!(" · {summary}")
    };
    Some(format!(
        "{} {} agent={} task={}{}",
        ev.meta.at.format("%H:%M:%S"),
        kind_tag,
        agent,
        task,
        suffix
    ))
}

fn event_summary(kind: &fuxi_core::EventKind) -> String {
    use fuxi_core::EventKind::*;
    match kind {
        AgentSpawning { role, cli } => format!("role={role} cli={cli}"),
        AgentReady { endpoint } => format!("endpoint={endpoint}"),
        AgentDead { cause } => truncate_preview(cause, 120),
        TaskCreated { title, .. } => truncate_preview(title, 120),
        TaskStateChanged { from, to } => format!("{from:?} -> {to:?}"),
        TaskBlocked { reason } => truncate_preview(reason, 120),
        TaskResumed { input } => input
            .as_deref()
            .map(|s| truncate_preview(s, 120))
            .unwrap_or_else(|| "resumed".to_string()),
        UserPrompted { text } | AgentResponded { text } => truncate_preview(text, 160),
        ToolCallStarted { tool, .. } => format!("{tool} started"),
        ToolCallFinished {
            tool,
            ok,
            output_preview,
        } => {
            let state = if *ok { "ok" } else { "failed" };
            format!("{tool} {state}: {}", truncate_preview(output_preview, 100))
        }
        UserInterventionSent { mode, text, .. } => {
            format!("{mode}: {}", truncate_preview(text, 120))
        }
        AgentInterrupted { reason } => truncate_preview(reason, 120),
        TaskInterventionApplied { mode } => format!("mode={mode}"),
        OrchestratorCcReceived { text, .. } => truncate_preview(text, 120),
        TriggerRegistered { id, kind, .. } => format!("{id} ({kind})"),
        TriggerFired { id, cause, .. } => format!("{id} cause={cause}"),
        TriggerDispatched { id, to_agent } => format!("{id} -> {to_agent}"),
        TriggerSkipped { id, reason } => format!("{id}: {}", truncate_preview(reason, 100)),
        TriggerFailed { id, error } => format!("{id}: {}", truncate_preview(error, 100)),
        PlatformStarted { version } => format!("v{version}"),
        SkillStaged { role, template, .. } => format!("{role} ({template})"),
        SkillApproved { role } => role.clone(),
        SkillRejected { role, reason } => format!("{role}: {}", truncate_preview(reason, 100)),
        SkillActivated { role } => role.clone(),
        NoRoleMatched { need } => truncate_preview(need, 120),
        Custom { label, payload } => {
            format!("{label}: {}", truncate_preview(&payload.to_string(), 100))
        }
        PlatformStopping => "stopping".to_string(),
        AgentShuttingDown { reason } => truncate_preview(reason, 120),
        TaskDispatched { to } => format!("to={to}"),
        ThinkingStarted => "thinking".to_string(),
        ThinkingFinished => "thinking done".to_string(),
        AgentRequestReview {
            deliverable_kind,
            summary,
            ..
        } => format!("[{deliverable_kind:?}] {}", truncate_preview(summary, 120)),
        ReviewRequestTimeout { waited_for_ms, .. } => format!("timeout {waited_for_ms}ms"),
        WorkerRegistered {
            node_id,
            tags,
            max_concurrency,
        } => format!("{node_id} tags={tags:?} cap={max_concurrency}"),
        WorkerHeartbeatStateChanged {
            node_id,
            inflight_count,
            status,
        } => format!("{node_id} inflight={inflight_count} {status}"),
        WorkerStaleSwept {
            node_id,
            recycled_jobs,
        } => format!("{node_id} recycled={}", recycled_jobs.len()),
        AgentMessageQueued { from, to, text, .. } => {
            format!("{from} -> {to}: {}", truncate_preview(text, 100))
        }
        AgentMessageDelivered { from, to, .. } => format!("{from} -> {to} delivered"),
        AgentMessageRead { reader, .. } => format!("{reader} read"),
        AgentMessageFailed {
            from, to, error, ..
        } => {
            format!("{from} -> {to} failed: {}", truncate_preview(error, 100))
        }
        WorkspaceCreated {
            workspace_id,
            layer,
            ..
        } => format!("{workspace_id} ({layer:?})"),
        WorkspaceMutated {
            workspace_id,
            files_changed,
        } => format!("{workspace_id} files={files_changed}"),
        WorkspaceCommitted {
            workspace_id,
            commit_sha,
            branch,
        } => format!(
            "{workspace_id} {} on {branch}",
            truncate_preview(commit_sha, 12)
        ),
        WorkspaceArchived {
            workspace_id,
            reason,
        } => format!("{workspace_id} archived ({reason:?})"),
        WorkspaceCollected { workspace_id } => format!("{workspace_id} collected"),
        WorkspaceQuotaExceeded {
            project,
            quota_kind,
            requested,
            limit,
        } => format!("{project} {quota_kind:?} {requested}>{limit}"),
        WorkspacePromoted {
            from_workspace_id,
            to_role,
            project,
        } => format!("{from_workspace_id} -> {project}/L3/{to_role}"),
        DeliverableProduced {
            task,
            deliverable_kind,
            files,
            ..
        } => format!("{task} [{deliverable_kind:?}] files={}", files.len()),
        DeliverableAccepted { task, accepted_to } => format!(
            "{task} -> {}",
            accepted_to
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "(inbox)".into())
        ),
        DeliverableRejected { task, reason } => format!(
            "{task}{}",
            reason
                .as_deref()
                .map(|s| format!(": {}", truncate_preview(s, 100)))
                .unwrap_or_default()
        ),
        DeliverableExpired { task } => format!("{task} expired"),
        AgentInlineMessagePushed {
            task,
            from,
            filename,
            mime,
            body,
        } => format!("{task} ← {from} {filename} ({mime}, {} bytes)", body.len()),
        UsageReport {
            total_tokens,
            window_size,
            pct,
            ..
        } => format!(
            "usage {} / {} ({:.1}%)",
            total_tokens,
            window_size,
            pct * 100.0
        ),
        XuannvContextWatermark {
            threshold_pct,
            action,
            ..
        } => format!("玄女 ctx⚠ {threshold_pct}% [{action}]"),
        XuannvHandoffWritten { length_chars, .. } => {
            format!("玄女 handoff ↓ {length_chars} chars")
        }
        XuannvVoiceLine { text, emotion } => {
            let preview: String = text.chars().take(40).collect();
            let tag = emotion.as_deref().unwrap_or("normal");
            format!("玄女 say [{tag}] {preview}")
        }
        VisionRequest {
            target,
            hint,
            request_id,
        } => {
            let rid: String = request_id.chars().take(8).collect();
            match hint.as_deref() {
                Some(h) if !h.is_empty() => {
                    format!(
                        "玄女 look [{target}] req={rid} hint={}",
                        truncate_preview(h, 60)
                    )
                }
                _ => format!("玄女 look [{target}] req={rid}"),
            }
        }
    }
}

fn truncate_preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars().take(max.saturating_sub(1)) {
        out.push(ch);
    }
    out.push('…');
    out
}

// ── 更漏（cron / triggers） ──

#[derive(Debug, ClapArgs)]
pub struct CronAddArgs {
    /// cron 表达式。支持 5 字段（分时日月周）或 6 字段（含秒）。
    pub expr: String,
    /// 触发意图（自然语言，玄女收到后读）。
    #[arg(long)]
    pub intent: String,
    /// 时区（例 `Asia/Shanghai`）。默认 UTC。
    #[arg(long)]
    pub tz: Option<String>,
    /// 绑定的玄女 session_id；省略 = 触发时新起一个 session。
    #[arg(long)]
    pub session: Option<String>,
}

pub async fn run_cron_add(args: CronAddArgs) -> Result<()> {
    let resp = client::send(Command::CronAdd {
        expr: args.expr,
        intent: args.intent,
        tz: args.tz,
        session_id: args.session,
    })
    .await?;
    print_response(resp)
}

#[derive(Debug, ClapArgs)]
pub struct CronOnceArgs {
    /// 绝对时间（RFC3339，例 `2026-04-19T21:00:00+08:00`）。
    pub at: String,
    #[arg(long)]
    pub intent: String,
    #[arg(long)]
    pub session: Option<String>,
}

pub async fn run_cron_once(args: CronOnceArgs) -> Result<()> {
    let resp = client::send(Command::CronOnce {
        at: args.at,
        intent: args.intent,
        session_id: args.session,
    })
    .await?;
    print_response(resp)
}

#[derive(Debug, ClapArgs)]
pub struct CronWatchArgs {
    /// 要监视的目录或文件路径。
    pub path: String,
    #[arg(long)]
    pub intent: String,
    /// 只关心的事件类型白名单（`create`/`modify`/`remove`/`access`）；空 = 全部。
    #[arg(long, value_delimiter = ',')]
    pub events: Vec<String>,
    #[arg(long)]
    pub session: Option<String>,
}

pub async fn run_cron_watch(args: CronWatchArgs) -> Result<()> {
    let resp = client::send(Command::CronWatch {
        path: args.path,
        intent: args.intent,
        events: args.events,
        session_id: args.session,
    })
    .await?;
    print_response(resp)
}

#[derive(Debug, ClapArgs)]
pub struct CronWebhookArgs {
    #[arg(long)]
    pub intent: String,
    /// 可选共享密钥——命中时要求 header `x-fuxi-secret` 或 body `secret` 匹配。
    #[arg(long)]
    pub secret: Option<String>,
    #[arg(long)]
    pub session: Option<String>,
}

pub async fn run_cron_webhook(args: CronWebhookArgs) -> Result<()> {
    let resp = client::send(Command::CronWebhook {
        intent: args.intent,
        secret: args.secret,
        session_id: args.session,
    })
    .await?;
    print_response(resp)
}

#[derive(Debug, ClapArgs)]
pub struct CronListArgs {}

pub async fn run_cron_list(_args: CronListArgs) -> Result<()> {
    let resp = client::send(Command::CronList).await?;
    print_response(resp)
}

#[derive(Debug, ClapArgs)]
pub struct CronFireArgs {
    /// trigger_id（例 `trg_<uuid>`）。
    pub id: String,
}

pub async fn run_cron_fire(args: CronFireArgs) -> Result<()> {
    let resp = client::send(Command::CronFire { id: args.id }).await?;
    print_response(resp)
}

#[derive(Debug, ClapArgs)]
pub struct CronRemoveArgs {
    pub id: String,
}

pub async fn run_cron_remove(args: CronRemoveArgs) -> Result<()> {
    let resp = client::send(Command::CronRemove { id: args.id }).await?;
    print_response(resp)
}

// ── im start ──

/// `fuxi im start` 的参数 re-export——main.rs 只 import `subcommands::ImStartArgs`，
/// 真实装在 `crate::im::run` 里以保持 subcommands.rs 是薄壳约定。
pub use crate::im::StartArgs as ImStartArgs;

pub async fn run_im_start(args: ImStartArgs) -> Result<()> {
    crate::im::run(args).await
}

/// `fuxi im set-password` 的参数 re-export——同样 thin shell。
pub use crate::im::SetPasswordArgs as ImSetPasswordArgs;

pub async fn run_im_set_password(args: ImSetPasswordArgs) -> Result<()> {
    crate::im::run_set_password(args).await
}

pub use crate::im::IssueTokenArgs as ImIssueTokenArgs;

pub async fn run_im_issue_token(args: ImIssueTokenArgs) -> Result<()> {
    crate::im::run_issue_token(args).await
}

// ── 共用渲染 ──

/// 把 Response 打印到 stdout；Err 转成 anyhow 让 exit code 非零。
fn print_response(resp: Response) -> Result<()> {
    match resp {
        Response::Ok { data } => {
            println!("{}", data);
            Ok(())
        }
        Response::Pong => {
            println!("{{\"status\":\"pong\"}}");
            Ok(())
        }
        Response::Err { error } => Err(anyhow!(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--recall-task` 与 `--recall-role` 是 P2 召回的两条互斥入口；clap 的
    /// `conflicts_with` 必须在 wire 上即生效，避免错误同时给两个 flag 后
    /// 跑到 daemon 才报错——CLI 层早死早超生。
    #[test]
    fn spawn_args_recall_task_and_role_are_mutually_exclusive() {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            a: SpawnArgs,
        }
        let r = W::try_parse_from([
            "w",
            "--role",
            "dev",
            "--recall-task",
            "t1",
            "--recall-role",
            "dev",
        ]);
        assert!(r.is_err(), "互斥 flag 同时给应失败");
    }

    /// 单独给 `--recall-task` 应正常解析；recall_role 必须留 None。
    #[test]
    fn spawn_args_recall_task_only() {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            a: SpawnArgs,
        }
        let w = W::try_parse_from(["w", "--role", "dev", "--recall-task", "task-abc"]).unwrap();
        assert_eq!(w.a.recall_task.as_deref(), Some("task-abc"));
        assert!(w.a.recall_role.is_none());
    }

    #[test]
    fn spawn_args_parse_node_and_cli_override() {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            a: SpawnArgs,
        }
        let w = W::try_parse_from(["w", "--role", "luban", "--node", "home", "--cli", "codex"])
            .unwrap();
        assert_eq!(w.a.node.as_deref(), Some("home"));
        assert_eq!(w.a.cli.as_deref(), Some("codex"));
    }

    /// M3.1 · `fuxi task unblock` 和 `fuxi resume`（alias）共享 TaskUnblockArgs：
    /// 类型别名保证两路签名一致；clap 解析也都该接 `--task` `--input`。
    #[test]
    fn task_unblock_and_resume_share_args_shape() {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            a: TaskUnblockArgs,
        }
        let w = W::try_parse_from(["w", "--task", "task-1", "--input", "ok"]).unwrap();
        assert_eq!(w.a.task, "task-1");
        assert_eq!(w.a.input.as_deref(), Some("ok"));
        // ResumeArgs 是 TaskUnblockArgs 的 type alias——同一类型同一行为。
        let _: ResumeArgs = w.a;
    }

    /// `dispatch --print-task-id` 应由 clap 正确解析，且不影响原有位置参数 body。
    #[test]
    fn dispatch_args_parse_print_task_id_flag() {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            a: DispatchArgs,
        }
        let w = W::try_parse_from([
            "w",
            "--to",
            "agent-123",
            "--title",
            "修 auth bug",
            "--print-task-id",
            "请先跑测试",
        ])
        .unwrap();
        assert_eq!(w.a.agent_id, "agent-123");
        assert!(w.a.print_task_id);
        assert_eq!(w.a.body, vec!["请先跑测试"]);
        assert_eq!(w.a.title.as_deref(), Some("修 auth bug"));
    }

    /// 不传 `--title` 时 args.title 是 None，由 run_dispatch 调
    /// `derive_title_from_body` 派生（PWA 任务列表不再全是 ad-hoc）。
    #[test]
    fn dispatch_args_title_is_optional_when_omitted() {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            a: DispatchArgs,
        }
        let w = W::try_parse_from(["w", "--to", "agent-123", "请先跑测试"]).unwrap();
        assert!(w.a.title.is_none());
        assert_eq!(w.a.body, vec!["请先跑测试"]);
    }

    #[test]
    fn derive_title_takes_first_line_trimmed() {
        let body = "  修一下登录页\n详细：xxx";
        assert_eq!(derive_title_from_body(body), "修一下登录页");
    }

    #[test]
    fn derive_title_truncates_at_60_unicode_chars_with_ellipsis() {
        let body = "中".repeat(80);
        let got = derive_title_from_body(&body);
        let chars: Vec<char> = got.chars().collect();
        assert_eq!(chars.len(), 61, "60 字符 + 省略号: {got}");
        assert_eq!(chars[60], '…');
    }

    #[test]
    fn derive_title_short_body_passes_through() {
        let body = "short";
        assert_eq!(derive_title_from_body(body), "short");
    }

    #[test]
    fn derive_title_empty_body_falls_back_to_ad_hoc() {
        assert_eq!(derive_title_from_body(""), "ad-hoc");
        assert_eq!(derive_title_from_body("   \n  "), "ad-hoc");
    }

    /// β · #70 `--pinned-node` flag 解析。
    #[test]
    fn dispatch_args_parse_pinned_node() {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            a: DispatchArgs,
        }
        let w = W::try_parse_from([
            "w",
            "--to",
            "agent-123",
            "--pinned-node",
            "mac-local",
            "ls",
            "erp",
        ])
        .unwrap();
        assert_eq!(w.a.pinned_node.as_deref(), Some("mac-local"));
        assert!(w.a.required_tags.is_empty());
        assert_eq!(w.a.body, vec!["ls", "erp"]);
    }

    /// β · #70 `--required-tags local,erp` 逗号分隔解析为 Vec。
    #[test]
    fn dispatch_args_parse_required_tags_comma_separated() {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            a: DispatchArgs,
        }
        let w = W::try_parse_from([
            "w",
            "--to",
            "agent-123",
            "--required-tags",
            "local,erp",
            "看代码",
        ])
        .unwrap();
        assert_eq!(w.a.required_tags, vec!["local", "erp"]);
        assert!(w.a.pinned_node.is_none());
    }

    /// β · #70 既无 pinned-node 也无 required-tags → 默认空（本地 spawn 路径）
    #[test]
    fn dispatch_args_default_no_routing_hint() {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            a: DispatchArgs,
        }
        let w = W::try_parse_from(["w", "--to", "agent-123", "干活"]).unwrap();
        assert!(w.a.pinned_node.is_none());
        assert!(w.a.required_tags.is_empty());
    }

    #[test]
    fn dispatch_task_id_from_response_ok() {
        let resp = Response::Ok {
            data: serde_json::json!({"task_id":"task-123"}),
        };
        let got = dispatch_task_id_from_response(resp).unwrap();
        assert_eq!(got, "task-123");
    }

    /// issue 0a31de15 · --id-only 把 spawn 回的 JSON 提取裸 agent_id 给 pipeline。
    #[test]
    fn spawn_agent_id_from_response_extracts_bare_id() {
        let resp = Response::Ok {
            data: serde_json::json!({"agent_id":"agent-abc","worktree":"/tmp/x"}),
        };
        assert_eq!(
            spawn_agent_id_from_response(&resp),
            Some("agent-abc".to_string())
        );
    }

    #[test]
    fn spawn_agent_id_from_response_missing_field_returns_none() {
        let resp = Response::Ok {
            data: serde_json::json!({"task_id":"task-foo"}),
        };
        assert_eq!(spawn_agent_id_from_response(&resp), None);
    }

    #[test]
    fn spawn_agent_id_from_response_err_returns_none() {
        let resp = Response::Err {
            error: "daemon 挂了".into(),
        };
        assert_eq!(spawn_agent_id_from_response(&resp), None);
    }

    #[test]
    fn dispatch_task_id_from_response_missing_field_errors() {
        let resp = Response::Ok {
            data: serde_json::json!({"foo":"bar"}),
        };
        let err = dispatch_task_id_from_response(resp)
            .unwrap_err()
            .to_string();
        assert!(err.contains("缺 task_id"), "unexpected err: {err}");
    }

    /// `format_event_line` 渲染：含 kind_tag 标签 + agent/task 字段；
    /// 无 agent/task 的事件用 `-` 占位，避免 grep 时列错位。
    #[test]
    fn events_format_line_renders_kind_and_meta() {
        use fuxi_core::id::{AgentId, TaskId};
        use fuxi_core::{Event, EventKind, EventMeta};
        let aid = AgentId::new();
        let tid = TaskId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(aid);
        meta.task = Some(tid);
        let ev = Event {
            meta,
            kind: EventKind::AgentReady {
                endpoint: "stub://x".into(),
            },
        };
        let line = format_event_line(&ev, None).expect("无 filter 应输出");
        assert!(line.contains("agent_ready"), "缺 kind_tag: {line}");
        assert!(line.contains(&aid.to_string()), "缺 agent id: {line}");
        assert!(line.contains(&tid.to_string()), "缺 task id: {line}");
    }

    /// `--filter` 按 agent id 子串过滤——不命中返回 None（被跳过）。
    #[test]
    fn events_format_line_filter_skips_non_matching() {
        use fuxi_core::id::AgentId;
        use fuxi_core::{Event, EventKind, EventMeta};
        let aid = AgentId::new();
        let mut meta = EventMeta::now();
        meta.agent = Some(aid);
        let ev = Event {
            meta,
            kind: EventKind::PlatformStarted {
                version: "0.1".into(),
            },
        };
        // 命中 → Some
        let id_str = aid.to_string();
        assert!(format_event_line(&ev, Some(&id_str)).is_some());
        // 不命中 → None（救急 ops 看到 grep 输出和 stdout 一致很关键）
        assert!(format_event_line(&ev, Some("nonexistent-prefix")).is_none());
    }

    #[test]
    fn events_format_line_includes_agent_responded_text() {
        use fuxi_core::{Event, EventKind, EventMeta};
        let ev = Event {
            meta: EventMeta::now(),
            kind: EventKind::AgentResponded {
                text: "门客最终答案：README 可改两点".into(),
            },
        };
        let line = format_event_line(&ev, None).expect("应输出");
        assert!(line.contains("agent_responded"), "缺 kind: {line}");
        assert!(line.contains("README 可改两点"), "缺摘要正文: {line}");
    }

    /// issue d044c089 反回归：default 路径必须 `events.db` 不是 `memory.db`。
    /// daemon (`fuxi im start`) 写的就是 events.db；老默认走 memory_cmd 的
    /// memory.db 让 `fuxi events` 不带 `--db` 永远空输出。
    #[test]
    fn events_default_db_path_returns_events_not_memory() {
        // 临时设 HOME 防 CI 上 $HOME 缺失干扰 — 用现 HOME 即可
        let path = super::default_events_db_path()
            .expect("$HOME 应有，否则平台异常")
            .to_string_lossy()
            .into_owned();
        assert!(
            path.ends_with("/.fuxi/events.db"),
            "default 路径应是 ~/.fuxi/events.db；得到 {path}"
        );
        assert!(
            !path.ends_with("memory.db"),
            "回归保护：禁止 fallback 到 memory.db；得到 {path}"
        );
    }

    /// `fuxi nodes` 默认 table，--json 输出 NodeSnapshot[]——clap 解析两个 flag
    /// 都该正确落到 args 里；--interval 缺省 1。
    #[test]
    fn nodes_args_parses_json_watch_interval() {
        use clap::Parser;
        #[derive(Parser)]
        struct W {
            #[command(flatten)]
            a: NodesArgs,
        }
        let w = W::try_parse_from(["w"]).unwrap();
        assert!(!w.a.json && !w.a.watch);
        assert_eq!(w.a.interval, 1);

        let w = W::try_parse_from(["w", "--json"]).unwrap();
        assert!(w.a.json);

        let w = W::try_parse_from(["w", "--watch", "--interval", "5"]).unwrap();
        assert!(w.a.watch);
        assert_eq!(w.a.interval, 5);
    }

    #[test]
    fn format_ms_ago_renders_buckets() {
        assert_eq!(format_ms_ago(0), "0ms");
        assert_eq!(format_ms_ago(300), "300ms");
        assert_eq!(format_ms_ago(999), "999ms");
        assert_eq!(format_ms_ago(1_000), "1s");
        assert_eq!(format_ms_ago(62_000), "1m");
        assert_eq!(format_ms_ago(3_600_000), "1h");
        assert_eq!(format_ms_ago(7_200_000), "2h");
    }
}

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
    /// P2 召回：续写指定 task 的 cc session（与 --recall-role 互斥）。
    /// 接受 `task-<uuid>` 或裸 `<uuid>`——daemon 端会标准化成 `task-<uuid>`。
    #[arg(long = "recall-task", conflicts_with = "recall_role")]
    pub recall_task: Option<String>,
    /// P2 召回：续写该 role 最近活动的 session（subject=`role-<role>`）。
    #[arg(long = "recall-role")]
    pub recall_role: Option<String>,
}

pub async fn run_spawn(args: SpawnArgs) -> Result<()> {
    let resp = client::send(Command::Spawn {
        role: args.role,
        name: args.name,
        recall_task: args.recall_task,
        recall_role: args.recall_role,
    })
    .await?;
    print_response(resp)
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
    /// 任务标题。
    #[arg(long, default_value = "ad-hoc")]
    pub title: String,
    /// 任务正文（prompt）。位置参数——剩余参数拼起来。
    #[arg(trailing_var_arg = true, required = true)]
    pub body: Vec<String>,
    /// 只打印 task_id（便于 shell capture 做同 task fan-out）。
    #[arg(long, default_value_t = false)]
    pub print_task_id: bool,
}

pub async fn run_dispatch(args: DispatchArgs) -> Result<()> {
    let body = args.body.join(" ");
    let resp = client::send(Command::Dispatch {
        agent_id: args.agent_id,
        task_id: args.task_id,
        title: args.title,
        body: if body.is_empty() { None } else { Some(body) },
    })
    .await?;
    if !args.print_task_id {
        return print_response(resp);
    }
    let task_id = dispatch_task_id_from_response(resp)?;
    println!("{task_id}");
    Ok(())
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
    /// SQLite 路径。省略时读 `$FUXI_DB`，再省则 `~/.fuxi/memory.db`——
    /// 和 `fuxi memory` 同一文件（events / oracle 共存同一 db，PRAGMA WAL 互不踩）。
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
    /// 按 agent id 过滤（前缀匹配，便于贴半截 id）。
    #[arg(long)]
    pub filter: Option<String>,
}

pub async fn run_events(args: EventsArgs) -> Result<()> {
    use fuxi_events::EventStore;
    let db = crate::memory_cmd::resolve_db_path(args.db)?;
    let store = EventStore::connect_file(&db).await?;

    let mut events = store.recent(args.tail).await?;
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
    Some(format!(
        "{} {} agent={} task={}",
        ev.meta.at.format("%H:%M:%S"),
        kind_tag,
        agent,
        task
    ))
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
    }

    #[test]
    fn dispatch_task_id_from_response_ok() {
        let resp = Response::Ok {
            data: serde_json::json!({"task_id":"task-123"}),
        };
        let got = dispatch_task_id_from_response(resp).unwrap();
        assert_eq!(got, "task-123");
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
}

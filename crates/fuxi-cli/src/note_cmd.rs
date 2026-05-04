//! `fuxi note` —— P2.7 轻量文件推送子命令。
//!
//! 使用场景：门客在 sandbox 里跑出一段 markdown / 纯文本（小报告、状态摘要、
//! 临时草稿），想直推到任务对话流让用户立刻看，但又不值得走 deliverable
//! 收件箱（不是工件、不需要 accept_to 落地、不在五分类里）。
//!
//! 与 `fuxi deliverable produce` 的产品分工：
//! - 大于 256KB 或要落用户磁盘 → `fuxi deliverable produce`
//! - 小段消息体 + 只想"贴到对话里" → `fuxi note`
//!
//! Wire: 写一条 `EventKind::AgentInlineMessagePushed` 到共享 events.db；
//! IM 后端 conv_store 翻译成 worker 角色 inline_file 消息，PWA WorkerBubble
//! 内嵌渲染。

use anyhow::{Context, Result};
use clap::Args;
use fuxi_core::{AgentId, Event, EventKind, EventMeta, TaskId};
use fuxi_events::EventStore;
use std::path::PathBuf;
use std::str::FromStr;

const BODY_MAX_BYTES: usize = 256 * 1024;

/// `fuxi note --task <id> [--from <agent-id>] [--mime <type>] <file>`
#[derive(Debug, Args)]
pub struct NoteArgs {
    /// 任务 id（`task-<uuid>` 或裸 uuid）。门客在 sandbox 里能从注入到 prompt
    /// 头部的 `[FUXI_TASK_ID=...]` 抓到。必填。
    #[arg(long)]
    pub task: String,
    /// 发件门客 id（uuid）。缺省走 events.db 反查 `TaskDispatched.to`——
    /// 命中即用。两边都 None / 反查不到 → 报错让人工传 --from。
    #[arg(long)]
    pub from: Option<String>,
    /// MIME。缺省按文件扩展猜：`.md`/`.markdown` → `text/markdown`，其余
    /// 一律 `text/plain`。手动覆盖时必须是 `text/markdown` 或 `text/plain`
    /// 之一（v1 仅支持文本预览）。
    #[arg(long)]
    pub mime: Option<String>,
    /// 用户可见的文件名（basename），仅用于 PWA 卡片标题。缺省取入参文件的
    /// basename。
    #[arg(long)]
    pub name: Option<String>,
    /// 要推送的文件。≤ 256KB；超限请改走 `fuxi deliverable produce`。
    pub file: PathBuf,
    /// EventBus SQLite 文件路径覆写。默认 `$HOME/.fuxi/events.db`——同
    /// `fuxi im start`。文件必须已存在（fuxi-im 已起即 OK）。
    #[arg(long = "events-db")]
    pub events_db: Option<PathBuf>,
}

pub async fn run_note(args: NoteArgs) -> Result<()> {
    // 1. 解析 task / 文件 / mime / filename
    let task = parse_task_id(&args.task)?;
    let canon = args
        .file
        .canonicalize()
        .with_context(|| format!("文件无法解析: {}", args.file.display()))?;
    if !canon.is_file() {
        anyhow::bail!("源不是文件: {}", canon.display());
    }
    let body = std::fs::read_to_string(&canon)
        .with_context(|| format!("读文件失败（v1 仅支持 UTF-8 文本）: {}", canon.display()))?;
    if body.len() > BODY_MAX_BYTES {
        anyhow::bail!(
            "正文 {} bytes 超 256KB 上限——请改走 `fuxi deliverable produce` 走收件箱",
            body.len()
        );
    }

    let filename = args.name.unwrap_or_else(|| {
        canon
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("untitled")
            .to_string()
    });

    let mime = match args.mime.as_deref() {
        Some(m @ ("text/markdown" | "text/plain")) => m.to_string(),
        Some(other) => anyhow::bail!(
            "v1 仅支持 text/markdown 或 text/plain，收到 {other}（去掉 --mime 让它按扩展猜）"
        ),
        None => {
            let ext = canon
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if ext == "md" || ext == "markdown" {
                "text/markdown".into()
            } else {
                "text/plain".into()
            }
        }
    };

    // 2. 连 events.db
    let events_db = args
        .events_db
        .or_else(default_events_db_path)
        .ok_or_else(|| anyhow::anyhow!("无法定位 events.db（设 HOME 或显式传 --events-db）"))?;
    if !events_db.exists() {
        anyhow::bail!(
            "events.db 不存在: {} —— 先起 `fuxi im start` 再 note",
            events_db.display()
        );
    }
    let store = EventStore::connect_file(&events_db)
        .await
        .with_context(|| format!("connect events.db {} 失败", events_db.display()))?;

    // 3. 解析 from：显式 > events.db 反查 TaskDispatched.to
    let from = match args.from.as_deref() {
        Some(s) => parse_agent_id(s)?,
        None => find_assigned_agent(&store, task).await?.ok_or_else(|| {
            anyhow::anyhow!(
                "events.db 里没找到 task {task} 的 TaskDispatched——请显式传 --from <agent-uuid>"
            )
        })?,
    };

    // 4. 落事件
    let mut meta = EventMeta::now();
    meta.task = Some(task);
    meta.agent = Some(from);
    let body_len = body.len();
    let ev = Event {
        meta,
        kind: EventKind::AgentInlineMessagePushed {
            task,
            from,
            filename: filename.clone(),
            mime: mime.clone(),
            body,
        },
    };
    store.append(&ev).await.context("append 事件失败")?;
    println!(
        "已 inline-push: task={task} from={from} file={filename} mime={mime} bytes={body_len}"
    );
    Ok(())
}

fn parse_task_id(raw: &str) -> Result<TaskId> {
    let trimmed = raw.strip_prefix("task-").unwrap_or(raw);
    uuid::Uuid::from_str(trimmed)
        .map(TaskId::from)
        .map_err(|e| anyhow::anyhow!("无效 task id {raw}: {e}"))
}

fn parse_agent_id(raw: &str) -> Result<AgentId> {
    let trimmed = raw.strip_prefix("agent-").unwrap_or(raw);
    uuid::Uuid::from_str(trimmed)
        .map(AgentId::from)
        .map_err(|e| anyhow::anyhow!("无效 agent id {raw}: {e}"))
}

fn default_events_db_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".fuxi").join("events.db"))
}

/// events.db 反查：找 task 的 `TaskDispatched.to`。倒序查最新一条
/// （re-dispatch 场景下取最近赋的 agent）。
async fn find_assigned_agent(store: &EventStore, task: TaskId) -> Result<Option<AgentId>> {
    let events = store
        .history_for_task(task)
        .await
        .with_context(|| format!("history_for_task {task} 失败"))?;
    // 倒序拿最近一次 TaskDispatched
    for ev in events.iter().rev() {
        if let EventKind::TaskDispatched { to } = &ev.kind {
            return Ok(Some(*to));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_task_id_accepts_prefixed_and_bare() {
        let u = uuid::Uuid::new_v4();
        let with_prefix = format!("task-{u}");
        assert_eq!(parse_task_id(&with_prefix).unwrap(), TaskId::from(u));
        assert_eq!(parse_task_id(&u.to_string()).unwrap(), TaskId::from(u));
    }

    #[test]
    fn parse_task_id_rejects_garbage() {
        assert!(parse_task_id("not-a-uuid").is_err());
    }

    #[test]
    fn parse_agent_id_accepts_prefixed_and_bare() {
        let u = uuid::Uuid::new_v4();
        let with_prefix = format!("agent-{u}");
        assert_eq!(parse_agent_id(&with_prefix).unwrap(), AgentId::from(u));
        assert_eq!(parse_agent_id(&u.to_string()).unwrap(), AgentId::from(u));
    }
}

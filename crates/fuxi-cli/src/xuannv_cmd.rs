//! `fuxi xuannv refresh` —— 让玄女下次 fresh session 加载最新 dispatch-routing 教学。
//!
//! ## 为什么需要
//!
//! 玄女是长跑 cc 进程：`ensure_xuannv` 启动时若 oracle 里有 xuannv/session_id
//! record → 走 `cc --resume <session>` 续写。cc 自身行为：resume 时用**老 session
//! 的** system prompt，**忽略** `--append-system-prompt`。
//!
//! 后果：我们更新 `roles/xuannv/instructions/dispatch-routing.md` 教新东西
//! （比如 `fuxi spawn --project / --ephemeral` 用法），玄女永远学不到——除非
//! fresh session。
//!
//! ## 用法
//!
//! ```bash
//! fuxi xuannv refresh         # 清 oracle 里 session_id record
//! systemctl --user restart fuxi-im   # 触发 ensure_xuannv 走 fresh session 路径
//! ```
//!
//! 重启后 oracle 里没 session record → resolve_xuannv_session 走 fresh path →
//! cc 启动加 `--append-system-prompt`（含 dispatch-routing.md 最新版）→ 玄女
//! 学到新教学。
//!
//! ## 代价
//!
//! 玄女失忆——cc session 历史断档。下次启动是全新对话，不知道之前用户说了啥。
//! 这是单次成本：教学更新频率应远低于对话连续性需求。

use anyhow::{Context, Result};
use clap::Args;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_events::EventStore;
use fuxi_memory::OracleStore;
use std::io::Read;
use std::path::PathBuf;

use crate::session;

/// 玄女上下文交接落档绝对路径。**与后端 fuxi-im::xuannv_handoff 看的同一份**。
/// 路径走 `~/.fuxi/xuannv-handoff.md`——跟其他 fuxi 家产同目录，systemd 服务
/// 进程也读得到。
fn handoff_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".fuxi").join("xuannv-handoff.md"))
        .unwrap_or_else(|| PathBuf::from(".fuxi/xuannv-handoff.md"))
}

/// 上限：500 字 = 大约 1500-2000 chars（中文每字 1 char）。500 字数中文还能塞下
/// 「当前活跃 task + 待用户拍板事项 + 用户近期偏好」三段，超出说明玄女写跑题
/// 了——拒并提示她精简。
const HANDOFF_MAX_CHARS: usize = 2000;

/// daemon / IM 默认 events.db 路径——跟 `fuxi im start` 用同一份
/// (`im.rs::default_events_db_path` SoT：`$HOME/.fuxi/events.db`)。
fn default_events_db_path() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".fuxi").join("events.db"))
        .unwrap_or_else(|| std::path::PathBuf::from(".fuxi/events.db"))
}

#[derive(Debug, Args)]
pub struct HandoffWriteArgs {
    /// handoff 内容（≤500 字 markdown）；传 `-` 从 stdin 读。
    pub body: String,
}

pub async fn run_handoff_write(args: HandoffWriteArgs) -> Result<()> {
    let body = if args.body == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .context("读 stdin 失败")?;
        s
    } else {
        args.body
    };
    let trimmed = body.trim();
    if trimmed.is_empty() {
        anyhow::bail!("handoff 内容为空——必须 ≤500 字 markdown 实际内容");
    }
    let len = trimmed.chars().count();
    if len > HANDOFF_MAX_CHARS {
        anyhow::bail!(
            "handoff 超长：{len} chars > {HANDOFF_MAX_CHARS} chars。500 字应足够 \
             摘要「活跃 task + 待用户拍板事项 + 用户偏好」——精简后再写"
        );
    }

    let path = handoff_path();
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建父目录 {} 失败", parent.display()))?;
    }
    std::fs::write(&path, trimmed.as_bytes())
        .with_context(|| format!("写 handoff 文件 {} 失败", path.display()))?;
    println!("✓ handoff 已落档：{} ({} chars)", path.display(), len);

    // emit 事件——后端 fuxi-im 长跑进程订阅 EventBus 看 XuannvHandoffWritten
    // 即触发 idle kill + spawn 新副本。CLI 用 EventStore 直写而非走 daemon
    // socket：CLI 短命，连 socket 拉 IPC 重；EventStore append-only 安全。
    let events_db = std::env::var("FUXI_EVENTS_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_events_db_path());
    if events_db.exists() {
        let store = EventStore::connect_file(&events_db)
            .await
            .context("打开 events.db 失败")?;
        let mut meta = EventMeta::now();
        meta.session = None;
        let ev = Event {
            meta,
            kind: EventKind::XuannvHandoffWritten {
                path: path.clone(),
                length_chars: len.min(u32::MAX as usize) as u32,
            },
        };
        store
            .append(&ev)
            .await
            .context("XuannvHandoffWritten 事件入库失败")?;
        println!("✓ XuannvHandoffWritten 事件已 publish");
    } else {
        println!(
            "⚠ events.db 不存在（{}）——跳过事件发布",
            events_db.display()
        );
        println!("  fuxi-im 启动时会自动检测落档，不影响交接流程。");
    }
    Ok(())
}

pub async fn run_handoff_read() -> Result<()> {
    let path = handoff_path();
    if !path.exists() {
        println!("尚无 handoff 文件：{}", path.display());
        return Ok(());
    }
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("读 handoff 文件 {} 失败", path.display()))?;
    println!("=== handoff @ {} ===", path.display());
    println!("{}", body);
    Ok(())
}

/// Jarvis · 语音模式：上限只是"防呆"，玄女自己应该写一两句。超过 500 字
/// 大概率是她把整段 IM 回复也塞过来了——拒并提示她精简。
const SAY_MAX_CHARS: usize = 500;

#[derive(Debug, Args)]
pub struct SayArgs {
    /// 要念出口的话；传 `-` 从 stdin 读（兼容长字符串里有引号、换行等场景）。
    pub text: String,
}

pub async fn run_say(args: SayArgs) -> Result<()> {
    let text = if args.text == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .context("读 stdin 失败")?;
        s
    } else {
        args.text
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        anyhow::bail!("say 内容为空——至少给一两个字让用户听到");
    }
    let len = trimmed.chars().count();
    if len > SAY_MAX_CHARS {
        anyhow::bail!(
            "say 超长：{len} chars > {SAY_MAX_CHARS}。语音模式下只念一两句， \
             长内容（代码 / 列表 / 解释）写 IM 即可，不必念出来"
        );
    }

    // 走 daemon —— daemon 端 EmitEvent handler 会注入 meta.agent=xuannv_id，
    // CLI 进程拿不到运行时 xuannv_id，必须 daemon 兜底。
    let resp = crate::client::send(crate::ipc::Command::EmitEvent {
        kind: crate::ipc::EventKindPayload::XuannvVoiceLine {
            text: trimmed.to_string(),
        },
    })
    .await
    .context("daemon 通讯失败——是否在 `fuxi up` 或 `fuxi im start` 进程下")?;
    match resp {
        crate::ipc::Response::Ok { .. } => {
            println!("✓ 已上发 ({} chars)", len);
            Ok(())
        }
        crate::ipc::Response::Pong => anyhow::bail!("daemon 返回 Pong（异常）"),
        crate::ipc::Response::Err { error } => anyhow::bail!("daemon 拒绝：{error}"),
    }
}

pub async fn run_refresh() -> Result<()> {
    let path = std::env::var("FUXI_EVENTS_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| default_events_db_path());

    if !path.exists() {
        println!("策府数据库不存在：{}", path.display());
        println!("→ 没历史 session record 可清——下次 fuxi-im 启动自动 fresh.");
        return Ok(());
    }

    let oracle = OracleStore::connect_file(&path)
        .await
        .with_context(|| format!("打开策府 {}", path.display()))?;

    let cleared = session::forget_xuannv_session(&oracle)
        .await
        .context("清 oracle xuannv session record 失败")?;

    if cleared == 0 {
        println!("没找到玄女 session record（之前就是 fresh）。");
    } else {
        println!("已清 {cleared} 条玄女 session record。");
    }
    println!();
    println!("下一步：");
    println!("  systemctl --user restart fuxi-im");
    println!();
    println!("（触发 ensure_xuannv 走 fresh session 路径，cc 重读");
    println!("  `--append-system-prompt`，含 dispatch-routing.md 最新版）");
    Ok(())
}

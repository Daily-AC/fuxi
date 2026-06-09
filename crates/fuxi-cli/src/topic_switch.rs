//! Phase 1 · 切 topic 入口。
//!
//! 用户从 topic A 切到 topic B 时：fuxi 关掉当前玄女 cc 进程 → 拉 B 的对话历史
//! 拼 prelude → 起新 cc 进程注入 prelude → set_xuannv + set_current_topic。
//!
//! 复用 [`crate::xuannv_handoff::spawn_with_prelude`]：handoff 跟 topic 切换都
//! 走 kill+spawn-with-prelude pattern，区别只在 prelude 内容（handoff = 上一只
//! 副本的 handoff 摘要；topic = 新 topic 的对话回顾）。
//!
//! 设计依据：handoff `v1-session19.md` §3.2 + 决策 1（fuxi 重建 prelude 不依赖
//! cc resume）。

use anyhow::{Context, Result};
use fuxi_core::TopicId;
use fuxi_im::conv_store::{ConvStore, Message, SCOPE_XUANNV};
use fuxi_im::topic_store::TopicStore;
use fuxi_memory::OracleStore;
use fuxi_orchestrator::Fuxi;
use std::time::Duration;
use tracing::{info, warn};

use crate::xuannv_handoff::{spawn_with_prelude, wait_xuannv_idle};

/// prelude 拼接时拉的玄女主线最近消息条数。
pub const DEFAULT_RECENT_MESSAGES: usize = 50;
/// 单条消息文本预览截断字符上限——避免单条爆炸长把 prelude 撑爆。
pub const MESSAGE_PREVIEW_CHAR_LIMIT: usize = 240;
/// prelude 文本总字符上限（粗保护，超出按整段截尾）。handoff §3.2 给的指标：≤ 1500 字。
pub const PRELUDE_TOTAL_CHAR_LIMIT: usize = 1500;

/// 切 topic 到 `target_id`——完整路径：等 idle → kill old → 拉 prelude → spawn new
/// → set_xuannv + set_current_topic + touch_last_active。
///
/// 失败语义：若 kill 老玄女失败仍继续 spawn 新副本（老进程可能已死）。spawn 失败
/// 整体 bail——current_topic / xuannv 不更新，调用方按需 retry。
pub async fn switch_topic_to(
    fuxi: &Fuxi,
    oracle: &OracleStore,
    role: &str,
    conv_store: &ConvStore,
    topic_store: &TopicStore,
    target_id: TopicId,
) -> Result<()> {
    // 验证 topic 存在；不存在拒切（避免 typo 导致玄女绑到孤儿 id）。
    let target_meta = topic_store
        .get(target_id)
        .await
        .with_context(|| format!("查 topic {target_id} 失败"))?
        .with_context(|| format!("topic {target_id} 不存在，先创建"))?;

    let old_xuannv = fuxi
        .xuannv_id()
        .await
        .context("玄女 id 未设——尚未 spawn 完成？")?;
    info!(old = %old_xuannv, target = %target_id, "等玄女当前 turn idle 后切 topic");
    wait_xuannv_idle(fuxi, old_xuannv).await;

    info!(old = %old_xuannv, "kill 老玄女副本");
    if let Err(err) = fuxi
        .shutdown_xuannv_for_handoff(old_xuannv, format!("topic_switch_to:{target_id}"))
        .await
    {
        warn!(?err, "kill 老玄女失败——继续 spawn 新副本（老进程可能已死）");
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 拉新 topic 的对话回顾。失败 warn 不 bail——空回顾仍能 spawn，玄女首条消息
    // 由用户接着说。
    let recent = match load_recent_messages(conv_store, DEFAULT_RECENT_MESSAGES).await {
        Ok(v) => v,
        Err(err) => {
            warn!(?err, "拉对话回顾失败，prelude 走空回顾");
            Vec::new()
        }
    };
    let prelude = build_topic_prelude(&target_meta.title, &recent);
    info!(
        target = %target_id,
        title = %target_meta.title,
        msgs = recent.len(),
        prelude_chars = prelude.chars().count(),
        "spawn 新玄女副本（注入 topic prelude）"
    );

    let new_id = spawn_with_prelude(fuxi, oracle, role, &prelude)
        .await
        .context("spawn 新玄女失败")?;
    fuxi.set_xuannv(new_id).await;
    fuxi.set_current_topic(target_id).await;
    if let Err(err) = topic_store.touch_last_active(target_id).await {
        warn!(?err, "touch_last_active 失败，sidebar 排序可能滞后");
    }
    info!(new = %new_id, target = %target_id, "topic 切换完成");
    Ok(())
}

/// 从 conv_store 拉玄女主线（[`SCOPE_XUANNV`]）最近 N 条消息。
///
/// Phase 1 v1：先拉主线最近若干条作 prelude 回顾。Phase 1 v2 拆 topic 后改为按
/// `messages.topic_id = target` 过滤拉。当前是过渡——保证切 topic 总有上文兜底。
pub async fn load_recent_messages(conv_store: &ConvStore, n: usize) -> Result<Vec<Message>> {
    let conv_id = conv_store
        .ensure_scope(SCOPE_XUANNV, None)
        .await
        .context("ensure xuannv scope")?;
    let (msgs, _has_more, _oldest) = conv_store
        .page_messages(&conv_id, n, None)
        .await
        .context("page_messages")?;
    Ok(msgs)
}

/// 把 topic 标题 + 最近消息拼成给新玄女副本的 prelude 文本。
///
/// 文本上限 [`PRELUDE_TOTAL_CHAR_LIMIT`]；超限按整段截尾不切单条。
/// 抽出独立 fn 让单测可验证文案不漂移 + 长度兜底。
pub fn build_topic_prelude(topic_title: &str, recent: &[Message]) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    let _ = writeln!(
        out,
        "## 切到话题「{topic_title}」（必读）\n\n\
         你刚被 fuxi 切到 topic「{topic_title}」上。下面是这条 topic 最近的对话回顾，\
         请把它当作「你刚才在做的事」读，不要当陌生信息。回顾结束后直接按用户接下来\
         的消息继续往下聊，**不要**单独发「✻ 切换完成」之类的元消息——切 topic 应\
         是无感的（除非用户主动问）。\n\n\
         ---"
    );

    if recent.is_empty() {
        let _ = writeln!(out, "（暂无历史对话——这是该 topic 的首次进入）");
    } else {
        for m in recent {
            let role = display_role(&m.role, m.agent_id.as_deref());
            let text = extract_message_text(&m.content);
            let preview = truncate_chars(&text, MESSAGE_PREVIEW_CHAR_LIMIT);
            let _ = writeln!(out, "[{role}] {preview}");
        }
    }
    let _ = writeln!(out, "---\n");

    truncate_chars(&out, PRELUDE_TOTAL_CHAR_LIMIT)
}

fn display_role(role: &str, agent_id: Option<&str>) -> String {
    match role {
        "user" => "用户".to_string(),
        "system" => "系统".to_string(),
        "xuannv" => "玄女".to_string(),
        other => match agent_id {
            Some(id) => format!("{other}:{id}"),
            None => other.to_string(),
        },
    }
}

fn extract_message_text(content: &serde_json::Value) -> String {
    content
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// 按字符截断：返回总字符数（含省略号）≤ `max` 的字符串。
/// max=0 退化为空串；max=1 返单个省略号。保留中文边界（按 chars 不按 byte）。
fn truncate_chars(s: &str, max: usize) -> String {
    let total = s.chars().count();
    if total <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let head: String = s.chars().take(max - 1).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use fuxi_im::conv_store::Message;

    fn msg(role: &str, text: &str) -> Message {
        Message {
            id: uuid::Uuid::new_v4().to_string(),
            conv_id: "test-conv".into(),
            role: role.into(),
            agent_id: None,
            kind: "text".into(),
            content: serde_json::json!({ "text": text }),
            attachments: None,
            source_event_id: None,
            ts: Utc::now(),
            attachment_uploads: Vec::new(),
            topic_id: TopicId::general().0.to_string(),
        }
    }

    #[test]
    fn build_prelude_empty_history_renders_placeholder() {
        let out = build_topic_prelude("画头像", &[]);
        assert!(out.contains("「画头像」"));
        assert!(out.contains("首次进入"));
        assert!(out.chars().count() <= PRELUDE_TOTAL_CHAR_LIMIT);
    }

    #[test]
    fn build_prelude_includes_user_and_xuannv_roles() {
        let recent = vec![
            msg("user", "帮我画个萝莉斯"),
            msg("xuannv", "好的，我先起两张草图"),
        ];
        let out = build_topic_prelude("画头像", &recent);
        assert!(out.contains("[用户] 帮我画个萝莉斯"));
        assert!(out.contains("[玄女] 好的，我先起两张草图"));
    }

    #[test]
    fn build_prelude_truncates_super_long_history() {
        // 灌 100 条长消息，确保 total 超 1500 字 → 截尾省略
        let long = "x".repeat(300);
        let recent: Vec<Message> = (0..100).map(|_| msg("user", &long)).collect();
        let out = build_topic_prelude("ramble", &recent);
        assert!(out.chars().count() <= PRELUDE_TOTAL_CHAR_LIMIT);
        assert!(out.ends_with('…') || out.ends_with("…\n"));
    }

    #[test]
    fn truncate_chars_handles_chinese_boundary() {
        // 5 个中文字符 = 15 bytes；max=4 含省略号应得 3 字 + …
        let out = truncate_chars("你好世界吗", 4);
        assert_eq!(out, "你好世…");
        // max=3 包含省略号 → 2 字 + …
        let out = truncate_chars("你好世界吗", 3);
        assert_eq!(out, "你好…");
    }

    #[test]
    fn extract_message_text_falls_back_to_empty_on_missing_field() {
        let v = serde_json::json!({"other": "noise"});
        assert_eq!(extract_message_text(&v), "");
    }

    #[test]
    fn display_role_translates_known_roles() {
        assert_eq!(display_role("user", None), "用户");
        assert_eq!(display_role("xuannv", None), "玄女");
        assert_eq!(display_role("system", None), "系统");
        assert_eq!(display_role("painter", Some("abc")), "painter:abc");
        assert_eq!(display_role("painter", None), "painter");
    }
}

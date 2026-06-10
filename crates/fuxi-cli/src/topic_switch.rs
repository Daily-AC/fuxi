//! Phase 2 · 切 topic 入口。
//!
//! Phase 1（kill 当前 cc → 拉历史拼 prelude → spawn 新 cc）已废：切 topic 不再
//! 杀进程。现在切 = `set_current_topic` + `ensure_xuannv_for_topic`——池里有活
//! 分身就毫秒级秒切（上下文真留内存，不重灌回顾）；没有则懒启动（走
//! [`crate::xuannv_spawner_impl::TopicXuannvSpawner`]：按 topic 过滤的回顾
//! prelude + FUXI_TOPIC env + drain 持久队列）。旧分身留给 idle_gc dormant 回收。
//!
//! 本文件剩下的 prelude 拼装函数（[`build_topic_prelude`] 等）是懒启动路径的
//! 共享件，被 spawner impl 复用——别删。
//!
//! 设计依据：spec `2026-06-11-玄女分身-phase2-路由-design.md` §4.1。

use anyhow::{Context, Result};
use fuxi_core::TopicId;
use fuxi_im::conv_store::Message;
use fuxi_im::topic_store::TopicStore;
use fuxi_orchestrator::Fuxi;
use tracing::{info, warn};

/// 懒启动拉 topic 历史拼 prelude 时的最近消息条数。
pub const DEFAULT_RECENT_MESSAGES: usize = 50;
/// 单条消息文本预览截断字符上限——避免单条爆炸长把 prelude 撑爆。
pub const MESSAGE_PREVIEW_CHAR_LIMIT: usize = 240;
/// prelude 文本总字符上限（粗保护，超出按整段截尾）。handoff §3.2 给的指标：≤ 1500 字。
pub const PRELUDE_TOTAL_CHAR_LIMIT: usize = 1500;

/// 切 topic 到 `target_id`——Phase 2 路径：验证 topic 存在 → ensure 分身
/// （热路径 = 池查询毫秒级；冷路径 = spawner 懒启动数秒）→ commit
/// `set_current_topic` → touch_last_active。
///
/// 失败语义：ensure 失败（spawner 未注入 / spawn 炸）整体 bail，
/// current_topic **不** flip——调用方按需 retry（HTTP 5xx）。
/// 不杀旧分身、不等旧分身 idle：旧 turn 跑完输出落它自己的 topic。
pub async fn switch_topic_to(
    fuxi: &Fuxi,
    topic_store: &TopicStore,
    target_id: TopicId,
) -> Result<()> {
    // 验证 topic 存在；不存在拒切（避免 typo 导致玄女绑到孤儿 id）。
    let target_meta = topic_store
        .get(target_id)
        .await
        .with_context(|| format!("查 topic {target_id} 失败"))?
        .with_context(|| format!("topic {target_id} 不存在，先创建"))?;

    let clone = fuxi
        .ensure_xuannv_for_topic(target_id)
        .await
        .context("ensure 分身失败（spawner 未注入或 spawn 失败）")?;

    fuxi.set_current_topic(target_id).await;
    if let Err(err) = topic_store.touch_last_active(target_id).await {
        warn!(?err, "touch_last_active 失败，sidebar 排序可能滞后");
    }
    info!(
        %clone,
        target = %target_id,
        title = %target_meta.title,
        "topic 切换完成（常驻分身，未 kill）"
    );
    Ok(())
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
    use fuxi_core::id::AgentId;
    use fuxi_events::EventBus;
    use fuxi_im::conv_store::Message;
    use fuxi_im::db::init_at;
    use fuxi_im::topic_store::TopicStore;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;

    async fn make_fuxi() -> Arc<Fuxi> {
        let dir = tempfile::tempdir().unwrap();
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let bus = EventBus::with_memory_store().await.unwrap();
        std::mem::forget(dir);
        Arc::new(Fuxi::new(bus, ws))
    }

    async fn open_topic_store() -> (tempfile::TempDir, TopicStore) {
        let dir = tempfile::tempdir().unwrap();
        let pool = init_at(&dir.path().join("im.db")).await.unwrap();
        (dir, TopicStore::new(pool))
    }

    #[tokio::test]
    async fn switch_hot_path_keeps_clone_and_flips_topic() {
        // Phase 2 核心：池里已有活分身 → 秒切，分身 id 不变（不 kill 不 respawn）。
        let (_dir, topic_store) = open_topic_store().await;
        let fuxi = make_fuxi().await;
        let meta = topic_store.create("画画").await.unwrap();
        let clone = AgentId::new();
        fuxi.set_xuannv_for_topic(meta.id, clone).await;

        switch_topic_to(&fuxi, &topic_store, meta.id)
            .await
            .expect("热路径 switch 应成功");

        assert_eq!(fuxi.current_topic_id(), meta.id);
        assert_eq!(
            fuxi.xuannv_id_for_topic(meta.id).await,
            Some(clone),
            "热路径不得 kill / 换分身"
        );
    }

    #[tokio::test]
    async fn switch_cold_path_without_spawner_bails_and_keeps_topic() {
        // 池 miss + spawner 未注入 → ensure 返 None → bail，current_topic 不动
        //（Phase 1 失败语义保留：失败不 flip）。
        let (_dir, topic_store) = open_topic_store().await;
        let fuxi = make_fuxi().await;
        let meta = topic_store.create("新话题").await.unwrap();
        let before = fuxi.current_topic_id();

        let r = switch_topic_to(&fuxi, &topic_store, meta.id).await;

        assert!(r.is_err(), "ensure 失败应 bail");
        assert_eq!(
            fuxi.current_topic_id(),
            before,
            "失败不得 flip current_topic"
        );
    }

    #[tokio::test]
    async fn switch_rejects_unknown_topic() {
        let (_dir, topic_store) = open_topic_store().await;
        let fuxi = make_fuxi().await;
        let r = switch_topic_to(&fuxi, &topic_store, fuxi_core::TopicId::new()).await;
        assert!(r.is_err(), "不存在的 topic 应拒切");
    }

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

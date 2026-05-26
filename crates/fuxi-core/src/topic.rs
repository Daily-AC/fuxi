//! Topic —— 玄女与用户多话题并行的一等公民。
//!
//! Phase 1 设计（handoff session19 §2/§3）：用户跟玄女在多个话题之间切换；
//! 切 topic = fuxi 关掉当前 cc 进程 + 起新 cc 注入 topic-scoped prelude。
//! 老数据全归默认 topic [`TopicId::general()`]，向后兼容。
//!
//! 本模块只定义 vocabulary（id + meta + 校验）。SQL 表/CRUD 在 `fuxi-im`，
//! 路由 filter 在 `fuxi-orchestrator`。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Topic id。
///
/// Phase 1 选 UUID 而非 slug：title 用户可改且可重复（"周末画画" × 2），
/// id 必须稳定且与 title 解耦。`general` 是兜底常量 UUID 让老数据可归位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TopicId(pub Uuid);

impl TopicId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// 默认兜底 topic。所有 Phase 1 之前持久化的事件 / 消息归这里。
    ///
    /// WHY 选常量 UUID 而非 nil(0…0)：nil 对人类不可读；这里固定成
    /// `00000000-0000-0000-0000-000000000001` 在 sqlite / log 里一眼可识别为
    /// "general"，又跟随机生成的真 topic id 没碰撞概率。
    pub const fn general() -> Self {
        Self(Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_0001))
    }

    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for TopicId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TopicId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "topic-{}", self.0)
    }
}

impl From<Uuid> for TopicId {
    fn from(u: Uuid) -> Self {
        Self(u)
    }
}

/// Topic 持久化元数据。
///
/// `pinned` / `archived_at` 是 Phase 2 才需要但字段先占位避免老 db migrate
/// 二次破坏；当前 CLI / UI 不暴露 pin。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopicMeta {
    pub id: TopicId,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub last_active_at: DateTime<Utc>,
    /// pin 到 sidebar 顶。决策 3：第一版不暴露此功能，字段保留。
    #[serde(default)]
    pub pinned: bool,
    /// 归档时间。`None` = 活跃 topic。归档 ≠ 删除（决策 6）。
    #[serde(default)]
    pub archived_at: Option<DateTime<Utc>>,
}

impl TopicMeta {
    /// 用 title 构造一个全新活跃 topic（id 随机、created/last_active 同一时刻）。
    pub fn new(title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: TopicId::new(),
            title: title.into(),
            created_at: now,
            last_active_at: now,
            pinned: false,
            archived_at: None,
        }
    }

    /// 默认 "general" topic 的标准 meta。供 migration 一键塞入。
    pub fn general() -> Self {
        // 这里时间固定到一个语义性时间点（fuxi 项目起点 2026-04-19），
        // 不用 Utc::now() 是为了让多机器/多次 init 出一致结果。
        let anchor: DateTime<Utc> = DateTime::parse_from_rfc3339("2026-04-19T00:00:00Z")
            .expect("anchor literal is well-formed")
            .with_timezone(&Utc);
        Self {
            id: TopicId::general(),
            title: "general".to_string(),
            created_at: anchor,
            last_active_at: anchor,
            pinned: false,
            archived_at: None,
        }
    }

    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topic_id_general_is_stable_constant() {
        // 必须是确定性 UUID，多次调用同值。Migration / 老数据归位依赖这个稳定性。
        assert_eq!(TopicId::general(), TopicId::general());
        assert_eq!(
            TopicId::general().to_string(),
            "topic-00000000-0000-0000-0000-000000000001"
        );
    }

    #[test]
    fn topic_id_new_is_unique() {
        let a = TopicId::new();
        let b = TopicId::new();
        assert_ne!(a, b);
        assert_ne!(a, TopicId::general());
    }

    #[test]
    fn topic_id_serializes_transparent() {
        let id = TopicId::general();
        let json = serde_json::to_string(&id).unwrap();
        // transparent => 直接是字符串形式，不带 wrapper
        assert_eq!(json, "\"00000000-0000-0000-0000-000000000001\"");
        let back: TopicId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn topic_meta_round_trip_through_json() {
        let m = TopicMeta::new("周末画画");
        let json = serde_json::to_string(&m).unwrap();
        let back: TopicMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn topic_meta_general_is_stable() {
        let g1 = TopicMeta::general();
        let g2 = TopicMeta::general();
        assert_eq!(g1, g2);
        assert_eq!(g1.id, TopicId::general());
        assert_eq!(g1.title, "general");
        assert!(!g1.is_archived());
    }

    /// 反公理：老 json（Phase 1 之前的 meta，或将来 client 故意省略可选字段）
    /// 必须能反序列化得 pinned=false / archived_at=None。
    /// CLAUDE.md 公理：加字段必默认兼容老持久化。
    #[test]
    fn topic_meta_deserializes_legacy_without_pin_or_archive() {
        let legacy = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "title": "general",
            "created_at": "2026-04-19T00:00:00Z",
            "last_active_at": "2026-04-19T00:00:00Z"
        }"#;
        let m: TopicMeta =
            serde_json::from_str(legacy).expect("legacy meta 缺 pinned/archived_at 应能反序列化");
        assert_eq!(m.id, TopicId::general());
        assert!(!m.pinned);
        assert_eq!(m.archived_at, None);
    }

    #[test]
    fn topic_meta_archived_round_trip() {
        let archived_at = Utc::now();
        let m = TopicMeta {
            id: TopicId::new(),
            title: "已结束的".into(),
            created_at: archived_at,
            last_active_at: archived_at,
            pinned: false,
            archived_at: Some(archived_at),
        };
        let json = serde_json::to_string(&m).unwrap();
        let back: TopicMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
        assert!(back.is_archived());
    }
}

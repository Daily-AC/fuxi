//! `/api/roles` —— PWA「更多 → 角色」页数据源。
//!
//! 扫 `roles/<name>/ROLE.md`：每个子目录有 ROLE.md 就算一张角色卡。
//! 解析 frontmatter（`name` / `description` / `metadata.role` / `metadata.tier` /
//! `allowed-tools`）做 一句话 + 标签 展示；body 不读（用户感兴趣再点详情，
//! 当前 v1 仅列卡，详情走二期）。
//!
//! frontmatter 解析手写而不引 yaml crate——只取顶层 string 字段 + `metadata`
//! 嵌套对象的若干 key，用 split + trim 够用。fuxi-skills 那边 ROLE.md 解析也是
//! 类似手法（见 `fuxi-skills/src/loader.rs`），保持一致风格。

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{Error, Result};
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RoleCardView {
    /// 目录名 = role 标识。
    pub id: String,
    /// frontmatter `name`，缺则 fallback 到 id。
    pub name: String,
    /// frontmatter `description`——一句话定位。
    pub description: String,
    /// frontmatter `metadata.tier`（worker / memory / skillsmith / ...）。
    pub tier: Option<String>,
    /// frontmatter `metadata.cli`（claude-code / codex / 空）——
    /// 多 CLI 形态时让用户能识别本卡靠哪条门客线。
    pub cli: Option<String>,
    /// frontmatter `allowed-tools` 原始 string——卡片底部小字摘要。
    pub allowed_tools: Option<String>,
    /// 与 ROLE.md 同级是否有 instructions/examples/resources 子目录——
    /// 给详情页（v2）做 entry 提示用。
    pub has_instructions: bool,
    pub has_examples: bool,
    pub has_resources: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RolesResponse {
    pub roles: Vec<RoleCardView>,
}

/// `GET /api/roles` —— 列 roles_root 下所有 `<name>/ROLE.md`。
pub async fn list(State(state): State<AppState>) -> Result<Json<RolesResponse>> {
    let root = state
        .roles_root
        .as_ref()
        .ok_or_else(|| Error::Unavailable("roles 目录未注入".into()))?;
    let roles = scan_roles(root).map_err(|e| Error::Internal(format!("scan roles: {e}")))?;
    Ok(Json(RolesResponse { roles }))
}

/// 同步扫盘——roles 数量 < 20，cold cache 也是 ms 级，不必 spawn_blocking。
fn scan_roles(root: &Path) -> std::io::Result<Vec<RoleCardView>> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let role_md = p.join("ROLE.md");
        if !role_md.is_file() {
            continue;
        }
        let id = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }
        let body = std::fs::read_to_string(&role_md)?;
        let mut card = parse_frontmatter(&body);
        card.id = id.clone();
        if card.name.is_empty() {
            card.name = id;
        }
        card.has_instructions = p.join("instructions").is_dir();
        card.has_examples = p.join("examples").is_dir();
        card.has_resources = p.join("resources").is_dir();
        out.push(card);
    }
    // 按 id 字母序——稳定顺序方便测试 + UI 比较。
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// 极简 frontmatter parser：找 `---\n...\n---` 块，按行抓 key: value 与 nested
/// `metadata.<k>: v`。多行 value / list / 复杂嵌套不支持——roles 目前 ROLE.md
/// 顶部都是平的，足够。
fn parse_frontmatter(body: &str) -> RoleCardView {
    let mut card = RoleCardView::default();
    let trimmed = body.trim_start();
    if !trimmed.starts_with("---") {
        return card;
    }
    let after = &trimmed[3..]; // skip leading ---
    let end = match after.find("\n---") {
        Some(i) => i,
        None => return card,
    };
    let block = &after[..end];

    let mut in_metadata = false;
    for raw in block.lines() {
        // metadata: 块开始 → 之后行带前导缩进的 `  key: val` 视为 nested
        let trimmed_line = raw.trim_end();
        if trimmed_line.is_empty() {
            continue;
        }
        // 顶层（无前导空格）的 key: val
        if !raw.starts_with(' ') && !raw.starts_with('\t') {
            in_metadata = false;
            if let Some((k, v)) = split_kv(trimmed_line) {
                let v = strip_quotes(v.trim());
                match k.trim() {
                    "name" => card.name = v.to_string(),
                    "description" => card.description = v.to_string(),
                    "allowed-tools" => card.allowed_tools = Some(v.to_string()),
                    "metadata" => {
                        in_metadata = true;
                    }
                    _ => {}
                }
            }
            continue;
        }
        // metadata 嵌套：处于 in_metadata 才取
        if in_metadata && let Some((k, v)) = split_kv(trimmed_line.trim_start()) {
            let v = strip_quotes(v.trim());
            match k.trim() {
                "tier" => card.tier = Some(v.to_string()),
                "cli" => card.cli = Some(v.to_string()),
                _ => {}
            }
        }
    }
    card
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let i = line.find(':')?;
    let (k, rest) = line.split_at(i);
    Some((k, &rest[1..]))
}

fn strip_quotes(s: &str) -> &str {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn write_role(root: &Path, name: &str, body: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ROLE.md"), body).unwrap();
    }

    async fn build_app(roles_root: std::path::PathBuf) -> Router {
        let dir = tempfile::tempdir().unwrap();
        let bus = EventBus::with_memory_store().await.unwrap();
        let ws = Arc::new(fuxi_workspace::GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi).with_roles_root(roles_root);
        // tempdir 拿 router，让外层调用方持久 _dir 防 drop
        std::mem::forget(dir);
        Router::new()
            .route("/api/roles", get(list))
            .with_state(state)
    }

    #[tokio::test]
    async fn list_returns_card_per_role_subdir_with_role_md() {
        let tmp = TempDir::new().unwrap();
        write_role(
            tmp.path(),
            "luban",
            "---\n\
             name: luban\n\
             description: 工匠门客\n\
             metadata:\n  \
               tier: worker\n  \
               cli: claude-code\n\
             allowed-tools: Read Write Edit Bash\n\
             ---\n\n\
             # 鲁班\n",
        );
        write_role(
            tmp.path(),
            "extractor",
            "---\nname: extractor\ndescription: 抽取器\nmetadata:\n  tier: memory\n---\n",
        );
        // 既无 ROLE.md 子目录应该忽略
        std::fs::create_dir_all(tmp.path().join("nope")).unwrap();

        let app = build_app(tmp.path().to_path_buf()).await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/roles")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
        let body: RolesResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.roles.len(), 2);
        let luban = body.roles.iter().find(|r| r.id == "luban").unwrap();
        assert_eq!(luban.name, "luban");
        assert_eq!(luban.description, "工匠门客");
        assert_eq!(luban.tier.as_deref(), Some("worker"));
        assert_eq!(luban.cli.as_deref(), Some("claude-code"));
        assert_eq!(luban.allowed_tools.as_deref(), Some("Read Write Edit Bash"));
    }

    #[tokio::test]
    async fn missing_frontmatter_falls_back_to_id_as_name() {
        let tmp = TempDir::new().unwrap();
        write_role(tmp.path(), "raw", "# 无 frontmatter 的 role\n");
        let app = build_app(tmp.path().to_path_buf()).await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/roles")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
        let body: RolesResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.roles.len(), 1);
        assert_eq!(body.roles[0].id, "raw");
        assert_eq!(body.roles[0].name, "raw");
        assert!(body.roles[0].description.is_empty());
    }

    #[tokio::test]
    async fn detects_instructions_examples_resources_dirs() {
        let tmp = TempDir::new().unwrap();
        write_role(tmp.path(), "luban", "---\nname: luban\n---\n");
        std::fs::create_dir_all(tmp.path().join("luban").join("instructions")).unwrap();
        std::fs::create_dir_all(tmp.path().join("luban").join("examples")).unwrap();
        let app = build_app(tmp.path().to_path_buf()).await;
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/roles")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(resp.into_body(), 1024 * 64).await.unwrap();
        let body: RolesResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(body.roles[0].has_instructions);
        assert!(body.roles[0].has_examples);
        assert!(!body.roles[0].has_resources);
    }

    #[tokio::test]
    async fn handler_returns_503_when_roles_root_not_set() {
        let dir = TempDir::new().unwrap();
        let bus = EventBus::with_memory_store().await.unwrap();
        let ws = Arc::new(fuxi_workspace::GitWorktreeWorkspace::with_default_base(
            dir.path().to_path_buf(),
        ));
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi);
        let app = Router::new()
            .route("/api/roles", get(list))
            .with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/roles")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}

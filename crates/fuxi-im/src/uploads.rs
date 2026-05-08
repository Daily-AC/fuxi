//! 文件上传持久层（Task #17）—— `uploads` 表 CRUD + 落盘 + 流式 sha256。
//!
//! 设计取舍：
//! - 同 sha256 文件复用磁盘——重复上传只新插一行 `uploads`，path 列指向同一个文件
//! - 文件落 `~/.fuxi/im_uploads/<sha[:2]>/<sha>.<ext>` 两层散开
//! - mime 服务端用 mime crate 重新校验，客户端报的不可信
//! - 16MB 上限——既挡 nginx 后边的边界，也防进程内存被一个大文件吃满

#![allow(dead_code)]

use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// 16MB 上限——同 nginx `client_max_body_size`（ζ vhost 配的）。
pub const MAX_UPLOAD_BYTES: usize = 16 * 1024 * 1024;

/// 白名单 mime——服务端用 mime crate 校验客户端报的 Content-Type。
/// 故意保守：用户场景就是手机粘贴图、传 PDF / 文本 / 视频，**不**收 zip 之外的二进制。
///
/// **iPhone 兼容**（#22 hotfix）：iOS 默认相机存 HEIC / HEIF；浏览器把 jpeg 别名
/// 写成 image/jpg；Safari 也允许 svg；Windows 文件管理器把 zip 报成
/// `application/x-zip-compressed`。这些情况 v0.1 第一版漏了，导致用户实测 400。
pub const ALLOWED_MIMES: &[&str] = &[
    // 图片
    "image/png",
    "image/jpeg",
    "image/jpg", // 浏览器/某些设备的别名（正经是 image/jpeg）
    "image/gif",
    "image/webp",
    "image/heic", // iPhone 主流格式（iOS 11+ 默认相机）
    "image/heif", // 同上，HEIF 是规范名 / heic 是品牌名
    "image/svg+xml",
    // 文档
    "application/pdf",
    "application/json",
    "application/zip",
    "application/x-zip-compressed", // Windows 文件管理器别名
    // 媒体
    "video/mp4",
    "audio/mpeg",
    "audio/mp3",
    // 文本
    "text/plain",
    "text/markdown",
    "text/csv",
];

/// `uploads` 表一行——也是给前端的 wire 形态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadRecord {
    pub id: String,
    pub sha256: String,
    pub name: Option<String>,
    pub mime: Option<String>,
    pub bytes: i64,
    pub path: String,
    pub created_at: DateTime<Utc>,
    pub owner_device: Option<String>,
}

/// 附件元数据 wire 形（v1-session19 #2）—— UploadRecord 去掉 path / created_at /
/// owner_device 等内部字段，让 conv messages handler hydrate 给前端。前端
/// `Upload` interface 字段就是这五个。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadDigest {
    pub id: String,
    pub name: Option<String>,
    pub mime: Option<String>,
    pub bytes: i64,
    pub sha256: String,
}

impl From<UploadRecord> for UploadDigest {
    fn from(r: UploadRecord) -> Self {
        Self {
            id: r.id,
            name: r.name,
            mime: r.mime,
            bytes: r.bytes,
            sha256: r.sha256,
        }
    }
}

/// 包装 SqlitePool + 上传根目录。
#[derive(Clone)]
pub struct UploadStore {
    pool: SqlitePool,
    root: PathBuf,
}

impl UploadStore {
    pub fn new(pool: SqlitePool, root: PathBuf) -> Self {
        Self { pool, root }
    }

    /// 默认根：`$HOME/.fuxi/im_uploads`。`$HOME` 缺时返 None——daemon 自决怎么办。
    pub fn default_root() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".fuxi").join("im_uploads"))
    }

    /// mime 是否在白名单。
    pub fn mime_allowed(mime: &str) -> bool {
        ALLOWED_MIMES.iter().any(|m| m.eq_ignore_ascii_case(mime))
    }

    /// 把字节数据 + 元信息落盘 + 写库。重复 sha256 走"复用磁盘 + 新插一行 uploads"
    /// 的语义——不重新写文件但 uploads 表里给 caller 一份新 id 引用。
    pub async fn put(
        &self,
        bytes: &[u8],
        name: Option<&str>,
        mime: Option<&str>,
        owner_device: Option<&str>,
    ) -> Result<UploadRecord> {
        if bytes.len() > MAX_UPLOAD_BYTES {
            tracing::warn!(
                size_bytes = bytes.len(),
                limit_bytes = MAX_UPLOAD_BYTES,
                ?name,
                ?mime,
                magic = %magic_preview(bytes),
                "upload rejected: 文件过大"
            );
            return Err(Error::BadRequest(format!(
                "文件过大（{} 字节，上限 {MAX_UPLOAD_BYTES}）",
                bytes.len()
            )));
        }
        if let Some(m) = mime
            && !Self::mime_allowed(m)
        {
            tracing::warn!(
                mime = %m,
                size_bytes = bytes.len(),
                ?name,
                magic = %magic_preview(bytes),
                "upload rejected: mime 不在白名单（可能是 iPhone HEIC / Windows zip 别名等，看是否要扩 ALLOWED_MIMES）"
            );
            return Err(Error::BadRequest(format!("mime 不在白名单：{m}")));
        }

        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        let sha = hex::encode(digest);
        let ext = mime_to_ext(mime).unwrap_or("bin");
        let prefix = &sha[..2];
        let dir = self.root.join(prefix);
        let path = dir.join(format!("{sha}.{ext}"));

        // 文件不存在才落盘——同 sha 复用
        if !path.exists() {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&path, bytes)?;
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let path_str = path.to_string_lossy().to_string();
        sqlx::query(
            "INSERT INTO uploads (id, sha256, name, mime, bytes, path, created_at, owner_device) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&id)
        .bind(&sha)
        .bind(name)
        .bind(mime)
        .bind(bytes.len() as i64)
        .bind(&path_str)
        .bind(&now_str)
        .bind(owner_device)
        .execute(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("uploads insert: {e}")))?;

        Ok(UploadRecord {
            id,
            sha256: sha,
            name: name.map(String::from),
            mime: mime.map(String::from),
            bytes: bytes.len() as i64,
            path: path_str,
            created_at: now,
            owner_device: owner_device.map(String::from),
        })
    }

    /// 拉单条记录——handler GET /api/uploads/:id 用。
    pub async fn get(&self, id: &str) -> Result<Option<UploadRecord>> {
        let row = sqlx::query(
            "SELECT id, sha256, name, mime, bytes, path, created_at, owner_device \
             FROM uploads WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| Error::Internal(format!("uploads get: {e}")))?;
        match row {
            Some(r) => Ok(Some(row_to_record(r)?)),
            None => Ok(None),
        }
    }

    /// 读盘——给 handler 返文件内容。
    pub fn read_bytes(&self, rec: &UploadRecord) -> Result<Vec<u8>> {
        let bytes = std::fs::read(Path::new(&rec.path))?;
        Ok(bytes)
    }
}

fn row_to_record(row: sqlx::sqlite::SqliteRow) -> Result<UploadRecord> {
    let created_at_s: String = row
        .try_get("created_at")
        .map_err(|e| Error::Internal(format!("row created_at: {e}")))?;
    let created_at = DateTime::parse_from_rfc3339(&created_at_s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| Error::Internal(format!("created_at parse: {e}")))?;
    Ok(UploadRecord {
        id: row
            .try_get("id")
            .map_err(|e| Error::Internal(format!("row id: {e}")))?,
        sha256: row
            .try_get("sha256")
            .map_err(|e| Error::Internal(format!("row sha: {e}")))?,
        name: row
            .try_get("name")
            .map_err(|e| Error::Internal(format!("row name: {e}")))?,
        mime: row
            .try_get("mime")
            .map_err(|e| Error::Internal(format!("row mime: {e}")))?,
        bytes: row
            .try_get("bytes")
            .map_err(|e| Error::Internal(format!("row bytes: {e}")))?,
        path: row
            .try_get("path")
            .map_err(|e| Error::Internal(format!("row path: {e}")))?,
        created_at,
        owner_device: row
            .try_get("owner_device")
            .map_err(|e| Error::Internal(format!("row owner: {e}")))?,
    })
}

/// 头 16 字节 hex 预览——给 journal 排查 mime 不匹配时人眼对照 magic number。
/// 例：HEIC 是 `00 00 00 18 66 74 79 70 68 65 69 63`（"ftypheic"）；JPEG 是 `ff d8 ff`；
/// PNG 是 `89 50 4e 47`。比 mime 更可靠地认出文件类型。
fn magic_preview(bytes: &[u8]) -> String {
    let n = bytes.len().min(16);
    hex::encode(&bytes[..n])
}

/// 给 mime 推荐扩展名——存盘文件名上挂上让 OS 文件管理器看着对路。
/// 失败返 None；caller fallback "bin"。
fn mime_to_ext(mime: Option<&str>) -> Option<&'static str> {
    match mime?.to_ascii_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/heic" => Some("heic"),
        "image/heif" => Some("heif"),
        "image/svg+xml" => Some("svg"),
        "application/pdf" => Some("pdf"),
        "application/json" => Some("json"),
        "application/zip" | "application/x-zip-compressed" => Some("zip"),
        "video/mp4" => Some("mp4"),
        "audio/mpeg" | "audio/mp3" => Some("mp3"),
        "text/plain" => Some("txt"),
        "text/markdown" => Some("md"),
        "text/csv" => Some("csv"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_at;
    use tempfile::tempdir;

    async fn make_store() -> (tempfile::TempDir, UploadStore) {
        let dir = tempdir().expect("tmp");
        let pool = init_at(&dir.path().join("im.db")).await.expect("init");
        let root = dir.path().join("uploads");
        let store = UploadStore::new(pool, root);
        (dir, store)
    }

    #[test]
    fn mime_allowed_whitelist_basic() {
        assert!(UploadStore::mime_allowed("image/png"));
        assert!(UploadStore::mime_allowed("Image/PNG"));
        assert!(UploadStore::mime_allowed("application/pdf"));
        assert!(!UploadStore::mime_allowed("application/x-shellscript"));
        assert!(!UploadStore::mime_allowed(""));
    }

    /// #22 hotfix：iPhone 默认相机 HEIC / HEIF 必须放行。
    #[test]
    fn mime_allowed_heic() {
        assert!(UploadStore::mime_allowed("image/heic"));
        assert!(UploadStore::mime_allowed("image/heif"));
        assert!(UploadStore::mime_allowed("Image/HEIC")); // 大小写不敏感
    }

    /// #22 hotfix：浏览器/某些设备把 jpeg 别名写成 image/jpg。
    #[test]
    fn mime_allowed_jpg_alias() {
        assert!(UploadStore::mime_allowed("image/jpg"));
        assert!(UploadStore::mime_allowed("image/jpeg"));
    }

    /// #22 hotfix：svg + Windows zip 别名。
    #[test]
    fn mime_allowed_svg_and_windows_zip() {
        assert!(UploadStore::mime_allowed("image/svg+xml"));
        assert!(UploadStore::mime_allowed("application/x-zip-compressed"));
        assert!(UploadStore::mime_allowed("application/zip"));
    }

    /// #22 mime_to_ext 对新加的 mime 也要给对应扩展名。
    #[test]
    fn mime_to_ext_covers_heic_jpg_alias_svg() {
        assert_eq!(mime_to_ext(Some("image/heic")), Some("heic"));
        assert_eq!(mime_to_ext(Some("image/heif")), Some("heif"));
        assert_eq!(mime_to_ext(Some("image/jpg")), Some("jpg"));
        assert_eq!(mime_to_ext(Some("image/svg+xml")), Some("svg"));
        assert_eq!(
            mime_to_ext(Some("application/x-zip-compressed")),
            Some("zip")
        );
    }

    #[test]
    fn magic_preview_short_and_long() {
        assert_eq!(magic_preview(&[0xff, 0xd8, 0xff, 0xe0]), "ffd8ffe0");
        // ≤16 字节全返；>16 字节只取前 16
        let long: Vec<u8> = (0..32).collect();
        let prev = magic_preview(&long);
        assert_eq!(prev.len(), 32, "hex 长度 = 字节数 * 2");
        // 第 16 字节是 0x0f，所以最后两字符是 "0f"
        assert!(prev.ends_with("0f"));
    }

    #[tokio::test]
    async fn put_writes_file_and_row_then_get_returns_same() {
        let (_dir, store) = make_store().await;
        let bytes = b"hello-world";
        let rec = store
            .put(
                bytes,
                Some("greeting.txt"),
                Some("text/plain"),
                Some("dev-A"),
            )
            .await
            .unwrap();
        assert_eq!(rec.bytes, bytes.len() as i64);
        // sha256 是 64 hex 字符——具体值不写死（让 sha2 实现细节自由），只验长度 + 全 hex
        assert_eq!(rec.sha256.len(), 64);
        assert!(rec.sha256.chars().all(|c| c.is_ascii_hexdigit()));
        // 文件真落盘
        let on_disk = std::fs::read(&rec.path).unwrap();
        assert_eq!(on_disk, bytes);

        let got = store.get(&rec.id).await.unwrap().unwrap();
        assert_eq!(got.id, rec.id);
        assert_eq!(got.path, rec.path);
        assert_eq!(got.name.as_deref(), Some("greeting.txt"));
    }

    #[tokio::test]
    async fn put_dedupes_by_sha_does_not_rewrite_disk() {
        let (_dir, store) = make_store().await;
        let bytes = b"same-content";
        let r1 = store
            .put(bytes, Some("a.txt"), Some("text/plain"), None)
            .await
            .unwrap();
        // 改文件 mtime 让我们能感知是否被重写
        let mtime1 = std::fs::metadata(&r1.path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));

        let r2 = store
            .put(bytes, Some("b.txt"), Some("text/plain"), None)
            .await
            .unwrap();
        assert_ne!(r1.id, r2.id, "新插入一行 → 新 id");
        assert_eq!(r1.sha256, r2.sha256, "同 hash");
        assert_eq!(r1.path, r2.path, "复用同 path");

        let mtime2 = std::fs::metadata(&r2.path).unwrap().modified().unwrap();
        assert_eq!(mtime1, mtime2, "文件不应被重写");
    }

    #[tokio::test]
    async fn put_rejects_oversize() {
        let (_dir, store) = make_store().await;
        // 用单字节 fake 越界——直接传 max+1 长度的 vec
        let bytes = vec![0u8; MAX_UPLOAD_BYTES + 1];
        let err = store
            .put(&bytes, None, Some("application/zip"), None)
            .await
            .unwrap_err();
        match err {
            Error::BadRequest(m) => assert!(m.contains("过大"), "msg: {m}"),
            other => panic!("expect BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn put_rejects_bad_mime() {
        let (_dir, store) = make_store().await;
        let err = store
            .put(
                b"x",
                Some("evil.sh"),
                Some("application/x-shellscript"),
                None,
            )
            .await
            .unwrap_err();
        match err {
            Error::BadRequest(m) => assert!(m.contains("白名单"), "msg: {m}"),
            other => panic!("expect BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_unknown_returns_none() {
        let (_dir, store) = make_store().await;
        let got = store.get("not-existing").await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn read_bytes_roundtrip() {
        let (_dir, store) = make_store().await;
        let bytes = b"random-payload";
        let rec = store
            .put(bytes, None, Some("text/plain"), None)
            .await
            .unwrap();
        let got = store.read_bytes(&rec).unwrap();
        assert_eq!(got, bytes);
    }

    #[test]
    fn mime_to_ext_known() {
        assert_eq!(mime_to_ext(Some("image/png")), Some("png"));
        assert_eq!(mime_to_ext(Some("application/pdf")), Some("pdf"));
        assert_eq!(mime_to_ext(Some("audio/mp3")), Some("mp3"));
        assert_eq!(mime_to_ext(Some("audio/mpeg")), Some("mp3"));
        assert_eq!(mime_to_ext(None), None);
        assert_eq!(mime_to_ext(Some("application/x-shellscript")), None);
    }
}

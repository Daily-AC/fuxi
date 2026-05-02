//! 文件级交付产物（Decision 22 phase 1）—— per-project per-task 的「门客交活给
//! 用户」物质载体。
//!
//! 落地约定：
//! - 路径：`<projects_root>/<project>/deliverables/<task-id>/`
//! - 每条 deliverable 一个 `manifest.json` + 携带的若干文件
//! - 跟 sandbox / ephemeral **解耦**：sandbox GC 后 deliverables 仍在
//!
//! 本模块只负责物理存储 + manifest 写盘 + sha256 计算；事件发布
//! （`DeliverableProduced` 等）由 orchestrator 在调用前后包装，保持 workspace
//! crate 不依赖 fuxi-events。

use chrono::{DateTime, Utc};
use fuxi_core::{DeliverableFileMeta, DeliverableKind, ProjectId, TaskId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, info};

use crate::WorkspaceError;

/// 单个 deliverable 的句柄——返回给调用方让其知道东西落在哪、如何引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliverableHandle {
    pub project: ProjectId,
    pub task: TaskId,
    pub kind: DeliverableKind,
    /// `<projects_root>/<project>/deliverables/<task-id>/` 绝对路径。
    pub bucket_path: PathBuf,
    pub files: Vec<DeliverableFileMeta>,
}

/// 同一 task 的多次 produce_deliverable 共写一个 bucket——manifest 也合并。
///
/// 序列化形态对应 `<bucket_path>/manifest.json`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverableManifest {
    pub task: TaskId,
    pub project: ProjectId,
    /// 每次 produce 追加一条；按时间序。
    pub entries: Vec<DeliverableManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverableManifestEntry {
    pub kind: DeliverableKind,
    pub produced_at: DateTime<Utc>,
    pub files: Vec<DeliverableFileMeta>,
}

/// L3 sandbox 的姊妹：deliverables 管理器（per-project）。
///
/// 路径约定：`<projects_root>/<project>/deliverables/`。
#[derive(Debug, Clone)]
pub struct DeliverablesManager {
    project: ProjectId,
    deliverables_root: PathBuf,
}

impl DeliverablesManager {
    /// 从 project_id 和 projects_root 构造。
    pub fn new(project: ProjectId, projects_root: &Path) -> Self {
        let deliverables_root = projects_root.join(project.as_str()).join("deliverables");
        Self {
            project,
            deliverables_root,
        }
    }

    pub fn project(&self) -> &ProjectId {
        &self.project
    }

    pub fn root(&self) -> &Path {
        &self.deliverables_root
    }

    /// 把一组源文件（绝对路径）作为 deliverable 落到 task 的 bucket。
    ///
    /// - 复制（不 move）—— sandbox 那份留着可继续编辑
    /// - 计算 sha256 + size
    /// - 写 / 追加 manifest.json
    /// - 同 task 多次 produce → manifest entries 累积，文件平铺在 bucket 下
    /// - 文件名冲突时用 `(<n>)` 后缀避错（保留两份，不覆盖原版）
    ///
    /// `target_path`（Direct 模式）目前**不实装**——v1 只走 Inbox。Direct 留待
    /// 后续 phase 加。本签名预留参数空位让 trait 演进时不破契约。
    pub async fn produce(
        &self,
        task: TaskId,
        kind: DeliverableKind,
        sources: &[PathBuf],
    ) -> Result<DeliverableHandle, WorkspaceError> {
        if sources.is_empty() {
            return Err(WorkspaceError::Other(
                "produce_deliverable: sources 不能为空".into(),
            ));
        }

        let bucket_path = self.bucket_for(task);
        fs::create_dir_all(&bucket_path).await?;

        info!(
            project = %self.project,
            %task,
            files = sources.len(),
            bucket = %bucket_path.display(),
            "produce deliverable"
        );

        // 复制 + sha256 + 防重名
        let mut file_metas = Vec::with_capacity(sources.len());
        for src in sources {
            if !src.is_file() {
                return Err(WorkspaceError::Other(format!(
                    "deliverable source 不是文件: {}",
                    src.display()
                )));
            }
            let original_name = src
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| {
                    WorkspaceError::Other(format!("source 文件名非 utf8: {}", src.display()))
                })?
                .to_string();
            let final_name = pick_non_colliding_name(&bucket_path, &original_name).await?;
            let dest = bucket_path.join(&final_name);
            let bytes = fs::read(src).await?;
            let sha256 = sha256_hex(&bytes);
            let size_bytes = bytes.len() as u64;
            fs::write(&dest, &bytes).await?;
            file_metas.push(DeliverableFileMeta {
                name: final_name,
                sha256,
                size_bytes,
            });
            debug!(
                src = %src.display(),
                dest = %dest.display(),
                sha = %file_metas.last().unwrap().sha256,
                "deliverable file copied"
            );
        }

        // manifest.json 追加
        let manifest_path = bucket_path.join("manifest.json");
        let mut manifest = if manifest_path.exists() {
            let bytes = fs::read(&manifest_path).await?;
            serde_json::from_slice::<DeliverableManifest>(&bytes).unwrap_or_else(|_| {
                // 损坏 manifest：起新的，不阻塞 produce（旧 entry 丢了但本次仍能记账）
                DeliverableManifest {
                    task,
                    project: self.project.clone(),
                    entries: Vec::new(),
                }
            })
        } else {
            DeliverableManifest {
                task,
                project: self.project.clone(),
                entries: Vec::new(),
            }
        };
        manifest.entries.push(DeliverableManifestEntry {
            kind,
            produced_at: Utc::now(),
            files: file_metas.clone(),
        });
        let json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| WorkspaceError::Other(format!("manifest 序列化失败: {e}")))?;
        fs::write(&manifest_path, json).await?;

        Ok(DeliverableHandle {
            project: self.project.clone(),
            task,
            kind,
            bucket_path,
            files: file_metas,
        })
    }

    /// 列出某 task 的全部 deliverable entries（按 produce 时间序）。
    ///
    /// 不存在 bucket → 返空 Vec（不算 Err，方便调用方拼界面）。
    pub async fn list_for_task(
        &self,
        task: TaskId,
    ) -> Result<Vec<DeliverableManifestEntry>, WorkspaceError> {
        let bucket_path = self.bucket_for(task);
        let manifest_path = bucket_path.join("manifest.json");
        if !manifest_path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&manifest_path).await?;
        let manifest: DeliverableManifest = serde_json::from_slice(&bytes)
            .map_err(|e| WorkspaceError::Other(format!("manifest 解析失败: {e}")))?;
        Ok(manifest.entries)
    }

    fn bucket_for(&self, task: TaskId) -> PathBuf {
        // task display 是 `task-<uuid>`，bucket 名直接用——既人读、又无重名风险
        self.deliverables_root.join(task.to_string())
    }
}

/// 给 bucket 内的新文件挑一个不撞名的名字。
///
/// 优先用 original；撞了就 `name (1).ext` `name (2).ext` …
async fn pick_non_colliding_name(bucket: &Path, original: &str) -> Result<String, WorkspaceError> {
    if !bucket.join(original).exists() {
        return Ok(original.to_string());
    }
    // split off extension
    let (stem, ext) = match original.rsplit_once('.') {
        Some((s, e)) => (s, format!(".{e}")),
        None => (original, String::new()),
    };
    for n in 1..=999 {
        let candidate = format!("{stem} ({n}){ext}");
        if !bucket.join(&candidate).exists() {
            return Ok(candidate);
        }
    }
    Err(WorkspaceError::Other(format!(
        "{original} 撞名超过 999 次，bucket {} 有问题",
        bucket.display()
    )))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_id() -> ProjectId {
        ProjectId::new("erp").unwrap()
    }

    /// 小工具：写一个临时源文件返回路径。
    async fn mk_source(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        tokio::fs::write(&p, content).await.unwrap();
        p
    }

    #[tokio::test]
    async fn produce_writes_files_and_manifest() {
        let projects_root = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let mgr = DeliverablesManager::new(make_id(), projects_root.path());

        let task = TaskId::new();
        let f1 = mk_source(src_dir.path(), "report.md", "# 报告\n内容").await;
        let f2 = mk_source(src_dir.path(), "data.csv", "a,b\n1,2").await;

        let h = mgr
            .produce(
                task,
                DeliverableKind::ResearchSummary,
                &[f1.clone(), f2.clone()],
            )
            .await
            .expect("produce");

        // bucket 路径正确
        assert!(h.bucket_path.is_dir());
        assert!(h.bucket_path.ends_with(task.to_string()));
        // 文件落地
        assert!(h.bucket_path.join("report.md").is_file());
        assert!(h.bucket_path.join("data.csv").is_file());
        // sha256 算了 + size 对
        assert_eq!(h.files.len(), 2);
        assert!(h.files.iter().all(|f| f.size_bytes > 0));
        assert!(h.files.iter().all(|f| f.sha256.len() == 64)); // sha256 hex = 64 字符
        // manifest.json 落了
        let manifest_path = h.bucket_path.join("manifest.json");
        assert!(manifest_path.is_file());
        let mf: DeliverableManifest =
            serde_json::from_slice(&tokio::fs::read(&manifest_path).await.unwrap()).unwrap();
        assert_eq!(mf.task, task);
        assert_eq!(mf.entries.len(), 1);
        assert_eq!(mf.entries[0].kind, DeliverableKind::ResearchSummary);
    }

    #[tokio::test]
    async fn produce_twice_appends_manifest_entries() {
        let projects_root = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let mgr = DeliverablesManager::new(make_id(), projects_root.path());
        let task = TaskId::new();

        let f1 = mk_source(src_dir.path(), "first.md", "1").await;
        mgr.produce(task, DeliverableKind::ResearchSummary, &[f1])
            .await
            .unwrap();

        let f2 = mk_source(src_dir.path(), "second.md", "2").await;
        mgr.produce(task, DeliverableKind::CodeChange, &[f2])
            .await
            .unwrap();

        let entries = mgr.list_for_task(task).await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, DeliverableKind::ResearchSummary);
        assert_eq!(entries[1].kind, DeliverableKind::CodeChange);
    }

    #[tokio::test]
    async fn produce_dedups_filename_with_suffix() {
        let projects_root = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let mgr = DeliverablesManager::new(make_id(), projects_root.path());
        let task = TaskId::new();

        // 同名两次产
        let a = mk_source(src_dir.path(), "report.md", "v1").await;
        mgr.produce(task, DeliverableKind::ResearchSummary, &[a])
            .await
            .unwrap();
        let b = mk_source(src_dir.path(), "report.md", "v2").await;
        let h2 = mgr
            .produce(task, DeliverableKind::ResearchSummary, &[b])
            .await
            .unwrap();

        // 第二次的文件应被自动重命名为 `report (1).md`，不覆盖原版
        assert_eq!(h2.files[0].name, "report (1).md");
        let bucket = h2.bucket_path;
        assert!(bucket.join("report.md").is_file(), "原版应保留");
        assert!(bucket.join("report (1).md").is_file(), "新版用后缀");
        let v1 = tokio::fs::read_to_string(bucket.join("report.md"))
            .await
            .unwrap();
        let v2 = tokio::fs::read_to_string(bucket.join("report (1).md"))
            .await
            .unwrap();
        assert_eq!(v1, "v1");
        assert_eq!(v2, "v2");
    }

    #[tokio::test]
    async fn list_for_task_empty_when_no_bucket() {
        let projects_root = tempfile::tempdir().unwrap();
        let mgr = DeliverablesManager::new(make_id(), projects_root.path());
        let entries = mgr.list_for_task(TaskId::new()).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn produce_rejects_empty_sources() {
        let projects_root = tempfile::tempdir().unwrap();
        let mgr = DeliverablesManager::new(make_id(), projects_root.path());
        let err = mgr
            .produce(TaskId::new(), DeliverableKind::TestResult, &[])
            .await
            .expect_err("空 sources 应拒");
        assert!(matches!(err, WorkspaceError::Other(_)));
    }

    #[tokio::test]
    async fn produce_rejects_non_file_source() {
        let projects_root = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let mgr = DeliverablesManager::new(make_id(), projects_root.path());

        // 传一个目录路径（不是文件）
        let err = mgr
            .produce(
                TaskId::new(),
                DeliverableKind::TestResult,
                &[src_dir.path().to_path_buf()],
            )
            .await
            .expect_err("目录路径应拒");
        assert!(matches!(err, WorkspaceError::Other(_)));
    }

    #[tokio::test]
    async fn sha256_is_deterministic_and_correct() {
        // 简单回归：同内容 sha256 一致
        let a = sha256_hex(b"hello");
        let b = sha256_hex(b"hello");
        assert_eq!(a, b);
        // 已知 sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            a,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}

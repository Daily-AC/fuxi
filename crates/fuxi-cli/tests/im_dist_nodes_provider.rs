//! β · #55 集成测：`DistControllerNodesProvider` 生产 impl。
//!
//! 验：fuxi-cli 包 `Arc<DistController>` 后实现 `fuxi_im::nodes_provider::NodesProvider`，
//! 注册节点 → list_nodes 返 NodeView 列表 + home 节点 workers 从 fuxi shelf 拿 +
//! 远端节点 workers 始终空（v1 限制，等 #57 dispatch routing）。

use fuxi_cli::im_dist::{DistControllerNodesProvider, build_dist_layer};
use fuxi_events::EventBus;
use fuxi_im::nodes_provider::NodesProvider;
use fuxi_orchestrator::Fuxi;
use fuxi_workspace::GitWorktreeWorkspace;
use std::sync::Arc;
use tempfile::TempDir;

async fn make_workspace() -> (TempDir, Arc<GitWorktreeWorkspace>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().to_path_buf();
    let _ = tokio::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(&path)
        .output()
        .await;
    tokio::fs::write(path.join("README.md"), "x").await.unwrap();
    let _ = tokio::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&path)
        .output()
        .await;
    let _ = tokio::process::Command::new("git")
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-qm",
            "init",
        ])
        .current_dir(&path)
        .output()
        .await;
    let ws = Arc::new(GitWorktreeWorkspace::with_default_base(path));
    (dir, ws)
}

#[tokio::test]
async fn provider_returns_home_node_after_self_register() {
    unsafe {
        std::env::remove_var("FUXI_DIST_HMAC_SECRET");
        std::env::remove_var("FUXI_DIST_TOKEN");
    }
    let dist_dir = TempDir::new().expect("dist tmp");
    let bus = EventBus::with_memory_store().await.expect("bus");
    let (_ws_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let layer = build_dist_layer(dist_dir.path(), bus)
        .await
        .expect("build dist layer");
    let provider = DistControllerNodesProvider::new(layer.ctrl);

    let nodes = provider.list_nodes(&fuxi).await;
    assert_eq!(nodes.len(), 1, "home 节点已自注册");
    assert_eq!(nodes[0].node_id, "home");
    assert!(nodes[0].online, "刚 register 应 online");
    assert_eq!(nodes[0].max_concurrency, 4);
    assert!(nodes[0].tags.contains(&"home".to_string()));
    // home 节点没真 worker（fuxi shelf 空）→ workers 也空
    assert!(nodes[0].workers.is_empty());
}

#[tokio::test]
async fn provider_returns_remote_node_with_empty_workers_v1() {
    unsafe {
        std::env::remove_var("FUXI_DIST_HMAC_SECRET");
        std::env::remove_var("FUXI_DIST_TOKEN");
    }
    let dist_dir = TempDir::new().expect("dist tmp");
    let bus = EventBus::with_memory_store().await.expect("bus");
    let (_ws_dir, ws) = make_workspace().await;
    let fuxi = Fuxi::new(bus.clone(), ws);

    let layer = build_dist_layer(dist_dir.path(), bus)
        .await
        .expect("build dist layer");
    // 注册一个远端节点（模拟 mac-local worker register 上来）
    layer
        .ctrl
        .register(
            "mac-local".to_string(),
            vec!["local".to_string(), "mac".to_string()],
            2,
        )
        .await;

    let provider = DistControllerNodesProvider::new(layer.ctrl);
    let nodes = provider.list_nodes(&fuxi).await;
    assert_eq!(nodes.len(), 2, "home + mac-local");
    let mac = nodes
        .iter()
        .find(|n| n.node_id == "mac-local")
        .expect("mac-local 节点应存在");
    assert!(mac.online);
    assert_eq!(mac.max_concurrency, 2);
    assert!(mac.tags.contains(&"local".to_string()));
    // β · #55 v1 已知缺口：远端节点 workers 始终空，等 #57 dispatch routing
    assert!(
        mac.workers.is_empty(),
        "远端节点 v1 workers 应空（dist 协议无 agent metadata）"
    );
}

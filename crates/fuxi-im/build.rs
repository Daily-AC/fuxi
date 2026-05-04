//! bug #77 · build-time 注入 git sha + 编译时间戳，给 `/api/version` 端点用。
//!
//! PWA 启动 + 定时拉 `/api/version`，跟 localStorage 缓存版本对比；不同 →
//! 强 reload 跳过 SW 缓存，让用户立即看到新部署。
//!
//! sha 取 `git rev-parse --short HEAD`（fail 时 fallback "unknown"），
//! 时间戳是 build 时刻（rerun-if-changed 让 git HEAD 变就重 build）。

use std::process::Command;

fn main() {
    // 真因：home 部署目录是 rsync target，不是 git 仓库，`git rev-parse` fail。
    // 解：deploy-home.sh 在 rsync 前把 git sha 写到 `<workspace>/.fuxi-build-sha`，
    // 一起 rsync 到 home。build.rs 优先读该文件，缺则 fallback git，再 fallback "unknown"。
    let sha = std::fs::read_to_string("../../.fuxi-build-sha")
        .ok()
        .and_then(|s| {
            let t = s.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        })
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "--short", "HEAD"])
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        String::from_utf8(o.stdout)
                            .ok()
                            .map(|s| s.trim().to_string())
                    } else {
                        None
                    }
                })
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string());

    let build_ts = chrono::Utc::now().to_rfc3339();
    println!("cargo:rustc-env=FUXI_BUILD_SHA={sha}");
    println!("cargo:rustc-env=FUXI_BUILD_TS={build_ts}");

    // 三 rerun trigger：sha 文件 / git HEAD / git 当前 branch ref
    println!("cargo:rerun-if-changed=../../.fuxi-build-sha");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    if let Ok(content) = std::fs::read_to_string("../../.git/HEAD")
        && let Some(rest) = content.strip_prefix("ref: ")
    {
        println!("cargo:rerun-if-changed=../../.git/{}", rest.trim());
    }
}

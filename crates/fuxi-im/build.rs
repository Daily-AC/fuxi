//! bug #77 · build-time 注入 git sha + 编译时间戳，给 `/api/version` 端点用。
//!
//! PWA 启动 + 定时拉 `/api/version`，跟 localStorage 缓存版本对比；不同 →
//! 强 reload 跳过 SW 缓存，让用户立即看到新部署。
//!
//! sha 取 `git rev-parse --short HEAD`（fail 时 fallback "unknown"），
//! 时间戳是 build 时刻（rerun-if-changed 让 git HEAD 变就重 build）。

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let build_ts = chrono::Utc::now().to_rfc3339();
    println!("cargo:rustc-env=FUXI_BUILD_SHA={sha}");
    println!("cargo:rustc-env=FUXI_BUILD_TS={build_ts}");

    // git HEAD 变就重 build 拉新 sha；其它源码改动 cargo 已自跟踪
    let head_path = ".git/HEAD";
    println!("cargo:rerun-if-changed={head_path}");
    if let Ok(content) = std::fs::read_to_string(head_path)
        && let Some(rest) = content.strip_prefix("ref: ")
    {
        let ref_path = format!(".git/{}", rest.trim());
        println!("cargo:rerun-if-changed={ref_path}");
    }
}

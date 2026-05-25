//! fuxi-wake-server bin 入口。
//!
//! 用法：
//!   fuxi-wake-server [--bind 0.0.0.0:9101] [--mock] [--token-file <path>]
//!                    [--work-dir <path>] [--keywords 玄女,贾维斯]
//!
//! `--mock` 或 `FUXI_WAKE_MOCK=1` 时用 MockEngine（不依赖讯飞 SDK）。
//!
//! 非 mock 模式：从 ENV 读 `FUXI_XFYUN_APPID/API_KEY/API_SECRET`，
//! 进程启动期一次性 `init_process`（AIKIT_Init/EngineInit/LoadData）；
//! 每个 WS 连接持一份 `XfyunEngine` 跑 Start/Write/End。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::net::TcpListener;
use tracing::{info, warn};

use fuxi_wake_server::engine::WakeEngine;
use fuxi_wake_server::engine::mock::MockEngine;
use fuxi_wake_server::engine::xfyun::{ProcessInitParams, XfyunEngine};
use fuxi_wake_server::{AppState, router};

#[derive(Parser, Debug)]
#[command(name = "fuxi-wake-server", about = "伏羲家用唤醒守护")]
struct Cli {
    /// 监听地址；默认 0.0.0.0:9101（与 fuxi-im 9100 错开）。
    #[arg(long, default_value = "0.0.0.0:9101")]
    bind: SocketAddr,

    /// 用 MockEngine（不依赖讯飞 SDK，30s 触发一次玄女）。
    #[arg(long, env = "FUXI_WAKE_MOCK")]
    mock: bool,

    /// token 文件路径——默认 `~/.fuxi/wake.token`，ENV `FUXI_WAKE_TOKEN_FILE` 覆盖。
    #[arg(long, env = "FUXI_WAKE_TOKEN_FILE")]
    token_file: Option<PathBuf>,

    /// 讯飞 SDK 工作目录——license 落盘 + keywords 文件存放。
    /// 默认 `~/.fuxi/wake/`；systemd 部署建议改 `/var/lib/fuxi-wake/`（持久 + 可写）。
    #[arg(long, env = "FUXI_WAKE_WORK_DIR")]
    work_dir: Option<PathBuf>,

    /// 关键词列表——逗号分隔。默认 `玄女`。
    #[arg(long, env = "FUXI_WAKE_KEYWORDS", value_delimiter = ',', default_values_t = vec!["玄女".to_string()])]
    keywords: Vec<String>,

    /// Phase 5-B SV：声纹验证服务地址。设了就在 IVW 命中后调 /verify，
    /// non-match 静默丢 wake event；不设 = wake 行为跟 Phase 5-A 前一致。
    /// 例：`http://127.0.0.1:9883`（home localhost）。
    #[arg(long, env = "FUXI_WAKE_SV_URL")]
    sv_url: Option<String>,

    /// SV 用的 HMAC key 文件（跟 fuxi-im / sv_server 同款）。默认 `~/.fuxi/im_hmac.key`。
    /// 仅 `--sv-url` 设置时才读；wake-server systemd User= 必须有读权限。
    #[arg(long, env = "FUXI_WAKE_SV_HMAC_KEY")]
    sv_hmac_key: Option<PathBuf>,
}

fn default_work_dir() -> Result<PathBuf> {
    let home =
        std::env::var_os("HOME").context("HOME 未设置——请用 --work-dir 指定 SDK 工作目录")?;
    Ok(PathBuf::from(home).join(".fuxi").join("wake"))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,fuxi_wake_server=debug".into()),
        )
        .init();

    let cli = Cli::parse();

    let token_path = match cli.token_file {
        Some(p) => p,
        None => fuxi_wake_server::auth::default_token_path()?,
    };
    let token = fuxi_wake_server::auth::load_token(&token_path)
        .with_context(|| format!("读取 token 失败：{}", token_path.display()))?;
    info!(token_path = %token_path.display(), "wake-server: token loaded");

    let state = if cli.mock {
        info!("wake-server: 启用 MockEngine（30s 间隔触发玄女）");
        AppState::new(token, || Box::new(MockEngine::new()) as Box<dyn WakeEngine>)
    } else {
        let app_id =
            std::env::var("FUXI_XFYUN_APPID").context("非 mock 模式必须设 FUXI_XFYUN_APPID")?;
        let api_key =
            std::env::var("FUXI_XFYUN_API_KEY").context("非 mock 模式必须设 FUXI_XFYUN_API_KEY")?;
        let api_secret = std::env::var("FUXI_XFYUN_API_SECRET")
            .context("非 mock 模式必须设 FUXI_XFYUN_API_SECRET")?;
        let work_dir = match cli.work_dir.clone() {
            Some(p) => p,
            None => default_work_dir()?,
        };

        info!(
            %app_id,
            work_dir = %work_dir.display(),
            keywords = ?cli.keywords,
            "wake-server: 进程级 xfyun init"
        );
        let params = ProcessInitParams {
            app_id,
            api_key,
            api_secret,
            work_dir,
            keywords: cli.keywords.clone(),
        };
        // init_process 失败：非 Linux x86_64 构建会返 stub 报错；Linux 上 SDK 不可用
        // 也会失败——把错抛出来让 systemd Restart 走重试。
        fuxi_wake_server::engine::xfyun::init_process(params)
            .context("xfyun init_process 失败：检查 ENV / SDK / 网络")?;
        info!("wake-server: xfyun init_process ok，启用 XfyunEngine");

        AppState::new(token, || {
            Box::new(XfyunEngine::new()) as Box<dyn WakeEngine>
        })
    };

    // Phase 5-B SV：仅在 --sv-url 显式设了时启用。HMAC key 默认 ~/.fuxi/im_hmac.key
    // —— 跟 fuxi-im / sv_server.py 同款，确保 token 互验通过。fail-open 由
    // server.rs 命中分支兜底。
    let state = match cli.sv_url {
        Some(url) => {
            let key_path = cli
                .sv_hmac_key
                .clone()
                .or_else(|| {
                    std::env::var_os("HOME")
                        .map(|h| PathBuf::from(h).join(".fuxi").join("im_hmac.key"))
                })
                .context("HOME 未设置 + 没显式 --sv-hmac-key——无法定位 HMAC key")?;
            info!(%url, key_path = %key_path.display(), "wake-server: 启用 SV 拒陌生人");
            let client = fuxi_wake_server::sv::SvClient::from_key_file(url, &key_path)
                .context("加载 SV HMAC key 失败")?;
            state.with_sv(fuxi_wake_server::sv::SvConfig {
                client: Arc::new(client),
            })
        }
        None => {
            warn!("wake-server: 未配 --sv-url，任何人喊「玄女」都会触发 wake");
            state
        }
    };

    let app = router(Arc::new(state));
    let listener = TcpListener::bind(cli.bind)
        .await
        .with_context(|| format!("bind {} 失败", cli.bind))?;
    info!(addr = %cli.bind, "wake-server: serving");

    let serve_result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await;

    // 退出时 best-effort 释放讯飞资源（mock 路径不会动 PROCESS_INIT，是 noop）。
    fuxi_wake_server::engine::xfyun::shutdown_process();

    if let Err(e) = serve_result {
        warn!(error = ?e, "wake-server: axum serve 退出");
        return Err(anyhow::anyhow!("axum serve 失败: {e}"));
    }
    Ok(())
}

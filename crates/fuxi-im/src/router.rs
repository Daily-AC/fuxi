//! 路由装配——按 `docs/decisions/14-im-mobile-frontend.md` C 节路由表挂齐。
//!
//! 设计取舍：现在 stub 也挂上路径 + 501，**不留 404 给未实装端点**。
//! 理由——前端开发期间能区分"端点不存在（路由错）" vs "端点未实装（owner 还没接）"。
//! 实装到位的 owner（β/γ/δ/ε）改对应 handler 即可，不动这份装配。
//!
//! 静态 PWA 资源（Decision 14 表中的 `GET /` `include_dir!`）由 ε 接管时
//! 在 `lib::router` 里 `.fallback_service` 挂——本文件只管 `/api` + `/healthz`。
//!
//! ## 鉴权 layer
//!
//! `cookie_auth_layer` 必须挂在 router 上才能拦请求——单独写好但没挂等于裸奔。
//! 历史 bug（2026-04-26 用户实测撞上）：`build()` 之前只 `.layer(TraceLayer)`，
//! 鉴权 layer 写好了但**从未集成**，所有 `/api/*` 都能无 cookie 直接访问。
//! 修复 = 这里挂上 + `tests/router_auth_integration.rs` 走 Router 真请求测试做门禁。
//! 隔离单测（直接调 layer fn）抓不到此类"忘记挂上"的 bug。

use crate::handlers;
use crate::middleware::{AuthGate, cookie_auth_layer};
use crate::state::AppState;
use crate::uploads::MAX_UPLOAD_BYTES;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use tower_http::trace::TraceLayer;

/// `/api/upload` 单请求 body 上限——
/// 等于 [`MAX_UPLOAD_BYTES`] + 256 KB slack（multipart boundary / headers / form
/// fields 开销）。axum 默认 `DefaultBodyLimit::max(2MB)` 太小，iOS Safari 上传
/// 中等图片即超 → multer `failed to read stream` 报错（#49 用户实测踩到）。
///
/// 该常量必须 ≥ nginx `client_max_body_size`，否则 nginx 放行的字节 axum 拒。
/// 当前 nginx 给 32m，本侧给 16m + slack ≈ 16.25m；nginx 仍是上游硬上限。
const UPLOAD_BODY_LIMIT_BYTES: usize = MAX_UPLOAD_BYTES + 256 * 1024;

/// 装配 IM 服务的全部路由。
///
/// 后续 owner 加新端点：在对应 `handlers/*.rs` 里实装真逻辑，本文件不动。
pub fn build(state: AppState) -> Router {
    let auth_gate = AuthGate::new(state.im_auth.secret.clone());
    Router::new()
        .route("/healthz", get(handlers::health::healthz))
        // bug #77 · build-time 版本号给前端做 cache busting
        .route("/api/version", get(handlers::health::version))
        .route("/api/tasks", get(handlers::tasks::list_tasks))
        // β · #55 dist topology——节点 tab + 任务卡 @node 标识共用数据源
        .route("/api/nodes", get(handlers::nodes::list_nodes))
        // dist topology 实时变更流——前端节点 tab 订阅，每条 push 触发 refetch
        // /api/nodes，反应式刷 home/远端 worker 心跳和 inflight 数。
        .route("/api/nodes/stream", get(handlers::nodes::nodes_stream_ws))
        // Decision 21 phase 1：Project 注册表读 + 写
        .route(
            "/api/projects",
            get(handlers::projects::list_projects).post(handlers::projects::add_project),
        )
        .route(
            "/api/projects/{id}",
            axum::routing::delete(handlers::projects::remove_project),
        )
        .route(
            "/api/projects/{id}/sandboxes",
            get(handlers::projects::list_sandboxes),
        )
        // Decision 21 phase 3：L2 ephemeral 工作区列表（active + archived）
        .route(
            "/api/projects/{id}/ephemeral",
            get(handlers::projects::list_ephemeral),
        )
        // Decision 22 phase 1：交付收件箱列表 + 文件下载
        .route(
            "/api/deliverables",
            get(handlers::deliverables::list_deliverables),
        )
        .route(
            "/api/deliverables/{project}/{task}/files/{name}",
            get(handlers::deliverables::download_file),
        )
        // Decision 22 phase 3 ext：inline 预览端点（不设 attachment）。
        // 与 /files/ 二选一——前端预览走 /preview/，用户主动下载走 /files/。
        .route(
            "/api/deliverables/{project}/{task}/preview/{name}",
            get(handlers::deliverables::preview_file),
        )
        // Decision 22 phase 2：用户接收 / 拒绝（POST publish 事件，phase 3 加文件 copy）
        .route(
            "/api/deliverables/{project}/{task}/accept",
            post(handlers::deliverables::accept_deliverable),
        )
        .route(
            "/api/deliverables/{project}/{task}/reject",
            post(handlers::deliverables::reject_deliverable),
        )
        // β · #56 worker onboarding——主密码鉴权派 secret/token + 静态脚本端点
        // setup-worker 路径在 cookie middleware is_exempt 已豁免（同 /api/auth/login）；
        // setup-local-worker.sh 不在 /api/* 自然过 layer，脚本本身无 secret
        .route(
            "/api/dist/setup-worker",
            post(handlers::setup_worker::setup_worker),
        )
        .route(
            "/setup-local-worker.sh",
            get(handlers::setup_worker::get_setup_script),
        )
        // β · #N6' 任务 thread 镜像端点：events 走白名单 filter，conv WS 同 filter
        // 接续。`/stream` 是 γ 早期的"任务 raw 事件流"路径，filter 仅 meta.task；
        // 保留兼容观察器，不删
        .route("/api/tasks/{id}/events", get(handlers::tasks::task_events))
        .route("/api/tasks/{id}/conv", get(handlers::tasks::task_conv_ws))
        .route(
            "/api/tasks/{id}/stream",
            get(handlers::conv::task_stream_ws),
        )
        .route("/api/conv", get(handlers::conv::conv_ws))
        // β · #17 IM 层聊天记录拉取（首屏预加载 + 翻页）
        .route("/api/conv/messages", get(handlers::conv::messages))
        // β · #27 私聊页镜像端点（重设计 #N5）—— /api/conv 同模式按 agent 过滤
        .route(
            "/api/workers/{agent_id}/events",
            get(handlers::workers::worker_events),
        )
        .route(
            "/api/workers/{agent_id}/conv",
            get(handlers::workers::worker_conv_ws),
        )
        .route("/api/intervene", post(handlers::intervene::intervene))
        .route("/api/dispatch", post(handlers::dispatch::dispatch))
        // β · #17 文件上传 / 下载
        // #49 修：DefaultBodyLimit 必须显式拉到 UPLOAD_BODY_LIMIT_BYTES——axum
        // 默认只 2MB，iOS Safari 中等图片就超，multer 报 failed to read stream。
        // 注意 layer 仅作用于此 route（用 RouterExt route_layer 模式：跟在 .route 后
        // 调 .layer 会污染所有 route，所以单独 nest）。
        .route(
            "/api/upload",
            post(handlers::upload::upload).layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT_BYTES)),
        )
        .route("/api/uploads/{id}", get(handlers::upload::get_upload))
        .route("/api/auth/login", post(handlers::auth::login))
        .route("/api/auth/pair", post(handlers::auth::pair))
        .route("/api/push/subscribe", post(handlers::push::subscribe))
        .route("/api/push/silence", post(handlers::push::silence))
        .route("/api/push/vapid-pub", get(handlers::push::vapid_public_key))
        // 鉴权 layer 在最外侧——所有 /api/* 都过它；is_exempt 内部豁免
        // /api/auth/login + /api/auth/pair + /healthz（这三条本身就是签发 cookie /
        // upstream check 的入口，自然不能要求带已有的 cookie）。
        // 非 /api/* 路径（PWA 静态资源 /、ε 的 fallback_service）也由 layer
        // is_exempt 之外的 "!path.starts_with('/api/')" 分支放行。
        .layer(from_fn_with_state(auth_gate, cookie_auth_layer))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

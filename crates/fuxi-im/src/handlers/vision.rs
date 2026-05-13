//! `/api/xuannv/look` + `/api/xuannv/look/frame` —— 玄女眼睛 v1。
//!
//! 见 `docs/superpowers/specs/2026-05-14-xuannv-vision-design.md`。
//!
//! 流程一句话：玄女 Bash `fuxi xuannv look` → CLI POST `/api/xuannv/look`
//! → handler publish `VisionRequest` 事件让桌宠订 `/api/conv` WS 听到 → 桌宠
//! 拍一帧 multipart 上传 `/look/frame` → handler 用 `request_id` 反查
//! oneshot 通知阻塞中的 `/look` → 200 返本地 path，玄女 Read(path) 看图。
//!
//! 「无桌宠连接」用 `bus.receiver_count() == 0` 近似——粗粒度但足够：
//! 0 = 必然没人在听；非 0 不保证桌宠在线，但 publish 后超时兜底（408）。

use crate::error::{Error, Result};
use crate::state::AppState;
use axum::Json;
use axum::extract::{Multipart, State};
use fuxi_core::event::{Event, EventKind, EventMeta};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::{info, warn};
use uuid::Uuid;

/// 默认 frame 等待超时——10s 跟 spec 默认值一致。
/// 桌宠正常拍帧 < 1s（getUserMedia 同 session 二次以上无弹窗）；首次屏幕授权
/// 弹窗用户可能要 10s+ 才点完，留够。
const DEFAULT_TIMEOUT_SECS: u64 = 10;
/// 硬上限——防止前端误传超大值占住 oneshot 表。30s 给 macOS 屏幕授权弹窗
/// 「点完按钮」最坏情况，再长也是用户心智断点，重新发起更合理。
const MAX_TIMEOUT_SECS: u64 = 30;

/// `POST /api/xuannv/look` 请求体。
#[derive(Debug, Deserialize)]
pub struct LookRequest {
    /// `"webcam"` | `"screen"`——v1 仅这俩，未来 `"window"` / `"region"`。
    pub target: String,
    /// 给桌宠端 toast / log 的备忘文本，可空。
    #[serde(default)]
    pub hint: Option<String>,
    /// 等帧上传的最大秒数；默认 [`DEFAULT_TIMEOUT_SECS`]。
    /// `0` / 缺省 / 超过 [`MAX_TIMEOUT_SECS`] 都 clamp 成默认值。
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// `POST /api/xuannv/look` 响应。
#[derive(Debug, Serialize)]
pub struct LookResponse {
    pub ok: bool,
    pub request_id: String,
    /// 落档绝对路径——`~/.local/share/fuxi/vision/<request_id>.<ext>`。
    /// 玄女拿到 path 立即用 cc Read 工具看图。
    pub path: PathBuf,
    /// `image/png` 或 `image/jpeg`，桌宠端上传时声明。
    pub mime: String,
    pub bytes: u64,
}

/// `POST /api/xuannv/look/frame` 响应——内部确认用。
#[derive(Debug, Serialize)]
pub struct FrameAck {
    pub ok: bool,
    pub request_id: String,
}

/// oneshot 携带的载荷：成功时 `Ok(FrameRecord)`，失败时 `Err(reason)`。
#[derive(Debug, Clone)]
pub struct FrameRecord {
    pub path: PathBuf,
    pub mime: String,
    pub bytes: u64,
}

/// 桌宠端上报的 frame 错误——目前只 `user_denied`，留枚举给后续扩。
/// 字符串挂在 multipart `error` 字段里，handler 看到非空就走错误路径完成 oneshot。
#[derive(Debug, Clone)]
pub struct FrameError {
    pub code: String,
}

/// `target` 白名单——v1 严格只接 `"webcam"` / `"screen"`，
/// 桌宠端联调误传任何别的字符串都要立即 400 让 caller 知道。
fn validate_target(target: &str) -> Result<()> {
    match target {
        "webcam" | "screen" => Ok(()),
        other => Err(Error::BadRequest(format!(
            "unknown target `{other}`; v1 仅支持 webcam / screen"
        ))),
    }
}

fn clamp_timeout(secs: Option<u64>) -> Duration {
    let raw = secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let bounded = raw.clamp(1, MAX_TIMEOUT_SECS);
    Duration::from_secs(bounded)
}

/// `~/.local/share/fuxi/vision/`——XDG 数据目录下伏羲家产。
/// `dirs::data_local_dir()` 在 macOS 给 `~/Library/Application Support`，
/// Linux 给 `~/.local/share`，都在用户名下不污染系统。
fn vision_dir() -> Result<PathBuf> {
    dirs::data_local_dir()
        .map(|d| d.join("fuxi").join("vision"))
        .ok_or_else(|| Error::Internal("dirs::data_local_dir() 返 None（非常罕见）".into()))
}

/// `image/png` → `png`、`image/jpeg` → `jpg`、其它 → `bin`（兜底，仍能 Read）。
fn ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        _ => "bin",
    }
}

/// `POST /api/xuannv/look` —— 召唤桌宠拍一帧并阻塞等结果。
pub async fn look(
    State(state): State<AppState>,
    Json(req): Json<LookRequest>,
) -> Result<Json<LookResponse>> {
    validate_target(&req.target)?;
    let timeout = clamp_timeout(req.timeout_secs);

    // 玄女未起 → 503（与 conv_ws 同语义：路径正确但暂时不可服务）。
    let xuannv = state
        .fuxi
        .xuannv_id()
        .await
        .ok_or_else(|| Error::Unavailable("玄女尚未注入；请先 set_xuannv".into()))?;

    let bus = state.fuxi.bus();
    if bus.receiver_count() == 0 {
        // 0 个观察者 → 必然无桌宠。错误体里 `error` 字段 = `no_pet_connected`
        // 让 CLI 退出码映射拿到原因。
        return Err(Error::BadRequest("no_pet_connected".into()));
    }

    let request_id = Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel::<std::result::Result<FrameRecord, FrameError>>();
    {
        let mut map = state.vision_pairs.lock().await;
        map.insert(request_id.clone(), tx);
    }

    // publish 事件——meta.agent 必须 set 为 xuannv，桌宠订 /api/conv WS 才收得到。
    let mut meta = EventMeta::now();
    meta.agent = Some(xuannv);
    let ev = Event {
        meta,
        kind: EventKind::VisionRequest {
            request_id: request_id.clone(),
            target: req.target.clone(),
            hint: req.hint.clone(),
        },
    };
    if let Err(e) = bus.publish(ev) {
        // 撤回 oneshot——publish 都失败了 frame 永远不会到。
        state.vision_pairs.lock().await.remove(&request_id);
        return Err(Error::Internal(format!("publish VisionRequest 失败：{e}")));
    }
    info!(
        request_id = %request_id,
        target = %req.target,
        timeout_ms = timeout.as_millis(),
        "vision look 入口 publish + 等 frame"
    );

    // 等 oneshot——超时 / pet 报错 / 通道断都要兜。
    let frame_result = match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(Ok(rec))) => rec,
        Ok(Ok(Err(frame_err))) => {
            warn!(request_id = %request_id, code = %frame_err.code, "桌宠端报错");
            // 用 BadRequest 把 error code 透回客户端——CLI 按 code 映射 stderr 中文。
            return Err(Error::BadRequest(frame_err.code));
        }
        Ok(Err(_recv_err)) => {
            // sender drop 但没发——只可能 frame handler 把 sender 拿走又没 send 就 drop。
            // 当作 upload_failed 兜底。
            return Err(Error::Internal(
                "oneshot 通道断（frame handler bug？）".into(),
            ));
        }
        Err(_elapsed) => {
            // 撤回 oneshot——避免 frame 后到时塞个孤儿 sender 给已退出的 caller。
            state.vision_pairs.lock().await.remove(&request_id);
            return Err(Error::Timeout("timeout".into()));
        }
    };

    Ok(Json(LookResponse {
        ok: true,
        request_id,
        path: frame_result.path,
        mime: frame_result.mime,
        bytes: frame_result.bytes,
    }))
}

/// `POST /api/xuannv/look/frame` —— 桌宠端拍完帧 multipart 回传。
///
/// 字段：
/// - `request_id`：文本，必填，和 `look` 调用配对的 uuid。
/// - `mime`：文本，可选，默认 `image/png`。
/// - `file`：二进制 PNG/JPEG，必填——除非 `error` 字段非空（用户拒授权场景）。
/// - `error`：文本，可选；非空 = 桌宠侧拒上传（如 `user_denied`），handler 把
///   error code 透回 oneshot 让 `look` 返 BadRequest。
pub async fn look_frame(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Json<FrameAck>> {
    let mut request_id: Option<String> = None;
    let mut mime: Option<String> = None;
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut error_code: Option<String> = None;

    loop {
        let next = multipart
            .next_field()
            .await
            .map_err(|e| Error::BadRequest(format!("multipart 读取失败：{e:?}")))?;
        let Some(field) = next else { break };
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "request_id" => {
                request_id = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| Error::BadRequest(format!("读 request_id 失败：{e:?}")))?,
                );
            }
            "mime" => {
                mime = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| Error::BadRequest(format!("读 mime 失败：{e:?}")))?,
                );
            }
            "error" => {
                error_code = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| Error::BadRequest(format!("读 error 失败：{e:?}")))?,
                );
            }
            "file" => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| Error::BadRequest(format!("读 file 字节失败：{e:?}")))?;
                file_bytes = Some(bytes.to_vec());
            }
            _ => {
                // 静默跳——多塞字段无害。
            }
        }
    }

    let request_id =
        request_id.ok_or_else(|| Error::BadRequest("multipart 缺 request_id 字段".into()))?;

    let sender = {
        let mut map = state.vision_pairs.lock().await;
        map.remove(&request_id)
    };
    let Some(sender) = sender else {
        // 没找到匹配的 oneshot——caller 已超时退出 / 重复 frame / id 错误。
        warn!(request_id = %request_id, "frame 上传无匹配 oneshot（caller 已超时？）");
        return Err(Error::NotFound(format!(
            "request_id {request_id} 无匹配等待者（已超时？）"
        )));
    };

    // 错误路径：error 字段非空 → 直接传错给等待方，不写文件。
    if let Some(code) = error_code.as_deref().filter(|s| !s.is_empty()) {
        let _ = sender.send(Err(FrameError {
            code: code.to_string(),
        }));
        return Ok(Json(FrameAck {
            ok: true,
            request_id,
        }));
    }

    let bytes = file_bytes
        .ok_or_else(|| Error::BadRequest("multipart 缺 file 字段（也无 error）".into()))?;
    let mime = mime.unwrap_or_else(|| "image/png".to_string());
    let ext = ext_for_mime(&mime);

    let dir = vision_dir()?;
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| Error::Internal(format!("创建 vision 目录 {} 失败：{e}", dir.display())))?;
    let path = dir.join(format!("{request_id}.{ext}"));
    let len = bytes.len() as u64;
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|e| Error::Internal(format!("写 vision 帧到 {} 失败：{e}", path.display())))?;
    info!(
        request_id = %request_id,
        path = %path.display(),
        bytes = len,
        mime = %mime,
        "vision frame 落档"
    );

    let _ = sender.send(Ok(FrameRecord {
        path,
        mime,
        bytes: len,
    }));
    Ok(Json(FrameAck {
        ok: true,
        request_id,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use axum::routing::post as axum_post;
    use fuxi_core::id::AgentId;
    use fuxi_events::EventBus;
    use fuxi_orchestrator::Fuxi;
    use fuxi_workspace::GitWorktreeWorkspace;
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn make_workspace() -> (tempfile::TempDir, Arc<GitWorktreeWorkspace>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path();
        run_git(p, &["init", "-q", "-b", "main"]).await;
        tokio::fs::write(p.join("README.md"), "seed").await.unwrap();
        run_git(p, &["add", "-A"]).await;
        run_git(
            p,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "init",
            ],
        )
        .await;
        let ws = Arc::new(GitWorktreeWorkspace::with_default_base(p.to_path_buf()));
        (dir, ws)
    }

    async fn run_git(cwd: &std::path::Path, args: &[&str]) {
        let out = tokio::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .output()
            .await
            .expect("spawn git");
        assert!(out.status.success(), "git {args:?} failed");
    }

    async fn make_state() -> (tempfile::TempDir, AppState) {
        let bus = EventBus::with_memory_store().await.unwrap();
        let (ws_dir, ws) = make_workspace().await;
        let fuxi = Arc::new(Fuxi::new(bus, ws));
        let state = AppState::new(fuxi);
        (ws_dir, state)
    }

    fn json_req(uri: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[test]
    fn validate_target_accepts_webcam_and_screen() {
        assert!(validate_target("webcam").is_ok());
        assert!(validate_target("screen").is_ok());
        assert!(validate_target("window").is_err());
        assert!(validate_target("region").is_err());
        assert!(validate_target("").is_err());
    }

    #[test]
    fn clamp_timeout_bounds_to_default_and_max() {
        assert_eq!(
            clamp_timeout(None),
            Duration::from_secs(DEFAULT_TIMEOUT_SECS)
        );
        assert_eq!(clamp_timeout(Some(0)), Duration::from_secs(1));
        assert_eq!(
            clamp_timeout(Some(MAX_TIMEOUT_SECS + 100)),
            Duration::from_secs(MAX_TIMEOUT_SECS)
        );
        assert_eq!(clamp_timeout(Some(5)), Duration::from_secs(5));
    }

    #[test]
    fn ext_for_mime_maps_known_image_types() {
        assert_eq!(ext_for_mime("image/png"), "png");
        assert_eq!(ext_for_mime("image/jpeg"), "jpg");
        assert_eq!(ext_for_mime("image/jpg"), "jpg");
        assert_eq!(ext_for_mime("application/octet-stream"), "bin");
    }

    /// `receiver_count == 0` 闸是防御性的——Fuxi::new 会自启 death_watcher
    /// 一直订阅 bus，所以"真 0"在 production 几乎不发生。本测试通过暴露
    /// receiver_count=1（仅 death_watcher）之外的内部计数无法直接断言；
    /// 我们只断言「has_no_observer + 玄女在 + valid target」时 bus
    /// receiver_count 至少 1（= 防御 0 闸不会误把内部订阅当成「有桌宠」）。
    /// 真"无桌宠"场景靠超时兜底，由 [`look_times_out_when_pet_silent`] 覆盖。
    #[tokio::test]
    async fn fuxi_internal_observers_keep_receiver_count_above_zero() {
        let (_ws_dir, state) = make_state().await;
        let xuannv = AgentId::new();
        state.fuxi.set_xuannv(xuannv).await;
        // death_watcher / extractor / etc. 加起来至少 1
        assert!(
            state.fuxi.bus().receiver_count() >= 1,
            "Fuxi::new 应至少含 death_watcher 一个内部订阅"
        );
    }

    #[tokio::test]
    async fn look_returns_400_for_unknown_target() {
        let (_ws_dir, state) = make_state().await;
        let xuannv = AgentId::new();
        state.fuxi.set_xuannv(xuannv).await;
        let app = Router::new()
            .route("/api/xuannv/look", axum_post(super::look))
            .with_state(state);
        let resp = app
            .oneshot(json_req(
                "/api/xuannv/look",
                serde_json::json!({"target": "foobar"}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn look_returns_503_when_xuannv_missing() {
        let (_ws_dir, state) = make_state().await;
        let app = Router::new()
            .route("/api/xuannv/look", axum_post(super::look))
            .with_state(state);
        let resp = app
            .oneshot(json_req(
                "/api/xuannv/look",
                serde_json::json!({"target": "webcam"}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// 端到端：mock 桌宠订阅 EventBus → look 发起 → 桌宠收到 vision_request → 回 frame
    /// → look 返 200 + 文件存在。
    #[tokio::test]
    async fn end_to_end_look_then_frame_resolves_with_path() {
        let (_ws_dir, state) = make_state().await;
        let xuannv = AgentId::new();
        state.fuxi.set_xuannv(xuannv).await;

        // 模拟桌宠：订 EventBus 等 vision_request
        let bus = state.fuxi.bus().clone();
        let mut sub = bus.subscribe();

        let app = Router::new()
            .route("/api/xuannv/look", axum_post(super::look))
            .route("/api/xuannv/look/frame", axum_post(super::look_frame))
            .with_state(state.clone());

        let app_clone = app.clone();
        let pet = tokio::spawn(async move {
            use futures_util::StreamExt;
            // 等 vision_request 事件
            let req_id;
            loop {
                let ev = sub.next().await.expect("stream").expect("ev");
                if let EventKind::VisionRequest { request_id, .. } = &ev.kind {
                    req_id = request_id.clone();
                    break;
                }
            }
            // 构造 multipart body 模拟拍帧上传
            let boundary = "X-PET-BOUNDARY";
            let body = format!(
                "--{b}\r\n\
                 Content-Disposition: form-data; name=\"request_id\"\r\n\r\n{rid}\r\n\
                 --{b}\r\n\
                 Content-Disposition: form-data; name=\"mime\"\r\n\r\nimage/png\r\n\
                 --{b}\r\n\
                 Content-Disposition: form-data; name=\"file\"; filename=\"frame.png\"\r\n\
                 Content-Type: image/png\r\n\r\nFAKEPNGBYTES\r\n\
                 --{b}--\r\n",
                b = boundary,
                rid = req_id,
            );
            let req = Request::builder()
                .method("POST")
                .uri("/api/xuannv/look/frame")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap();
            let resp = app_clone.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        });

        // 主线发起 look
        let resp = app
            .oneshot(json_req(
                "/api/xuannv/look",
                serde_json::json!({"target": "screen", "timeout_secs": 5}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "look 应返 200");
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["ok"], true);
        let path_str = body["path"].as_str().expect("path");
        let path = std::path::Path::new(path_str);
        assert!(path.exists(), "落档文件应存在：{path_str}");
        assert_eq!(body["mime"], "image/png");
        assert_eq!(
            body["bytes"].as_u64().unwrap(),
            b"FAKEPNGBYTES".len() as u64
        );
        // 清理
        let _ = std::fs::remove_file(path);
        pet.await.unwrap();
    }

    #[tokio::test]
    async fn look_times_out_when_pet_silent() {
        let (_ws_dir, state) = make_state().await;
        let xuannv = AgentId::new();
        state.fuxi.set_xuannv(xuannv).await;

        // 故意挂一个永不消费的订阅者，让 receiver_count > 0 走过初始 400 闸
        let bus = state.fuxi.bus().clone();
        let _sub = bus.subscribe();

        let app = Router::new()
            .route("/api/xuannv/look", axum_post(super::look))
            .with_state(state);
        let resp = app
            .oneshot(json_req(
                "/api/xuannv/look",
                serde_json::json!({"target": "webcam", "timeout_secs": 1}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::REQUEST_TIMEOUT);
        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "timeout");
    }

    #[tokio::test]
    async fn frame_with_unknown_request_id_returns_404() {
        let (_ws_dir, state) = make_state().await;
        let app = Router::new()
            .route("/api/xuannv/look/frame", axum_post(super::look_frame))
            .with_state(state);

        let boundary = "X-BAD-BOUNDARY";
        let body = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"request_id\"\r\n\r\nnonexistent\r\n\
             --{b}--\r\n",
            b = boundary
        );
        let req = Request::builder()
            .method("POST")
            .uri("/api/xuannv/look/frame")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// 桌宠端「禁眼」时上传 error 字段——handler 应把错误透给 look 调用方。
    #[tokio::test]
    async fn frame_with_error_propagates_to_look_caller() {
        let (_ws_dir, state) = make_state().await;
        let xuannv = AgentId::new();
        state.fuxi.set_xuannv(xuannv).await;

        let bus = state.fuxi.bus().clone();
        let mut sub = bus.subscribe();

        let app = Router::new()
            .route("/api/xuannv/look", axum_post(super::look))
            .route("/api/xuannv/look/frame", axum_post(super::look_frame))
            .with_state(state.clone());

        let app_clone = app.clone();
        let pet = tokio::spawn(async move {
            use futures_util::StreamExt;
            let req_id;
            loop {
                let ev = sub.next().await.expect("stream").expect("ev");
                if let EventKind::VisionRequest { request_id, .. } = &ev.kind {
                    req_id = request_id.clone();
                    break;
                }
            }
            let boundary = "X-DENY";
            let body = format!(
                "--{b}\r\nContent-Disposition: form-data; name=\"request_id\"\r\n\r\n{rid}\r\n\
                 --{b}\r\nContent-Disposition: form-data; name=\"error\"\r\n\r\nuser_denied\r\n\
                 --{b}--\r\n",
                b = boundary,
                rid = req_id
            );
            let req = Request::builder()
                .method("POST")
                .uri("/api/xuannv/look/frame")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap();
            let resp = app_clone.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        });

        let resp = app
            .oneshot(json_req(
                "/api/xuannv/look",
                serde_json::json!({"target": "webcam", "timeout_secs": 5}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(resp.into_body(), 4096).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["message"], "bad request: user_denied");
        pet.await.unwrap();
    }
}

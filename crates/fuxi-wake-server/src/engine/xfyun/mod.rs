//! 讯飞 AIKit 唤醒引擎——v0.2 真 FFI（task #5 ship）。
//!
//! ## 编译矩阵
//!
//! - `target_os=linux + target_arch=x86_64 + SDK 目录就位` → `xfyun_ffi` cfg 开，
//!   走 `linux.rs` 真 FFI 实装（bindgen 生成的 binding + 调 `libaikit.so`）。
//! - 其他平台（mac dev / Windows / arm Linux 等）/ ENV `FUXI_XFYUN_SKIP_FFI=1`
//!   → 走 `stub.rs`，所有方法返 `unimplemented!`-like 错——`cargo build` 仍 OK。
//!
//! ## 进程级 vs 连接级
//!
//! `init_process()` —— 在 `main.rs` 启动期调一次：`AIKIT_Init` +
//! `RegisterAbilityCallback` + `AIKIT_EngineInit` + `AIKIT_LoadData`（关键词文件）。
//! 多连接会话共用同一份引擎——讯飞 SDK 的设计是"进程内单 ability 引擎、可多 session"。
//!
//! `XfyunEngine::{init,feed,close}` —— 每个 WS 连接一份：
//! - `init`：`AIKIT_SpecifyDataSet` 选关键词集 + `AIKIT_Start` 开 session
//! - `feed`：PCM 切 320 字节帧 → `AIKIT_Write`；非阻塞 `try_recv` 拿 OnOutput 命中
//! - `close`：`AIKIT_End` 关 session + drop usrContext
//!
//! ## SDK 路径（已 gitignore，不污染仓库）
//!
//! `/Users/e0_7/fuxi/Linux_ivw_e867a88f2_v1.0.11_v2.2.15-rc5/`
//!
//! 关键文件：
//! - `include/aikit_biz_api_c.h` —— **纯 C API**，bindgen 直接吃，**不要走 C++ 头**
//! - `include/aikit_biz_type.h` —— 数据结构（`AIKIT_HANDLE` / `AIKIT_OutputData` 等）
//! - `include/aikit_err.h` —— 错误码常量
//! - `libs/libaikit.so` (3.4 MB) —— 主库
//! - `libs/ef7d69542_v1011_aee.so` —— 引擎插件（对应 ability id `e867a88f2`）
//! - `samples/ivw_sample/ivw_sample.cpp` —— C++ 单文件 demo（参考调用顺序）
//! - `samples/ivw_record_sample/` —— **实时麦克风** demo，最贴近本 server 流式
//!
//! ## ABILITY ID（写死）
//!
//! `"e867a88f2"`——讯飞能力标识（**不是**用户 appID），与 `ef7d69542_v1011_aee.so`
//! 对应。
//!
//! ## ENV 鉴权（用户级，daemon 启动时读）
//!
//! - `FUXI_XFYUN_APPID`
//! - `FUXI_XFYUN_API_KEY`
//! - `FUXI_XFYUN_API_SECRET`
//!
//! 任一缺失 → `init_process` 返 `unauthorized`，让 mac 端 fallback。
//!
//! ## OnOutput callback wire（home 实测的，不是猜的）
//!
//! ```c
//! void OnOutput(AIKIT_HANDLE* handle, const AIKIT_OutputData* output) {
//!     // output->node->key   = "func_wake_up"
//!     // output->node->value = UTF-8 JSON：
//!     // {"rlt":[{"keyword":"...","ncm":<int>,"ncmThresh":<int>,...}]}
//! }
//! ```
//!
//! Rust 端：
//! - 先判 `key=="func_wake_up"`（其他 key 也走 OnOutput）
//! - `serde_json::from_str` 取 `rlt[0].keyword/ncm/ncmThresh`
//! - protocol `score` = `ncm as f32 / ncmThresh as f32`（1.0=阈值正中，>1.0=越界更强）
//! - OnOutput 在讯飞内部线程，桥到 `tokio::sync::mpsc::UnboundedSender<(String,f32)>`
//!
//! ## 关键词文件格式（落到 workDir/keywords.txt）
//!
//! ```text
//! 玄女;nCM:1000;
//! 贾维斯;nCM:1000;
//! ```
//!
//! `nCM`=阈值，越大越严。中文双字词 1000 起步实测 OK；调阈值要实测。
//!
//! ## 错误码 → 协议 error.code
//!
//! - `AIKIT_ERR_AUTH_*` / appid 不对 → `unauthorized`
//! - SDK init 失败 / 装机量耗尽 → `sdk_unavailable`
//! - 帧大小 / 格式错 → `audio_format_invalid`
//! - 限流 → `rate_limited`

use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;

use super::WakeEngine;

/// 讯飞 ability id——与 `libs/ef7d69542_v1011_aee.so` 引擎插件对应。
pub const ABILITY_ID: &str = "e867a88f2";

/// 关键词文件路径推断：`workDir/keywords.txt`。
pub fn default_keywords_path(work_dir: &std::path::Path) -> PathBuf {
    work_dir.join("keywords.txt")
}

/// 把 `&[String]` 关键词列表写成讯飞格式的关键词文件。
/// 每行一个 `词;nCM:<thresh>;`；阈值固定 1000（task #5 暂走默认，调优留 #6 测）。
///
/// 用 `Result` 是因为 IO 可能失败；调用方在 `init_process` 里早返。
pub fn write_keywords_file(path: &std::path::Path, keywords: &[String]) -> Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("keywords 文件路径必须有父目录：{}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| anyhow::anyhow!("创建 keywords 父目录 {} 失败：{}", parent.display(), e))?;
    let mut f = std::fs::File::create(path)
        .map_err(|e| anyhow::anyhow!("创建 keywords 文件 {} 失败：{}", path.display(), e))?;
    for kw in keywords {
        // 关键词不能含 `;`——否则破坏文件分隔；fail loud 不要悄悄改写用户输入。
        if kw.contains(';') {
            anyhow::bail!("关键词不能含分号：{kw}");
        }
        writeln!(f, "{kw};nCM:1000;").map_err(|e| anyhow::anyhow!("写 keywords 文件失败：{e}"))?;
    }
    Ok(())
}

/// 进程级初始化参数——`init_process` 入参。
#[derive(Debug, Clone)]
pub struct ProcessInitParams {
    pub app_id: String,
    pub api_key: String,
    pub api_secret: String,
    /// SDK 工作目录——license 落盘 + keywords 文件存放。需读写权限 + 持久化。
    pub work_dir: PathBuf,
    /// 关键词列表——会被 `write_keywords_file` 落到 `work_dir/keywords.txt`。
    pub keywords: Vec<String>,
}

#[cfg(xfyun_ffi)]
mod ffi;
#[cfg(xfyun_ffi)]
mod linux;

#[cfg(not(xfyun_ffi))]
mod stub;

/// 进程级初始化——daemon 启动期调一次。
///
/// 返回 Ok 表示 `AIKIT_Init/EngineInit/LoadData` 全 OK，可以接连接；Err 表示 SDK 不可用。
pub fn init_process(params: ProcessInitParams) -> Result<()> {
    #[cfg(xfyun_ffi)]
    {
        linux::init_process(params)
    }
    #[cfg(not(xfyun_ffi))]
    {
        stub::init_process(params)
    }
}

/// 进程级释放——daemon 退出时调（best-effort）。
pub fn shutdown_process() {
    #[cfg(xfyun_ffi)]
    {
        linux::shutdown_process();
    }
    #[cfg(not(xfyun_ffi))]
    {
        // stub 模式什么都不做
    }
}

/// 讯飞引擎——每个 WS 连接持一份。
pub struct XfyunEngine {
    #[cfg(xfyun_ffi)]
    inner: linux::Session,
    #[cfg(not(xfyun_ffi))]
    _stub: stub::Stub,
}

impl XfyunEngine {
    /// 新建空 engine——`init` 之前 `feed` 会返错。
    pub fn new() -> Self {
        Self {
            #[cfg(xfyun_ffi)]
            inner: linux::Session::new(),
            #[cfg(not(xfyun_ffi))]
            _stub: stub::Stub,
        }
    }
}

impl Default for XfyunEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl WakeEngine for XfyunEngine {
    async fn init(&self, keywords: &[String]) -> Result<()> {
        #[cfg(xfyun_ffi)]
        {
            self.inner.start_session(keywords)
        }
        #[cfg(not(xfyun_ffi))]
        {
            let _ = keywords;
            stub::session_unimplemented()
        }
    }

    async fn feed(&self, pcm: &[u8]) -> Result<Option<(String, f32)>> {
        #[cfg(xfyun_ffi)]
        {
            self.inner.feed(pcm)
        }
        #[cfg(not(xfyun_ffi))]
        {
            let _ = pcm;
            stub::session_unimplemented()
        }
    }

    async fn close(&self) -> Result<()> {
        #[cfg(xfyun_ffi)]
        {
            self.inner.close()
        }
        #[cfg(not(xfyun_ffi))]
        {
            Ok(())
        }
    }
}

/// OnOutput 内部 JSON 结构——`linux.rs` 与单元测共用 parse。
///
/// `allow(dead_code)`：mac stub 编译路径下 `linux.rs` 不进编译，rustc 看不到这些
/// 类型/函数被调，会误判 dead；测试模块用，但跨 cfg 死码分析不识别。
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
pub(crate) struct WakeRlt {
    pub keyword: String,
    pub ncm: i64,
    #[serde(rename = "ncmThresh")]
    pub ncm_thresh: i64,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
pub(crate) struct WakeOutput {
    pub rlt: Vec<WakeRlt>,
}

/// 解析 OnOutput value JSON → `(keyword, score)`；非命中或解析失败返 None。
///
/// 走纯函数便于无 SDK 也能跑单元测——cheatsheet 的 score 归一化逻辑就在这。
#[allow(dead_code)]
pub(crate) fn parse_wake_output(value: &str) -> Option<(String, f32)> {
    let parsed: WakeOutput = serde_json::from_str(value).ok()?;
    let first = parsed.rlt.into_iter().next()?;
    if first.ncm_thresh <= 0 {
        return None;
    }
    let score = first.ncm as f32 / first.ncm_thresh as f32;
    Some((first.keyword, score))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_keywords_file_writes_xfyun_format() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("kw.txt");
        write_keywords_file(&p, &["玄女".into(), "贾维斯".into()]).unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert_eq!(body, "玄女;nCM:1000;\n贾维斯;nCM:1000;\n");
    }

    #[test]
    fn write_keywords_rejects_semicolon_in_keyword() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("kw.txt");
        let err = write_keywords_file(&p, &["玄女;evil".into()]).unwrap_err();
        assert!(format!("{err}").contains("分号"));
    }

    #[test]
    fn write_keywords_creates_parent_dir() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("nested/sub/kw.txt");
        write_keywords_file(&p, &["玄女".into()]).unwrap();
        assert!(p.exists());
    }

    #[test]
    fn parse_wake_output_real_sdk_shape() {
        // home 实测的 wire 形态——不要破坏。
        let raw = r#"{"rlt":[{"sid":"x","istart":4,"iduration":138,
            "nkeywordscore":205316,"ncm_keyword":1422,"ncm":1422,"ncmThresh":1000,
            "keyword":"小白小白","nDelayFrame":0,"wakeUpType":0}]}"#;
        let (kw, score) = parse_wake_output(raw).expect("命中");
        assert_eq!(kw, "小白小白");
        assert!((score - 1.422).abs() < 1e-3, "score≈1.422，实际 {score}");
    }

    #[test]
    fn parse_wake_output_at_threshold_is_one() {
        let raw = r#"{"rlt":[{"keyword":"玄女","ncm":1000,"ncmThresh":1000}]}"#;
        let (_, score) = parse_wake_output(raw).unwrap();
        assert!((score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn parse_wake_output_empty_rlt_is_none() {
        assert!(parse_wake_output(r#"{"rlt":[]}"#).is_none());
    }

    #[test]
    fn parse_wake_output_zero_thresh_returns_none() {
        // 防 div-by-zero
        let raw = r#"{"rlt":[{"keyword":"x","ncm":1,"ncmThresh":0}]}"#;
        assert!(parse_wake_output(raw).is_none());
    }

    #[test]
    fn parse_wake_output_garbage_is_none() {
        assert!(parse_wake_output("not json").is_none());
        assert!(parse_wake_output(r#"{"foo":42}"#).is_none());
    }

    #[test]
    fn default_keywords_path_under_workdir() {
        let p = default_keywords_path(std::path::Path::new("/var/lib/fuxi-wake"));
        assert_eq!(
            p,
            std::path::PathBuf::from("/var/lib/fuxi-wake/keywords.txt")
        );
    }
}

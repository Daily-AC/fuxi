//! 讯飞 AIKit 真 FFI——只在 `cfg(xfyun_ffi)` 下编译。
//!
//! ## 进程级状态
//!
//! `init_process` 调用一次后进入 ready；`shutdown_process` 走收尾。`OnceLock` 保证
//! 多次调用幂等（SDK 内部不允许重复 Init/EngineInit）。
//!
//! ## 会话级状态
//!
//! `Session` 持 `*mut AIKIT_HANDLE` + 一个 `UnboundedReceiver<(String, f32)>`，
//! 配套的 `Sender` 通过 `Box::into_raw` 装在 `handle->usrContext`，OnOutput 回调
//! 通过 handle 反查 sender 把命中事件推回 `feed`。
//!
//! ## 线程模型
//!
//! `OnOutput` 在讯飞 SDK 内部线程上跑（**不在 tokio runtime**）——只能用同步 channel
//! API（`UnboundedSender::send`）；不能在回调里 `block_on`。

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, anyhow};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, info, warn};

use super::ffi as raw;
use super::{
    ABILITY_ID, ProcessInitParams, default_keywords_path, parse_wake_output, write_keywords_file,
};

/// 进程级一次性 init 守门——重入直接返 Ok。
/// 用 `OnceLock<Result<()>>` 把 SDK init 结果缓存住——失败也不重试（讯飞 SDK 重复
/// init 行为不一致，宁可让 daemon 退出重启）。
static PROCESS_INIT: OnceLock<Result<(), String>> = OnceLock::new();

/// 进程级 workDir——OnOutput 错误日志要用，shutdown 时也可能要 cleanup。
static WORK_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn init_process(params: ProcessInitParams) -> Result<()> {
    let res = PROCESS_INIT.get_or_init(|| init_process_inner(&params).map_err(|e| e.to_string()));
    res.clone().map_err(|s| anyhow!(s))
}

fn init_process_inner(params: &ProcessInitParams) -> Result<()> {
    if params.app_id.is_empty() || params.api_key.is_empty() || params.api_secret.is_empty() {
        anyhow::bail!("xfyun ENV 鉴权缺：FUXI_XFYUN_APPID/API_KEY/API_SECRET 必须全设");
    }
    if params.keywords.is_empty() {
        anyhow::bail!("xfyun init: keywords 不能为空");
    }

    std::fs::create_dir_all(&params.work_dir)
        .with_context(|| format!("创建 workDir {} 失败", params.work_dir.display()))?;

    let kw_path = default_keywords_path(&params.work_dir);
    write_keywords_file(&kw_path, &params.keywords)?;
    info!(
        keywords = ?params.keywords,
        path = %kw_path.display(),
        "xfyun: keywords 文件已写入"
    );

    let app_id = CString::new(params.app_id.clone())?;
    let api_key = CString::new(params.api_key.clone())?;
    let api_secret = CString::new(params.api_secret.clone())?;
    let work_dir = CString::new(
        params
            .work_dir
            .to_str()
            .ok_or_else(|| anyhow!("workDir 含非 UTF-8 字符：{}", params.work_dir.display()))?,
    )?;
    let ability = CString::new(ABILITY_ID)?;
    let kw_key = CString::new("key_word")?;
    let kw_value = CString::new(
        kw_path
            .to_str()
            .ok_or_else(|| anyhow!("keywords path 非 UTF-8"))?,
    )?;

    // 注意：bindgen 里 InitParam 有的字段是 `*const c_char`，未用字段填 NULL。
    let mut init_param: raw::AIKIT_InitParam = unsafe { std::mem::zeroed() };
    init_param.authType = 0;
    init_param.appID = app_id.as_ptr();
    init_param.apiKey = api_key.as_ptr();
    init_param.apiSecret = api_secret.as_ptr();
    init_param.workDir = work_dir.as_ptr();
    // resDir / licenseFile / batchID / UDID / cfgFile 全 NULL（讯飞会用默认值）

    let ret = unsafe { raw::AIKIT_Init(&mut init_param) };
    if ret != 0 {
        anyhow::bail!("AIKIT_Init 失败：err={ret}（appID/apiKey/apiSecret 是否对、能否联网激活）");
    }
    info!("xfyun: AIKIT_Init ok");

    let cbs = raw::AIKIT_Callbacks {
        outputCB: Some(on_output),
        eventCB: Some(on_event),
        errorCB: Some(on_error),
    };
    let ret = unsafe { raw::AIKIT_RegisterAbilityCallback(ability.as_ptr(), cbs) };
    if ret != 0 {
        anyhow::bail!("AIKIT_RegisterAbilityCallback 失败：err={ret}");
    }

    let ret = unsafe { raw::AIKIT_EngineInit(ability.as_ptr(), std::ptr::null_mut()) };
    if ret != 0 {
        anyhow::bail!("AIKIT_EngineInit 失败：err={ret}");
    }
    info!("xfyun: AIKIT_EngineInit ok");

    // 关键词文件加载——index=0；多关键词集要叠 index 跑多次 LoadData。
    let mut custom = raw::_AIKIT_CustomData {
        next: std::ptr::null_mut(),
        key: kw_key.as_ptr(),
        value: kw_value.as_ptr() as *mut c_void,
        reserved: std::ptr::null_mut(),
        index: 0,
        len: kw_value.as_bytes().len() as i32,
        from: raw::AIKIT_DATA_PTR_TYPE_E_AIKIT_DATA_PTR_PATH as i32,
    };
    let ret = unsafe { raw::AIKIT_LoadData(ability.as_ptr(), &mut custom) };
    if ret != 0 {
        anyhow::bail!(
            "AIKIT_LoadData 失败：err={ret}（关键词文件 {} 是否可读）",
            kw_path.display()
        );
    }
    info!(path = %kw_path.display(), "xfyun: AIKIT_LoadData ok");

    // workDir 保存——shutdown 用得上。
    let _ = WORK_DIR.set(params.work_dir.clone());

    Ok(())
}

pub fn shutdown_process() {
    if PROCESS_INIT.get().is_none() {
        return;
    }
    let ability = match CString::new(ABILITY_ID) {
        Ok(c) => c,
        Err(_) => return,
    };
    let key = match CString::new("key_word") {
        Ok(c) => c,
        Err(_) => return,
    };
    unsafe {
        raw::AIKIT_UnLoadData(ability.as_ptr(), key.as_ptr(), 0);
        raw::AIKIT_EngineUnInit(ability.as_ptr());
        raw::AIKIT_UnInit();
    }
    info!("xfyun: shutdown_process done");
}

/// AIKIT_End 失败时的进程级遗留 session 槽——下一个 start_session 先回收它。
///
/// WHY 进程级 static：Session 对象一连接一个，泄漏者连接已断、对象已 drop，
/// 回收责任只能落在后来者头上。SDK 进程级单 session，槽位至多一个。
/// 没有这层，End 一次失败 = handle 永久丢失 = 后续所有 AIKIT_Start 18310，
/// 唤醒全聋直到人工 systemctl restart（2026-06-10 用户实测命中）。
struct LeakedSession {
    handle: *mut raw::AIKIT_HANDLE,
    sender_ptr: *mut UnboundedSender<(String, f32)>,
}

// 安全：槽位只在 Mutex 下存取；指针仅作回收凭据，不在此解引用。
unsafe impl Send for LeakedSession {}

static LEAKED_SESSION: Mutex<Option<LeakedSession>> = Mutex::new(None);

/// 单连接会话——内部裹 `Mutex` 让 `&self` 接口下做可变操作（讯飞 handle 单线程使用）。
pub struct Session {
    state: Mutex<SessionState>,
}

struct SessionState {
    handle: *mut raw::AIKIT_HANDLE,
    /// usrContext 里装的 sender 原始指针——`close` 时 `Box::from_raw` 释放。
    sender_ptr: *mut UnboundedSender<(String, f32)>,
    rx: Option<UnboundedReceiver<(String, f32)>>,
}

// 安全：handle 仅在 `Mutex` 守护下被一个线程访问；OnOutput 回调通过 sender 跨线程
// 通信，这部分的同步走 `tokio::sync::mpsc` 自己保证。讯飞 SDK 把同一 handle 给多个
// 线程同时 `AIKIT_Write` 行为未定义——`Mutex<SessionState>` 串行化即可。
unsafe impl Send for SessionState {}
unsafe impl Sync for SessionState {}

impl Session {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(SessionState {
                handle: std::ptr::null_mut(),
                sender_ptr: std::ptr::null_mut(),
                rx: None,
            }),
        }
    }

    pub fn start_session(&self, keywords: &[String]) -> Result<()> {
        if PROCESS_INIT.get().and_then(|r| r.as_ref().ok()).is_none() {
            anyhow::bail!("xfyun: init_process 未成功——daemon 启动期失败，本次会话拒绝");
        }
        let mut s = self.state.lock().expect("session state lock");
        if !s.handle.is_null() {
            anyhow::bail!("xfyun session 已 start，重复 init");
        }

        // 先回收遗留 session——上一个连接 AIKIT_End 失败留下的。不回收的话
        // 下面 AIKIT_Start 必撞 18310（SDK 认为 session 还开着）。
        let leaked = LEAKED_SESSION.lock().expect("leaked slot lock").take();
        if let Some(l) = leaked {
            let ret = unsafe { raw::AIKIT_End(l.handle) };
            if ret == 0 {
                // End 成功后 SDK 不再回调 usrContext，sender box 可安全释放
                unsafe { drop(Box::from_raw(l.sender_ptr)) };
                info!("xfyun: 遗留 session 回收成功");
            } else {
                *LEAKED_SESSION.lock().expect("leaked slot lock") = Some(l);
                anyhow::bail!("遗留 xfyun session 回收失败 err={ret}——唤醒暂不可用，等下次连接重试");
            }
        }

        let ability = CString::new(ABILITY_ID)?;
        let kw_key = CString::new("key_word")?;

        // 关键词集——index 数组按当前 keywords 数量来；进程级只 LoadData 了 index=0
        // 一份（task #5 暂走单关键词集），所以这里 count=1，index=[0]。
        // 未来多关键词集（玄女 / 贾维斯切换）要扩。
        if keywords.is_empty() {
            anyhow::bail!("xfyun start_session: keywords 不能为空");
        }
        let mut indices: Vec<c_int> = vec![0];
        let ret = unsafe {
            raw::AIKIT_SpecifyDataSet(
                ability.as_ptr(),
                kw_key.as_ptr(),
                indices.as_mut_ptr(),
                indices.len() as c_int,
            )
        };
        if ret != 0 {
            anyhow::bail!("AIKIT_SpecifyDataSet 失败：err={ret}");
        }

        // 构 BizParam：阈值参数 + gramLoad=true（参考 ivw_sample.cpp:59-60）。
        let pb = unsafe { raw::AIKITBuilder_Create(raw::BuilderType__BUILDER_TYPE_PARAM) };
        if pb.is_null() {
            anyhow::bail!("AIKITBuilder_Create(PARAM) 返 null");
        }
        let thresh_key = CString::new("wdec_param_nCmThreshold")?;
        let thresh_val = CString::new("0 0:1000")?;
        let gram_key = CString::new("gramLoad")?;
        unsafe {
            raw::AIKITBuilder_AddString(
                pb,
                thresh_key.as_ptr(),
                thresh_val.as_ptr(),
                thresh_val.as_bytes().len() as c_int,
            );
            raw::AIKITBuilder_AddBool(pb, gram_key.as_ptr(), true);
        }
        let param = unsafe { raw::AIKITBuilder_BuildParam(pb) };
        if param.is_null() {
            unsafe { raw::AIKITBuilder_Destroy(pb) };
            anyhow::bail!("AIKITBuilder_BuildParam 返 null");
        }

        // sender 装进 usrContext——OnOutput 回调里反 cast 拿到。
        let (tx, rx) = mpsc::unbounded_channel::<(String, f32)>();
        let sender_ptr = Box::into_raw(Box::new(tx));

        let mut handle: *mut raw::AIKIT_HANDLE = std::ptr::null_mut();
        let ret = unsafe {
            raw::AIKIT_Start(
                ability.as_ptr(),
                param,
                sender_ptr as *mut c_void,
                &mut handle,
            )
        };
        unsafe { raw::AIKITBuilder_Destroy(pb) };

        if ret != 0 || handle.is_null() {
            // 失败要 drop sender 回收 box
            unsafe { drop(Box::from_raw(sender_ptr)) };
            anyhow::bail!("AIKIT_Start 失败：err={ret}");
        }

        s.handle = handle;
        s.sender_ptr = sender_ptr;
        s.rx = Some(rx);
        debug!(handle = ?handle, "xfyun: session started");
        Ok(())
    }

    pub fn feed(&self, pcm: &[u8]) -> Result<Option<(String, f32)>> {
        let mut s = self.state.lock().expect("session state lock");
        if s.handle.is_null() {
            anyhow::bail!("xfyun feed 在 init 之前");
        }

        // 切 320 字节帧（10ms @ 16kHz s16le）；尾部不足 320 的丢弃——讯飞 SDK 内部
        // 自己累计；半帧喂进去会触发 AIKIT_Write 报参数错。
        let chunk_size = 320;
        let key = CString::new("wav")?;

        let mut i = 0;
        while i + chunk_size <= pcm.len() {
            let frame = &pcm[i..i + chunk_size];
            // BuilderData 用 stack alloc——AIKIT_Write 内部会拷数据，调用返回后 frame
            // 的生命周期结束也安全。
            let mut bd = raw::BuilderData_ {
                type_: raw::BuilderDataType__DATA_TYPE_AUDIO as c_int,
                name: key.as_ptr(),
                data: frame.as_ptr() as *mut c_void,
                len: chunk_size as c_int,
                // bindgen 用 enum typedef 名拼前缀；SDK header 写 `typedef enum
                // _AIKIT_DataStatus_ {..} _AIKIT_DataStatus;` → 常量名带 `_` 前缀。
                status: raw::_AIKIT_DataStatus_AIKIT_DataContinue as c_int,
            };
            let db = unsafe { raw::AIKITBuilder_Create(raw::BuilderType__BUILDER_TYPE_DATA) };
            if db.is_null() {
                anyhow::bail!("AIKITBuilder_Create(DATA) 返 null");
            }
            let add_ret = unsafe { raw::AIKITBuilder_AddBuf(db, &mut bd) };
            if add_ret != 0 {
                unsafe { raw::AIKITBuilder_Destroy(db) };
                anyhow::bail!("AIKITBuilder_AddBuf 失败：err={add_ret}");
            }
            let input = unsafe { raw::AIKITBuilder_BuildData(db) };
            if input.is_null() {
                unsafe { raw::AIKITBuilder_Destroy(db) };
                anyhow::bail!("AIKITBuilder_BuildData 返 null");
            }
            let ret = unsafe { raw::AIKIT_Write(s.handle, input) };
            unsafe { raw::AIKITBuilder_Destroy(db) };
            if ret != 0 {
                anyhow::bail!("AIKIT_Write 失败：err={ret}");
            }
            i += chunk_size;
        }

        // try_recv 拿命中——OnOutput 是异步的，可能本次 feed 还没出结果，下次再 try。
        if let Some(rx) = s.rx.as_mut() {
            match rx.try_recv() {
                Ok(hit) => Ok(Some(hit)),
                Err(mpsc::error::TryRecvError::Empty) => Ok(None),
                Err(mpsc::error::TryRecvError::Disconnected) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    pub fn close(&self) -> Result<()> {
        let mut s = self.state.lock().expect("session state lock");
        if s.handle.is_null() {
            return Ok(());
        }
        let ret = unsafe { raw::AIKIT_End(s.handle) };
        if ret != 0 {
            // End 失败时 handle + sender 移入遗留槽，下次 start_session 前回收。
            // 老行为是直接丢弃：handle 永久丢失 → SDK 永久 18310；sender box 提前
            // free 还可能被 OnOutput 回调写成 UAF。宁可暂泄，不可丢凭据。
            warn!(err = ret, "xfyun: AIKIT_End 失败——session 移入遗留槽待回收");
            *LEAKED_SESSION.lock().expect("leaked slot lock") = Some(LeakedSession {
                handle: s.handle,
                sender_ptr: s.sender_ptr,
            });
        } else if !s.sender_ptr.is_null() {
            unsafe { drop(Box::from_raw(s.sender_ptr)) };
        }
        s.handle = std::ptr::null_mut();
        s.sender_ptr = std::ptr::null_mut();
        s.rx = None;
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // best-effort——`close` 已被调过的话是 noop。
        let _ = self.close();
    }
}

// ---- 异步回调 ----
//
// 三个 callback 都是 `extern "C"`，会在讯飞内部线程上被调。**禁忌**：
// - 不要 panic（panic 跨 FFI = UB）—— 用 std::panic::catch_unwind 兜
// - 不要持有 tokio runtime handle（不在 runtime 上）—— 只用 `mpsc::UnboundedSender::send`

unsafe extern "C" fn on_output(
    handle: *mut raw::AIKIT_HANDLE,
    output: *const raw::AIKIT_OutputData,
) {
    let _ = std::panic::catch_unwind(|| {
        if handle.is_null() || output.is_null() {
            return;
        }
        let usr_ctx = unsafe { (*handle).usrContext };
        if usr_ctx.is_null() {
            return;
        }

        // node 是链表头——讯飞 wake 事件单 node，取头读 key/value 即可。
        let node = unsafe { (*output).node };
        if node.is_null() {
            return;
        }
        let key_ptr = unsafe { (*node).key };
        let value_ptr = unsafe { (*node).value as *const c_char };
        if key_ptr.is_null() || value_ptr.is_null() {
            return;
        }

        let key = match unsafe { CStr::from_ptr(key_ptr) }.to_str() {
            Ok(s) => s,
            Err(_) => return,
        };
        if key != "func_wake_up" {
            // 其他 event 走 OnEvent；OnOutput 里的非 wake key 一律忽略。
            return;
        }

        let value = match unsafe { CStr::from_ptr(value_ptr) }.to_str() {
            Ok(s) => s,
            Err(_) => return,
        };
        let Some(hit) = parse_wake_output(value) else {
            return;
        };

        let sender_ptr = usr_ctx as *const UnboundedSender<(String, f32)>;
        let sender = unsafe { &*sender_ptr };
        // 失败说明 receiver 已 drop（连接关了）—— 静默丢弃。
        let _ = sender.send(hit);
    });
}

unsafe extern "C" fn on_event(
    _handle: *mut raw::AIKIT_HANDLE,
    event_type: raw::AIKIT_EVENT,
    _value: *const raw::AIKIT_OutputEvent,
) {
    let _ = std::panic::catch_unwind(|| {
        debug!(event = event_type, "xfyun OnEvent");
    });
}

unsafe extern "C" fn on_error(_handle: *mut raw::AIKIT_HANDLE, err: i32, desc: *const c_char) {
    let _ = std::panic::catch_unwind(|| {
        let desc_s = if desc.is_null() {
            "(null)".to_string()
        } else {
            unsafe { CStr::from_ptr(desc) }
                .to_string_lossy()
                .into_owned()
        };
        warn!(err, desc = %desc_s, "xfyun OnError");
    });
}

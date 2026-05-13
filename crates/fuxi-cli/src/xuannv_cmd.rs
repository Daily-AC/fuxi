//! `fuxi xuannv refresh` —— 让玄女下次 fresh session 加载最新 dispatch-routing 教学。
//!
//! ## 为什么需要
//!
//! 玄女是长跑 cc 进程：`ensure_xuannv` 启动时若 oracle 里有 xuannv/session_id
//! record → 走 `cc --resume <session>` 续写。cc 自身行为：resume 时用**老 session
//! 的** system prompt，**忽略** `--append-system-prompt`。
//!
//! 后果：我们更新 `roles/xuannv/instructions/dispatch-routing.md` 教新东西
//! （比如 `fuxi spawn --project / --ephemeral` 用法），玄女永远学不到——除非
//! fresh session。
//!
//! ## 用法
//!
//! ```bash
//! fuxi xuannv refresh         # 清 oracle 里 session_id record
//! systemctl --user restart fuxi-im   # 触发 ensure_xuannv 走 fresh session 路径
//! ```
//!
//! 重启后 oracle 里没 session record → resolve_xuannv_session 走 fresh path →
//! cc 启动加 `--append-system-prompt`（含 dispatch-routing.md 最新版）→ 玄女
//! 学到新教学。
//!
//! ## 代价
//!
//! 玄女失忆——cc session 历史断档。下次启动是全新对话，不知道之前用户说了啥。
//! 这是单次成本：教学更新频率应远低于对话连续性需求。

use anyhow::{Context, Result};
use clap::Args;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_events::EventStore;
use fuxi_memory::OracleStore;
use std::io::Read;
use std::path::PathBuf;

use crate::session;

/// 玄女上下文交接落档绝对路径。**与后端 fuxi-im::xuannv_handoff 看的同一份**。
/// 路径走 `~/.fuxi/xuannv-handoff.md`——跟其他 fuxi 家产同目录，systemd 服务
/// 进程也读得到。
fn handoff_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".fuxi").join("xuannv-handoff.md"))
        .unwrap_or_else(|| PathBuf::from(".fuxi/xuannv-handoff.md"))
}

/// 上限：500 字 = 大约 1500-2000 chars（中文每字 1 char）。500 字数中文还能塞下
/// 「当前活跃 task + 待用户拍板事项 + 用户近期偏好」三段，超出说明玄女写跑题
/// 了——拒并提示她精简。
const HANDOFF_MAX_CHARS: usize = 2000;

/// daemon / IM 默认 events.db 路径——跟 `fuxi im start` 用同一份
/// (`im.rs::default_events_db_path` SoT：`$HOME/.fuxi/events.db`)。
fn default_events_db_path() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".fuxi").join("events.db"))
        .unwrap_or_else(|| std::path::PathBuf::from(".fuxi/events.db"))
}

#[derive(Debug, Args)]
pub struct HandoffWriteArgs {
    /// handoff 内容（≤500 字 markdown）；传 `-` 从 stdin 读。
    pub body: String,
}

pub async fn run_handoff_write(args: HandoffWriteArgs) -> Result<()> {
    let body = if args.body == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .context("读 stdin 失败")?;
        s
    } else {
        args.body
    };
    let trimmed = body.trim();
    if trimmed.is_empty() {
        anyhow::bail!("handoff 内容为空——必须 ≤500 字 markdown 实际内容");
    }
    let len = trimmed.chars().count();
    if len > HANDOFF_MAX_CHARS {
        anyhow::bail!(
            "handoff 超长：{len} chars > {HANDOFF_MAX_CHARS} chars。500 字应足够 \
             摘要「活跃 task + 待用户拍板事项 + 用户偏好」——精简后再写"
        );
    }

    let path = handoff_path();
    if let Some(parent) = path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建父目录 {} 失败", parent.display()))?;
    }
    std::fs::write(&path, trimmed.as_bytes())
        .with_context(|| format!("写 handoff 文件 {} 失败", path.display()))?;
    println!("✓ handoff 已落档：{} ({} chars)", path.display(), len);

    // emit 事件——后端 fuxi-im 长跑进程订阅 EventBus 看 XuannvHandoffWritten
    // 即触发 idle kill + spawn 新副本。CLI 用 EventStore 直写而非走 daemon
    // socket：CLI 短命，连 socket 拉 IPC 重；EventStore append-only 安全。
    let events_db = std::env::var("FUXI_EVENTS_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| default_events_db_path());
    if events_db.exists() {
        let store = EventStore::connect_file(&events_db)
            .await
            .context("打开 events.db 失败")?;
        let mut meta = EventMeta::now();
        meta.session = None;
        let ev = Event {
            meta,
            kind: EventKind::XuannvHandoffWritten {
                path: path.clone(),
                length_chars: len.min(u32::MAX as usize) as u32,
            },
        };
        store
            .append(&ev)
            .await
            .context("XuannvHandoffWritten 事件入库失败")?;
        println!("✓ XuannvHandoffWritten 事件已 publish");
    } else {
        println!(
            "⚠ events.db 不存在（{}）——跳过事件发布",
            events_db.display()
        );
        println!("  fuxi-im 启动时会自动检测落档，不影响交接流程。");
    }
    Ok(())
}

pub async fn run_handoff_read() -> Result<()> {
    let path = handoff_path();
    if !path.exists() {
        println!("尚无 handoff 文件：{}", path.display());
        return Ok(());
    }
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("读 handoff 文件 {} 失败", path.display()))?;
    println!("=== handoff @ {} ===", path.display());
    println!("{}", body);
    Ok(())
}

/// Jarvis · 语音模式：上限只是"防呆"，玄女自己应该写一两句。超过 500 字
/// 大概率是她把整段 IM 回复也塞过来了——拒并提示她精简。
const SAY_MAX_CHARS: usize = 500;

/// Phase 3 情绪映射：合法 emotion 标签——TTS 端 / 桌宠端按这套切 ref / sprite。
/// 留 `normal` 是为了让 cc 显式传也合法；不传 = `None` daemon publish 兜底，
/// 跟 `--emotion normal` 行为等价。
const ALLOWED_EMOTIONS: &[&str] = &["normal", "happy", "surprise", "worry", "serious", "sad"];

#[derive(Debug, Args)]
pub struct SayArgs {
    /// 要念出口的话；传 `-` 从 stdin 读（兼容长字符串里有引号、换行等场景）。
    pub text: String,
    /// Phase 3 情绪：happy / surprise / worry / serious / sad / normal。
    /// 不传 = normal（TTS 走默认派蒙 ref + 桌宠 idle 走 Nomal）。
    #[arg(long)]
    pub emotion: Option<String>,
}

pub async fn run_say(args: SayArgs) -> Result<()> {
    let text = if args.text == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .context("读 stdin 失败")?;
        s
    } else {
        args.text
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        anyhow::bail!("say 内容为空——至少给一两个字让用户听到");
    }
    let len = trimmed.chars().count();
    if len > SAY_MAX_CHARS {
        anyhow::bail!(
            "say 超长：{len} chars > {SAY_MAX_CHARS}。语音模式下只念一两句， \
             长内容（代码 / 列表 / 解释）写 IM 即可，不必念出来"
        );
    }

    // emotion 校验：传了就必须是已知值（白名单 fail-fast，避免 cc 编错 typo 静默走 default）
    let emotion = match args.emotion.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(e) if ALLOWED_EMOTIONS.contains(&e) => Some(e.to_string()),
        Some(e) => anyhow::bail!(
            "未知 emotion `{e}`，合法值：{}",
            ALLOWED_EMOTIONS.join(" / ")
        ),
    };

    // 走 daemon —— daemon 端 EmitEvent handler 会注入 meta.agent=xuannv_id，
    // CLI 进程拿不到运行时 xuannv_id，必须 daemon 兜底。
    let resp = crate::client::send(crate::ipc::Command::EmitEvent {
        kind: crate::ipc::EventKindPayload::XuannvVoiceLine {
            text: trimmed.to_string(),
            emotion: emotion.clone(),
        },
    })
    .await
    .context("daemon 通讯失败——是否在 `fuxi up` 或 `fuxi im start` 进程下")?;
    match resp {
        crate::ipc::Response::Ok { .. } => {
            let tag = emotion.as_deref().unwrap_or("normal");
            println!("✓ 已上发 ({} chars, emotion={})", len, tag);
            Ok(())
        }
        crate::ipc::Response::Pong => anyhow::bail!("daemon 返回 Pong（异常）"),
        crate::ipc::Response::Err { error } => anyhow::bail!("daemon 拒绝：{error}"),
    }
}

// ── 玄女眼睛 v1（spec 2026-05-14-xuannv-vision-design.md）──────────────
//
// `fuxi xuannv look --target webcam|screen [--hint <str>] [--timeout-secs N]`
//
// 阻塞 CLI：HTTP POST `/api/xuannv/look`，stdout = 一行绝对 path，玄女随后
// `Read(path)` 把图带进上下文。错误 → 非零退出 + stderr 中文提示，对应 spec
// §错误兜底矩阵的退出码 / 文案表。
//
// fuxi-im 默认 loopback `127.0.0.1:9100`，CLI 在 home 上跟 systemd 同机，
// `FUXI_IM_BASE_URL` env 可覆盖（远程调试 / mac 端手测）。

const VISION_DEFAULT_BASE_URL: &str = "http://127.0.0.1:9100";

/// spec 错误兜底矩阵 + β 桌宠端 frame error 扩展：服务端 ErrorBody.error → CLI 退出码。
/// CLI 由 `process::exit(...)` 直退，绕开 anyhow 的 1（默认 panic 码同 1
/// 跟 `no_pet_connected` 重叠就读不准）。
///
/// `no_device` / `capture_failed` 是 β 上报的桌宠侧 frame 失败 code（spec
/// §错误兜底矩阵 v1.1 扩展），`upload_failed` 留给 PWA / 网络层后续报。
fn exit_code_for_vision_error(code: &str) -> i32 {
    match code {
        "no_pet_connected" => 2,
        "user_denied" => 3,
        "permission_denied" => 4,
        "timeout" => 5,
        "upload_failed" => 6,
        "no_device" => 7,
        "capture_failed" => 8,
        _ => 1,
    }
}

/// 把 spec 错误 code 翻成中文 stderr 提示——与 `roles/xuannv` prelude 教学
/// 同口径，方便玄女拿到原文直接转给用户而不必再二次润色。
fn stderr_for_vision_error(code: &str) -> &'static str {
    match code {
        "no_pet_connected" => "我现在看不见你（桌宠没连）",
        "user_denied" => "你把我眼睛蒙了，先去右键菜单解锁",
        "permission_denied" => "需要你在系统设置→隐私→屏幕录制里给我权限",
        "timeout" => "拍帧太慢，重新让我看一次？",
        "upload_failed" => "图传不上去，可能网断了",
        "no_device" => "桌宠那边找不到摄像头/屏幕设备",
        "capture_failed" => "拍帧失败，可能桌宠崩了或者权限刚被撤",
        _ => "看不见——服务端拒了请求",
    }
}

#[derive(Debug, Args)]
pub struct LookArgs {
    /// 看哪只眼睛：`webcam` 或 `screen`。v1 仅这俩；window/region 留 v1.1。
    #[arg(long)]
    pub target: String,
    /// 给桌宠端 toast 的备忘文本（"看看用户的报错"），可省。
    #[arg(long)]
    pub hint: Option<String>,
    /// 等帧上传超时（秒）。默认走服务端 10s；上限服务端 clamp 到 30s。
    #[arg(long = "timeout-secs")]
    pub timeout_secs: Option<u64>,
    /// 覆盖 fuxi-im base url——默认 `http://127.0.0.1:9100`（home loopback）。
    /// 没显式传时回 `FUXI_IM_BASE_URL` 环境变量，mac 端手测时可指
    /// `https://im.qmledmq.cn:8443` 之类的远端地址。
    #[arg(long = "base-url")]
    pub base_url: Option<String>,
}

#[derive(serde::Deserialize)]
struct LookOk {
    #[allow(dead_code)]
    ok: bool,
    #[allow(dead_code)]
    request_id: String,
    path: String,
    #[allow(dead_code)]
    mime: String,
    #[allow(dead_code)]
    bytes: u64,
}

#[derive(serde::Deserialize)]
struct LookErr {
    /// 服务端 ErrorBody.error 字段。
    /// - typed Vision 错误（user_denied / permission_denied / no_device /
    ///   capture_failed / no_pet_connected）已经是 spec code 字面量
    /// - 其它泛型错误（如 Timeout）走旧 `bad_request` / `timeout` 字面量，
    ///   需要从 message 兜底切 code（见 [`extract_code`]）
    error: String,
    /// `Error::Display` 形如 `"timeout: timeout"`——typed Vision 路径下不重要
    /// （error 已是 code），泛型错误走 fallback 用。
    #[serde(default)]
    message: String,
}

/// 已知 typed Vision code 白名单——`error` 字段直接命中其一就用那个；否则
/// fallback 到从 message 切 spec code。这套兜底覆盖：
/// (1) 后端老版本（α v1）把所有错误都塞进 BadRequest message 的旧响应；
/// (2) Timeout 这类后端通用错误 ErrorBody.error="timeout"（恰好命中白名单）。
const TYPED_VISION_CODES: &[&str] = &[
    "no_pet_connected",
    "user_denied",
    "permission_denied",
    "no_device",
    "capture_failed",
    "timeout",
    "upload_failed",
];

/// 从 ErrorBody 提取 spec code：先看 `error` 字段是否已是已知 code；否则
/// 兜底 parse `message`（"bad request: user_denied" 形态切尾段）。
fn extract_code<'a>(error: &'a str, message: &'a str) -> &'a str {
    if TYPED_VISION_CODES.contains(&error) {
        return error;
    }
    match message.split_once(": ") {
        Some((_, tail)) if TYPED_VISION_CODES.contains(&tail) => tail,
        _ => error,
    }
}

pub async fn run_look(args: LookArgs) -> Result<()> {
    if args.target != "webcam" && args.target != "screen" {
        anyhow::bail!(
            "未知 target `{}`；v1 仅支持 webcam / screen（v1.1 加 window/region）",
            args.target
        );
    }
    let env_base = std::env::var("FUXI_IM_BASE_URL").ok();
    let base = args
        .base_url
        .as_deref()
        .or(env_base.as_deref())
        .unwrap_or(VISION_DEFAULT_BASE_URL)
        .trim_end_matches('/');
    let url = format!("{base}/api/xuannv/look");

    let mut body = serde_json::json!({ "target": args.target });
    if let Some(h) = args.hint.as_deref().filter(|s| !s.is_empty()) {
        body["hint"] = serde_json::Value::String(h.to_string());
    }
    if let Some(t) = args.timeout_secs {
        body["timeout_secs"] = serde_json::Value::Number(serde_json::Number::from(t));
    }

    // reqwest 自身超时给 server timeout + 5s buffer——server 已 clamp 30s，
    // client 留余量等 server 走完返 408 而不是 client 自己 timeout 提前断。
    let client_timeout = args.timeout_secs.unwrap_or(10) + 5;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(client_timeout))
        .build()
        .context("构造 reqwest client 失败")?;

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {url} 失败——fuxi-im 是否在运行（127.0.0.1:9100）？"))?;
    let status = resp.status();
    if status.is_success() {
        let parsed: LookOk = resp
            .json()
            .await
            .context("解析 /look 200 响应 JSON 失败——服务端返回不符约定格式")?;
        // 关键约定：stdout 一行 = 绝对 path，给玄女 cc 自动 `Read`。
        println!("{}", parsed.path);
        Ok(())
    } else {
        // 服务端 ErrorBody.error 已经是 spec code（typed Vision 路径）；
        // 老版本 / 泛型错误走 message 兜底。
        let err_body: LookErr = resp.json().await.unwrap_or(LookErr {
            error: "unknown".into(),
            message: format!("服务端返回 {status}"),
        });
        let code = extract_code(&err_body.error, &err_body.message);
        eprintln!("{}", stderr_for_vision_error(code));
        std::process::exit(exit_code_for_vision_error(code));
    }
}

// ── ASR 热词（hotword）后处理 ───────────────────────────────────────────
//
// SenseVoiceSmall（home asr.service）不支持模型级 hotword，靠 Python 后处理
// 正则替换实现。CLI 在这里读写 `~/.fuxi/asr-hotwords.json`，asr_server.py
// 每次 transcribe 前 mtime check 自动 reload——加词不需要 systemctl restart。
//
// 设计意图：玄女在对话中收到「这个词又被识别错了」时，自己跑
// `Bash fuxi xuannv hotword add --match X --replace Y`，下一次用户说话就生效。
// 用户不必登 home 手编 json。

fn hotwords_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".fuxi").join("asr-hotwords.json")
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct HotwordRule {
    #[serde(rename = "match")]
    match_pat: String,
    replace: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    comment: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Default)]
struct HotwordsFile {
    #[serde(default)]
    rules: Vec<HotwordRule>,
}

fn load_hotwords(path: &std::path::Path) -> Result<HotwordsFile> {
    if !path.exists() {
        return Ok(HotwordsFile::default());
    }
    let body =
        std::fs::read_to_string(path).with_context(|| format!("读 {} 失败", path.display()))?;
    let file: HotwordsFile =
        serde_json::from_str(&body).with_context(|| format!("{} 不是合法 JSON", path.display()))?;
    Ok(file)
}

fn save_hotwords(path: &std::path::Path, file: &HotwordsFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("建 {} 目录失败", parent.display()))?;
    }
    let body = serde_json::to_string_pretty(file).context("序列化 hotwords")?;
    std::fs::write(path, body + "\n").with_context(|| format!("写 {} 失败", path.display()))?;
    Ok(())
}

/// 简易正则字面 escape——`--literal` flag 用，等价 Python re.escape 子集。
/// 只 escape 真正的 regex metacharacter，避免误转义中文等普通字符。
fn re_escape_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for ch in s.chars() {
        match ch {
            '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

#[derive(Debug, Args)]
pub struct HotwordAddArgs {
    /// 匹配模式——默认按 Python `re` 正则解释。例：`克劳德[寇口扣][德的]?`
    #[arg(long = "match")]
    pub match_pat: String,
    /// 替换为，例：`claude code`。
    #[arg(long)]
    pub replace: String,
    /// 可选注释，便于 list 时回顾「为啥加的」。
    #[arg(long)]
    pub comment: Option<String>,
    /// 将 --match 当字面字符串而非正则——CLI 会自动 escape regex metachar。
    /// 单纯想替换「麦克」→「mac」这种不含特殊字符的词建议用 --literal 避免歧义。
    /// 注意：纯字面替换会无视词边界——「麦克风」也会被改成「mac风」。需要
    /// 边界保护时不要用 --literal，而是直接传带 lookahead 的正则。
    #[arg(long)]
    pub literal: bool,
}

pub async fn run_hotword_add(args: HotwordAddArgs) -> Result<()> {
    let match_pat = if args.literal {
        re_escape_literal(&args.match_pat)
    } else {
        args.match_pat.clone()
    };
    if match_pat.trim().is_empty() {
        anyhow::bail!("--match 为空");
    }
    if args.replace.is_empty() {
        // 允许 replace 为空串（用户想删词），不报错
    }

    let path = hotwords_path();
    let mut file = load_hotwords(&path)?;

    // 去重：同 match 视为更新（不堆重复规则）
    let rule = HotwordRule {
        match_pat: match_pat.clone(),
        replace: args.replace.clone(),
        comment: args.comment.unwrap_or_default(),
    };
    let mut updated = false;
    for r in file.rules.iter_mut() {
        if r.match_pat == match_pat {
            *r = rule.clone();
            updated = true;
            break;
        }
    }
    if !updated {
        file.rules.push(rule);
    }
    save_hotwords(&path, &file)?;

    let action = if updated { "更新" } else { "新增" };
    println!(
        "✓ {action}热词 #{}（共 {} 条） {} → {}",
        file.rules.len(),
        file.rules.len(),
        match_pat,
        args.replace
    );
    println!("  文件：{}", path.display());
    println!("  asr.service 下次 transcribe 自动 reload，无需 restart。");
    Ok(())
}

pub async fn run_hotword_list() -> Result<()> {
    let path = hotwords_path();
    let file = load_hotwords(&path)?;
    if file.rules.is_empty() {
        println!("无热词规则（{} 不存在或为空）", path.display());
        return Ok(());
    }
    println!("热词 {} 条（{}）：", file.rules.len(), path.display());
    for (i, r) in file.rules.iter().enumerate() {
        let comment = if r.comment.is_empty() {
            String::new()
        } else {
            format!("  # {}", r.comment)
        };
        println!("  [{i}] {} → {}{comment}", r.match_pat, r.replace);
    }
    Ok(())
}

#[derive(Debug, Args)]
pub struct HotwordRmArgs {
    /// 要删的规则索引（`fuxi xuannv hotword list` 显示的 [N]）。
    pub index: usize,
}

pub async fn run_hotword_rm(args: HotwordRmArgs) -> Result<()> {
    let path = hotwords_path();
    let mut file = load_hotwords(&path)?;
    if args.index >= file.rules.len() {
        anyhow::bail!(
            "index {} 越界（当前 {} 条规则；list 看一下索引）",
            args.index,
            file.rules.len()
        );
    }
    let removed = file.rules.remove(args.index);
    save_hotwords(&path, &file)?;
    println!(
        "✓ 删了 #{}：{} → {}（剩 {} 条）",
        args.index,
        removed.match_pat,
        removed.replace,
        file.rules.len()
    );
    Ok(())
}

// ── 声纹（Phase 5 SV）─────────────────────────────────────────────────
//
// home 上 sv_server.py 端口 9883 跑 CAM++ 中文声纹模型；用户 mac 录一段 wav
// 上传到 home 后调 /enroll 提 embedding 存 ~/.fuxi/voiceprint/owner.npy。
// asr_server.py / wake_server 后续 transcribe / 唤醒时都调 /verify 拦截
// 陌生人声音。fail-open：未注册时所有 verify 全 match=true，注册后才严格。
//
// CLI 端默认 base_url = http://127.0.0.1:9883——玄女在 home 上 cc 进程跑就直
// 连 localhost；要从 mac 跑透 nginx 用 `--base-url https://im.qmledmq.cn:8443/api/sv`。

#[derive(Debug, Args)]
pub struct VoiceprintEnrollArgs {
    /// 16kHz mono wav 文件路径——5-30 秒自然说话语料（不要默念，越多角度越好）。
    /// mac 录：`sox -d -r 16000 -c 1 ~/Downloads/yilin.wav trim 0 20`，
    /// 然后 `scp ~/Downloads/yilin.wav home:/tmp/`，玄女在 home 跑这命令。
    #[arg(long)]
    pub wav: PathBuf,
    /// sv_server base url，默认 localhost；远端调走 nginx 入口（带 /api/sv 前缀）。
    #[arg(long, default_value = "http://127.0.0.1:9883")]
    pub base_url: String,
}

#[derive(Debug, Args)]
pub struct VoiceprintVerifyArgs {
    /// wav 文件路径——同 enroll 要求 16kHz mono。
    #[arg(long)]
    pub wav: PathBuf,
    #[arg(long, default_value = "http://127.0.0.1:9883")]
    pub base_url: String,
}

#[derive(Debug, Args)]
pub struct VoiceprintStatusArgs {
    #[arg(long, default_value = "http://127.0.0.1:9883")]
    pub base_url: String,
}

fn mint_sv_token() -> Result<String> {
    use fuxi_im::auth::{HmacSecret, TokenClaims, sign_token};
    let secret = HmacSecret::load_or_create_default()
        .context("加载 ~/.fuxi/im_hmac.key 失败——home 上 sv_server 用同款 HMAC")?;
    let claims = TokenClaims {
        device_id: format!("voiceprint-cli-{}", uuid::Uuid::new_v4()),
        name: "voiceprint-cli".into(),
        expires_at: chrono::Utc::now() + chrono::Duration::seconds(60),
    };
    sign_token(&secret, &claims).context("HMAC 签 token 失败")
}

async fn sv_post(url: &str, body: serde_json::Value, token: &str) -> Result<serde_json::Value> {
    let resp = reqwest::Client::new()
        .post(url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {url} 失败"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("sv {} → {}: {}", url, status, text);
    }
    Ok(serde_json::from_str(&text).unwrap_or(serde_json::json!({"raw": text})))
}

async fn sv_get(url: &str, token: &str) -> Result<serde_json::Value> {
    let resp = reqwest::Client::new()
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("GET {url} 失败"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("sv {} → {}: {}", url, status, text);
    }
    Ok(serde_json::from_str(&text).unwrap_or(serde_json::json!({"raw": text})))
}

fn read_wav_b64(path: &std::path::Path) -> Result<String> {
    use base64::Engine;
    let bytes = std::fs::read(path).with_context(|| format!("读 {} 失败", path.display()))?;
    if bytes.len() < 8 || &bytes[0..4] != b"RIFF" {
        anyhow::bail!(
            "{} 不像 wav（无 RIFF header）——必须 16kHz mono wav，先用 sox / ffmpeg 转",
            path.display()
        );
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

pub async fn run_voiceprint_enroll(args: VoiceprintEnrollArgs) -> Result<()> {
    if !args.wav.exists() {
        anyhow::bail!("wav 不存在：{}", args.wav.display());
    }
    let wav_b64 = read_wav_b64(&args.wav)?;
    let token = mint_sv_token()?;
    let url = format!("{}/enroll", args.base_url.trim_end_matches('/'));
    let resp = sv_post(&url, serde_json::json!({"wav_b64": wav_b64}), &token).await?;
    println!("✓ 注册 OK");
    println!(
        "{}",
        serde_json::to_string_pretty(&resp).unwrap_or_default()
    );
    Ok(())
}

pub async fn run_voiceprint_verify(args: VoiceprintVerifyArgs) -> Result<()> {
    if !args.wav.exists() {
        anyhow::bail!("wav 不存在：{}", args.wav.display());
    }
    let wav_b64 = read_wav_b64(&args.wav)?;
    let token = mint_sv_token()?;
    let url = format!("{}/verify", args.base_url.trim_end_matches('/'));
    let resp = sv_post(&url, serde_json::json!({"wav_b64": wav_b64}), &token).await?;
    let score = resp.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let threshold = resp
        .get("threshold")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.3);
    let match_ = resp.get("match").and_then(|v| v.as_bool()).unwrap_or(false);
    let enrolled = resp
        .get("enrolled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mark = if match_ { "✓" } else { "✗" };
    println!("{mark} match={match_} score={score:.3} threshold={threshold} enrolled={enrolled}");
    if !enrolled {
        println!("  → 未注册：所有 verify 强制返 true（fail-open）。先跑 voiceprint enroll。");
    }
    Ok(())
}

pub async fn run_voiceprint_status(args: VoiceprintStatusArgs) -> Result<()> {
    let token = mint_sv_token()?;
    let url = format!("{}/healthz", args.base_url.trim_end_matches('/'));
    let resp = sv_get(&url, &token).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&resp).unwrap_or_default()
    );
    Ok(())
}

pub async fn run_refresh() -> Result<()> {
    let path = std::env::var("FUXI_EVENTS_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| default_events_db_path());

    if !path.exists() {
        println!("策府数据库不存在：{}", path.display());
        println!("→ 没历史 session record 可清——下次 fuxi-im 启动自动 fresh.");
        return Ok(());
    }

    let oracle = OracleStore::connect_file(&path)
        .await
        .with_context(|| format!("打开策府 {}", path.display()))?;

    let cleared = session::forget_xuannv_session(&oracle)
        .await
        .context("清 oracle xuannv session record 失败")?;

    if cleared == 0 {
        println!("没找到玄女 session record（之前就是 fresh）。");
    } else {
        println!("已清 {cleared} 条玄女 session record。");
    }
    println!();
    println!("下一步：");
    println!("  systemctl --user restart fuxi-im");
    println!();
    println!("（触发 ensure_xuannv 走 fresh session 路径，cc 重读");
    println!("  `--append-system-prompt`，含 dispatch-routing.md 最新版）");
    Ok(())
}

#[cfg(test)]
mod vision_tests {
    use super::*;

    /// spec §错误兜底矩阵 + β 桌宠端 frame error 扩展：
    /// 每个 code 严格映射到固定退出码——玄女靠这个判断「该说哪句兜底中文」。
    /// 改这表必须同步 spec + roles/xuannv 提示词。
    #[test]
    fn exit_codes_match_spec_table() {
        assert_eq!(exit_code_for_vision_error("no_pet_connected"), 2);
        assert_eq!(exit_code_for_vision_error("user_denied"), 3);
        assert_eq!(exit_code_for_vision_error("permission_denied"), 4);
        assert_eq!(exit_code_for_vision_error("timeout"), 5);
        assert_eq!(exit_code_for_vision_error("upload_failed"), 6);
        assert_eq!(exit_code_for_vision_error("no_device"), 7);
        assert_eq!(exit_code_for_vision_error("capture_failed"), 8);
        assert_eq!(
            exit_code_for_vision_error("totally_unknown_code"),
            1,
            "unknown 兜底退 1"
        );
    }

    /// stderr 提示文本——玄女按 prelude 教学应原文转告用户，
    /// 文案不允许 silent 改动（破坏 prelude 教学一致性）。
    #[test]
    fn stderr_messages_match_spec_table() {
        assert_eq!(
            stderr_for_vision_error("no_pet_connected"),
            "我现在看不见你（桌宠没连）"
        );
        assert_eq!(
            stderr_for_vision_error("user_denied"),
            "你把我眼睛蒙了，先去右键菜单解锁"
        );
        assert_eq!(
            stderr_for_vision_error("permission_denied"),
            "需要你在系统设置→隐私→屏幕录制里给我权限"
        );
        assert_eq!(
            stderr_for_vision_error("timeout"),
            "拍帧太慢，重新让我看一次？"
        );
        assert_eq!(
            stderr_for_vision_error("upload_failed"),
            "图传不上去，可能网断了"
        );
        assert_eq!(
            stderr_for_vision_error("no_device"),
            "桌宠那边找不到摄像头/屏幕设备"
        );
        assert_eq!(
            stderr_for_vision_error("capture_failed"),
            "拍帧失败，可能桌宠崩了或者权限刚被撤"
        );
    }

    /// extract_code 双路径：
    /// (1) typed Vision 直接命中 ErrorBody.error 字段（α v1.1+ 后端走这条）
    /// (2) 老版本走 message 兜底切尾段（"bad request: user_denied"）
    /// 服务端契约升级后两条都要 work，避免新旧 client/server 错配静默退化。
    #[test]
    fn extract_code_prefers_typed_error_then_falls_back_to_message() {
        // typed 路径：error 字段已是 spec code
        assert_eq!(extract_code("user_denied", ""), "user_denied");
        assert_eq!(extract_code("no_device", ""), "no_device");
        assert_eq!(extract_code("capture_failed", ""), "capture_failed");
        assert_eq!(extract_code("no_pet_connected", ""), "no_pet_connected");
        assert_eq!(extract_code("timeout", "timeout: timeout"), "timeout");
        // 老 fallback：error 字段是泛型 kind（"bad_request"），切 message 尾段
        assert_eq!(
            extract_code("bad_request", "bad request: user_denied"),
            "user_denied"
        );
        assert_eq!(
            extract_code("bad_request", "bad request: no_pet_connected"),
            "no_pet_connected"
        );
        // 都不命中 → 返 error 字段（兜底，至少有点信息）
        assert_eq!(extract_code("internal", "internal: 啥也不是"), "internal");
    }
}

#[cfg(test)]
mod hotword_tests {
    use super::*;

    /// 字面 escape：中文 + 数字 + 字母原样保留，metachar 加 `\`。
    #[test]
    fn re_escape_literal_metachar_only() {
        assert_eq!(re_escape_literal("麦克"), "麦克");
        assert_eq!(re_escape_literal("a.b"), "a\\.b");
        assert_eq!(re_escape_literal("v1.0"), "v1\\.0");
        assert_eq!(re_escape_literal("(test)"), "\\(test\\)");
        assert_eq!(re_escape_literal("a+b*c?"), "a\\+b\\*c\\?");
    }

    /// add → list → rm round-trip：写 tmp dir，避免污染真 ~/.fuxi/。
    /// 同 match 二次 add 应 update 而非追加。
    #[test]
    fn hotwords_file_roundtrip_and_dedup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("asr-hotwords.json");

        // 空文件不存在 → load 返空
        let f = load_hotwords(&path).expect("load empty");
        assert!(f.rules.is_empty());

        // 写一条
        let mut f = HotwordsFile::default();
        f.rules.push(HotwordRule {
            match_pat: "克劳德[寇口扣][德的]?".into(),
            replace: "claude code".into(),
            comment: "test".into(),
        });
        save_hotwords(&path, &f).expect("save");

        // load 回来字段无丢
        let loaded = load_hotwords(&path).expect("load");
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.rules[0].match_pat, "克劳德[寇口扣][德的]?");
        assert_eq!(loaded.rules[0].replace, "claude code");
        assert_eq!(loaded.rules[0].comment, "test");

        // JSON 形态符合契约——key 是 "match" 不是 "match_pat"，asr_server.py 要这个
        let body = std::fs::read_to_string(&path).expect("read");
        assert!(
            body.contains("\"match\""),
            "json 必须有 match key（asr_server.py 读这个），got:\n{body}"
        );
        assert!(!body.contains("match_pat"));
    }
}

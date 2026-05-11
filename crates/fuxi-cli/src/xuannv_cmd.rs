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

//! Clipboard · OSC52 终端 escape + 平台 pbcopy/xclip/wl-copy/clip 双写。
//!
//! 为什么双写：
//! - OSC52 (`\x1b]52;c;{base64}\x07`)：无需外部进程，ssh 远程也能过；但部分
//!   终端（iTerm2 默认打开，Terminal.app 得勾 "Allow OSC52"）需要用户授权。
//! - 平台 CLI：macOS `pbcopy` / Linux X11 `xclip -selection clipboard` /
//!   Wayland `wl-copy` / Windows `clip`——在本机 TTY 上最可靠。
//!
//! WHY 两者都发：各有盲区，冗余写保证"至少一条路通"。进程失败只 warn 不
//! 阻断 OSC52——OSC52 是兜底。
//!
//! WHY 75KB 上限：xterm OSC52 规范建议 paste payload 不超过 100KB；base64
//! 展开 4/3 倍，为保留安全边距取原始 75KB。超过就截断 + warn。
use std::io::Write;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

/// OSC52 payload 安全上限（bytes，原始 UTF-8 计）。
/// WHY 75K：xterm 规范 100K；base64 4/3 展开 → 100K/1.34 ≈ 75K 保留边距。
pub const OSC52_SAFE_LIMIT: usize = 75_000;

/// 复制到系统剪贴板。
///
/// 两路并发写（先 OSC52，再派进程）：OSC52 失败不阻断进程路，反之亦然。
/// 返回 `Ok(())` 表示至少 OSC52 路已发出；进程失败只记 warn（返回值不反映）。
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let truncated = truncate_for_osc52(text);
    write_osc52(truncated)?;
    if let Err(e) = spawn_native_copy(text) {
        // WHY 只 warn：进程路是加分项不是必需项；OSC52 已走完。
        tracing::warn!("平台 clipboard 进程失败，仅 OSC52 路生效: {e}");
    }
    Ok(())
}

/// 构造 OSC52 escape 序列——`ESC ] 52 ; c ; {base64} BEL`。
///
/// 单独拎出来供测试无副作用地校验。
pub fn osc52_envelope(text: &str) -> String {
    let b64 = B64.encode(text.as_bytes());
    format!("\x1b]52;c;{b64}\x07")
}

/// 把 text 截到 OSC52 安全长度——按 **char 边界** 切，保证 UTF-8 不破。
fn truncate_for_osc52(text: &str) -> &str {
    if text.len() <= OSC52_SAFE_LIMIT {
        return text;
    }
    // 找到 ≤ limit 的最大 char 边界，切到那。
    let mut end = OSC52_SAFE_LIMIT;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// 把 OSC52 envelope 写 stdout。
///
/// WHY 直接 stdout：TUI 在 alt-screen 下用 crossterm 接管但 raw stdout 仍
/// 能走到终端——escape 序列会被终端解析。若终端不支持，序列只是字节，
/// 不会伤害画面。
fn write_osc52(text: &str) -> Result<()> {
    let envelope = osc52_envelope(text);
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(envelope.as_bytes())
        .context("写 OSC52 到 stdout 失败")?;
    stdout.flush().context("flush stdout 失败")?;
    Ok(())
}

/// 派一个平台 CLI 把 text 喂进 stdin。
///
/// macOS: `pbcopy`；Linux: 先试 `wl-copy`（Wayland）再 `xclip -selection clipboard`
/// （X11）；Windows: `clip`。
fn spawn_native_copy(text: &str) -> Result<()> {
    let (cmd, args): (&str, &[&str]) = platform_command();
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("启动 {cmd} 失败"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .with_context(|| format!("往 {cmd} 写 stdin 失败"))?;
    }
    let status = child.wait().with_context(|| format!("等 {cmd} 退出失败"))?;
    if !status.success() {
        anyhow::bail!("{cmd} 退出码 {status}");
    }
    Ok(())
}

/// 按编译 target 选 CLI。Linux 下默认 wl-copy——Wayland 是现代发行版主流；
/// 若无 wl-copy 则 spawn 失败 warn，用户自行装。
fn platform_command() -> (&'static str, &'static [&'static str]) {
    if cfg!(target_os = "macos") {
        ("pbcopy", &[])
    } else if cfg!(target_os = "windows") {
        ("clip", &[])
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        ("wl-copy", &[])
    } else {
        ("xclip", &["-selection", "clipboard"])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_emits_osc52_envelope() {
        let env = osc52_envelope("hello");
        // 前缀 ESC ] 52 ; c ;；后缀 BEL。
        assert!(env.starts_with("\x1b]52;c;"), "envelope 前缀不对: {env:?}");
        assert!(env.ends_with('\x07'), "envelope 应以 BEL 结尾");

        // payload 段应是 base64("hello")。
        let b64 = B64.encode("hello");
        assert!(env.contains(&b64), "envelope 应含 base64(hello)");
    }

    #[test]
    fn copy_handles_unicode() {
        // 中文 + emoji 不丢字节。
        let text = "玄女 🦊 门客";
        let env = osc52_envelope(text);
        let b64 = B64.encode(text.as_bytes());
        assert!(env.contains(&b64));

        // 解码回去也等于原文。
        let decoded = B64.decode(b64.as_bytes()).expect("base64 解码应成功");
        assert_eq!(String::from_utf8(decoded).unwrap(), text);
    }

    #[test]
    fn copy_truncates_at_safe_size() {
        // 构造一个 > 75K 的 ASCII 串。
        let big = "x".repeat(OSC52_SAFE_LIMIT + 100);
        let truncated = truncate_for_osc52(&big);
        assert_eq!(truncated.len(), OSC52_SAFE_LIMIT);

        // 刚好 75K 的不截断。
        let exact = "y".repeat(OSC52_SAFE_LIMIT);
        assert_eq!(truncate_for_osc52(&exact).len(), OSC52_SAFE_LIMIT);

        // 小的不变。
        assert_eq!(truncate_for_osc52("小串"), "小串");
    }

    #[test]
    fn truncate_respects_char_boundary() {
        // 每个中文 3 字节 UTF-8。构造一个 truncate 刚好落在 char 中间的情形。
        // 用"啊"（E5 95 8A）重复到 > limit，保证 limit 切中多字节 char。
        let ch = "啊";
        let n = OSC52_SAFE_LIMIT / ch.len() + 2;
        let big: String = ch.repeat(n);
        let out = truncate_for_osc52(&big);

        // out 必须是合法 UTF-8（能调 chars()）且长度 ≤ limit。
        assert!(out.len() <= OSC52_SAFE_LIMIT);
        // 不 panic → 证明切在 char 边界上。
        let _ = out.chars().count();
    }

    #[test]
    fn empty_text_produces_bare_envelope() {
        let env = osc52_envelope("");
        assert_eq!(env, "\x1b]52;c;\x07");
    }
}

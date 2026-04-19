//! `fuxi`（无参）—— v0.1 用户唯一入口：REPL TUI。
//!
//! 设计锚：`docs/superpowers/specs/2026-04-19-v0.1-scenario.md` §1 + §2.2 薄片 D
//! + `docs/session-review-2026-04-19-afternoon.md` §5。
//!
//! 用户视角铁律（不是 Firehose 观察器）：
//! - 用户只跟**玄女**对话，不直视门客
//! - 对话区展示的是 user 输入 + 玄女回话；门客的 tool_call / thinking 全塞事件区
//! - 底部输入框收键盘 → 每条按 Enter 走 `Fuxi::dispatch(xuannv, new Task)`——
//!   这样每个 turn 都重新挂一次 active_tx，事件链路稳定
//!
//! v0.1 范围（afternoon §5.1）：单区对话 + 事件 + 输入。三栏 / 鼠标 / 任务树全部
//! defer v0.2。但内部把 AgentResponded 按 agent 分流，为 v0.2 的 ConversationSwitch
//! 留口。

use crate::daemon::Daemon;
use crate::ipc;
use crate::skill_loader;
use anyhow::{Context, Result, anyhow};
use clap::Args as ClapArgs;
use crossterm::event::{self, Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures_util::StreamExt;
use fuxi_agent_cc::CcLaunchConfig;
use fuxi_core::event::{Event, EventKind, EventMeta};
use fuxi_core::id::AgentId;
use fuxi_core::task::Task;
use fuxi_events::EventBus;
use fuxi_firehose::{FirehoseApp, Hub};
use fuxi_orchestrator::{Fuxi, FuxiConfig, WorkerKind};
use fuxi_workspace::GitWorktreeWorkspace;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// 对话区最多保留多少行——更早的自动丢。v0.1 拍 500，人眼滚得动的上限。
const DIALOGUE_CAP: usize = 500;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// 覆盖 Hub HTTP 监听地址。默认同 `fuxi up`，便于并行 `fuxi watch` 外部观察。
    #[arg(long, default_value = "127.0.0.1:4100")]
    pub bind: SocketAddr,
    /// Unix socket 路径覆盖（给玄女的 Bash 工具用）。默认 `$FUXI_SOCK` / `/tmp/fuxi.sock`。
    #[arg(long)]
    pub sock_path: Option<PathBuf>,
    /// 工作区根（worktree 存哪里）。默认当前目录。
    #[arg(long, default_value = ".")]
    pub workspace_root: PathBuf,
    /// 门客是否分配 worktree。REPL 默认关掉——玄女当前只下发 Bash 命令，不写代码。
    #[arg(long, default_value_t = false)]
    pub allocate_worktree: bool,
    /// 玄女的 role（skills/<role>/SKILL.md）。默认 `xuannv`。
    #[arg(long, default_value = "xuannv")]
    pub xuannv_role: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:4100".parse().expect("static socket addr"),
            sock_path: None,
            workspace_root: PathBuf::from("."),
            allocate_worktree: false,
            xuannv_role: "xuannv".to_string(),
        }
    }
}

/// 在 PATH 中找指定 binary。`path_env` 抽出来是为了单测可以注入合成 PATH，
/// 不污染全进程环境变量（`std::env::set_var` 在多线程并发跑测试时不安全）。
pub fn find_in_path(name: &str, path_env: Option<&OsStr>) -> Option<PathBuf> {
    let path_env = path_env?;
    for dir in std::env::split_paths(path_env) {
        let candidate = dir.join(name);
        if !candidate.is_file() {
            continue;
        }
        // Unix 下要可执行才算数；非 Unix 见到文件就当数。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&candidate)
                && meta.permissions().mode() & 0o111 != 0
            {
                return Some(candidate);
            }
        }
        #[cfg(not(unix))]
        {
            return Some(candidate);
        }
    }
    None
}

/// 入装预检：`fuxi` binary 必须在 PATH 中，否则玄女的 Bash 工具调 `fuxi ...` 会
/// `command not found`，整个 platform 失语。失败时 error message 必须明确指向
/// `scripts/install.sh`，不许让用户自己猜要装什么。
pub fn require_fuxi_in_path(name: &str, path_env: Option<&OsStr>) -> Result<PathBuf> {
    if let Some(p) = find_in_path(name, path_env) {
        return Ok(p);
    }
    Err(anyhow!(
        "找不到 `{name}` 二进制（玄女的工具底座）。请先安装：\n\
         \n    ./scripts/install.sh\n\n\
         它会跑 `cargo install --path crates/fuxi-cli --force`，把 `fuxi` 装到 \
         `~/.cargo/bin/`。装完后 `which fuxi` 应返回路径，再重启 fuxi。"
    ))
}

pub async fn run(args: Args) -> Result<()> {
    // 入装预检——玄女的 Bash 工具会调 `fuxi spawn/dispatch ...`，shell 找不到 fuxi
    // 就全盘瘫痪。所以在 TUI 起来之前先 fail-fast，让用户先跑 install.sh。
    require_fuxi_in_path("fuxi", std::env::var_os("PATH").as_deref())?;

    // skills 预检——找不到直接报错提示，不要进 TUI 再卡死
    if skill_loader::skills_root().is_none() {
        return Err(anyhow!(
            "找不到 skills 目录：试过 $FUXI_SKILLS_DIR / git-root/skills / ./skills / $HOME/.fuxi/skills 都不在。\n\
             建议：export FUXI_SKILLS_DIR=/Users/e0_7/fuxi/skills  （或把 fuxi/skills 软链到 ~/.fuxi/skills）"
        ));
    }

    // 1. EventBus + Fuxi orchestrator
    let bus = EventBus::with_memory_store()
        .await
        .context("创建内存 EventBus 失败")?;
    let workspace = Arc::new(GitWorktreeWorkspace::with_default_base(
        args.workspace_root.clone(),
    ));
    let fuxi = Arc::new(Fuxi::with_config(
        bus.clone(),
        workspace,
        FuxiConfig {
            allocate_worktree: args.allocate_worktree,
            ..Default::default()
        },
    ));

    // 2. Firehose Hub（给 `fuxi watch` 外部观察留口；REPL 自己不用它）
    let hub = Arc::new(Hub::new(bus.clone()));
    let app_router = fuxi_firehose::hub::router(hub);
    let hub_listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("bind {} 失败", args.bind))?;
    let hub_task = tokio::spawn(async move {
        if let Err(e) = axum::serve(hub_listener, app_router).await {
            tracing::warn!(error = %e, "hub serve 退出");
        }
    });

    // 3. daemon socket——玄女 Bash `fuxi dispatch/intervene ...` 走这里
    let sock_path = args.sock_path.clone().unwrap_or_else(ipc::socket_path);
    // SAFETY: daemon::serve 自己会 parent-dir ensure + 清残留；这里只把路径传进 env
    // 让 cc 子进程继承到 $FUXI_SOCK
    unsafe {
        std::env::set_var("FUXI_SOCK", &sock_path);
    }
    let daemon = Daemon::new(fuxi.clone());
    let daemon_shutdown = daemon.shutdown_handle();
    let sock_for_task = sock_path.clone();
    let daemon_task = tokio::spawn(async move {
        if let Err(e) = daemon.serve(&sock_for_task).await {
            tracing::warn!(error = %e, "daemon serve 异常");
        }
    });

    // 4. 发 PlatformStarted 事件——REPL 启动点
    let _ = bus.publish(Event {
        meta: EventMeta::now(),
        kind: EventKind::PlatformStarted {
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    });

    // 5. spawn 玄女——从 skills/<role>/SKILL.md 读 profile
    let loaded = skill_loader::load(&args.xuannv_role)
        .with_context(|| format!("加载 skills/{}/SKILL.md", args.xuannv_role))?;
    let xuannv_profile = loaded.profile.clone();
    let cc_cfg = CcLaunchConfig {
        append_system_prompt: if loaded.append_system_prompt.is_empty() {
            None
        } else {
            Some(loaded.append_system_prompt)
        },
        allowed_tools: loaded.allowed_tools,
        ..Default::default()
    };
    let xuannv_id = fuxi
        .spawn_worker(xuannv_profile, WorkerKind::Cc(cc_cfg))
        .await
        .context("玄女 spawn 失败")?;
    tracing::info!(xuannv = %xuannv_id, "玄女已就绪");

    // 6. 发 greet task 让玄女主动开口（cc headless 没 prompt 不说话）
    let greet = Task::new(
        "greet",
        "用户刚启动 fuxi REPL。请用一句话（十字以内）主动问好，邀请用户提需求。不要自我介绍。",
    );
    if let Err(e) = fuxi.dispatch(xuannv_id, greet).await {
        tracing::warn!(error = %e, "greet dispatch 失败，继续");
    }

    // 7. 进入 TUI 主循环
    let outcome = drive_tui(bus, fuxi.clone(), xuannv_id).await;

    // 8. 收尾——无论 loop 怎么退都走 shutdown
    daemon_shutdown.notify_waiters();
    if let Err(e) = fuxi.shutdown().await {
        tracing::warn!(error = %e, "fuxi shutdown 部分失败");
    }
    // 事件 flush 窗口
    tokio::time::sleep(Duration::from_millis(80)).await;
    hub_task.abort();
    daemon_task.abort();

    outcome
}

/// 一条对话行——要么是用户输入，要么是玄女说的话。
#[derive(Debug, Clone)]
enum DialogueLine {
    User(String),
    Xuannv(String),
    System(String),
}

/// REPL TUI 的核心状态。
struct ReplApp {
    xuannv_id: AgentId,
    dialogue: VecDeque<DialogueLine>,
    /// 复用 FirehoseApp 的事件流视图——但我们接管 key handling，不让它吃键。
    events: FirehoseApp,
    input: String,
    should_quit: bool,
    /// 按过一次 Ctrl-C 后显示提示；再按一次才真退。
    confirm_quit: bool,
    /// 玄女是否还在回话中——`thinking_started` 未关闭前为 true，仅作 UI 提示。
    xuannv_thinking: bool,
}

impl ReplApp {
    fn new(xuannv_id: AgentId) -> Self {
        Self {
            xuannv_id,
            dialogue: VecDeque::with_capacity(64),
            events: FirehoseApp::new(),
            input: String::new(),
            should_quit: false,
            confirm_quit: false,
            xuannv_thinking: false,
        }
    }

    fn push_line(&mut self, line: DialogueLine) {
        if self.dialogue.len() == DIALOGUE_CAP {
            self.dialogue.pop_front();
        }
        self.dialogue.push_back(line);
    }

    fn ingest(&mut self, ev: &Event) {
        // 事件流窗口总是喂——让用户看得到全景
        self.events.ingest(ev);

        // 对话区只吃玄女的话 + UserPrompted（用户的话）
        let is_xuannv = ev.meta.agent == Some(self.xuannv_id);
        match &ev.kind {
            EventKind::UserPrompted { text } => {
                self.push_line(DialogueLine::User(text.clone()));
            }
            EventKind::AgentResponded { text } if is_xuannv => {
                self.push_line(DialogueLine::Xuannv(text.clone()));
            }
            EventKind::ThinkingStarted if is_xuannv => {
                self.xuannv_thinking = true;
            }
            EventKind::ThinkingFinished if is_xuannv => {
                self.xuannv_thinking = false;
            }
            EventKind::AgentDead { cause } if is_xuannv => {
                self.push_line(DialogueLine::System(format!(
                    "⚠ 玄女下线：{cause}。按 Ctrl-C 退出。"
                )));
                self.xuannv_thinking = false;
            }
            _ => {}
        }
    }

    /// 返回 Some(text) 表示有用户输入要提交；否则 None。
    fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) -> Option<String> {
        // Ctrl-C 两段式——第一次提示，第二次真退
        if mods.contains(KeyModifiers::CONTROL) && matches!(code, KeyCode::Char('c')) {
            if self.confirm_quit {
                self.should_quit = true;
            } else {
                self.confirm_quit = true;
                self.push_line(DialogueLine::System("再按一次 Ctrl-C 退出".into()));
            }
            return None;
        }
        // 除 Ctrl-C 外的任何键都重置确认
        self.confirm_quit = false;

        match code {
            KeyCode::Enter => {
                let t = std::mem::take(&mut self.input);
                let trimmed = t.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
                None
            }
            KeyCode::Char(c) => {
                // 过滤控制字符；tab 也不放进去（tab 放 TUI 里会乱对齐）
                if !c.is_control() && c != '\t' {
                    self.input.push(c);
                }
                None
            }
            _ => None,
        }
    }

    fn draw(&mut self, f: &mut ratatui::Frame<'_>) {
        let area = f.area();
        // 布局：对话区（占 40%）/ 事件流（占 55%）/ 输入条（3 行）/ 状态条（1 行）
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(45),
                Constraint::Min(5),
                Constraint::Length(3),
                Constraint::Length(1),
            ])
            .split(area);

        self.draw_dialogue(f, chunks[0]);
        self.draw_events(f, chunks[1]);
        self.draw_input(f, chunks[2]);
        self.draw_status(f, chunks[3]);
    }

    fn draw_dialogue(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let title = if self.xuannv_thinking {
            " 玄女（思考中…） "
        } else {
            " 玄女 "
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::Cyan));

        // 每行前缀染色：User=绿、Xuannv=白 bold、System=黄 italic
        let lines: Vec<Line> = self
            .dialogue
            .iter()
            .flat_map(render_dialogue_lines)
            .collect();

        let para = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            // 只展示末尾——ratatui 的 Paragraph 没 auto-scroll，靠 scroll 偏移人工调
            .scroll((self.dialogue_scroll_offset(area.height), 0));
        f.render_widget(para, area);
    }

    /// 估算滚动偏移——把最新内容顶到底部。
    ///
    /// ratatui 的 Paragraph.wrap 在跨行时高度不是 1:1，这里用 lines 数做近似：
    /// - 可视高度 = area.height - 2（边框占两行）
    /// - 若 lines 数 > 可视高度，scroll = lines - visible
    ///
    /// 长消息被 wrap 时会少滚几行，接受。
    fn dialogue_scroll_offset(&self, area_height: u16) -> u16 {
        let visible = area_height.saturating_sub(2) as usize;
        let total: usize = self
            .dialogue
            .iter()
            .map(|l| render_dialogue_lines(l).len())
            .sum();
        if total > visible {
            (total - visible) as u16
        } else {
            0
        }
    }

    fn draw_events(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
        // 复用 FirehoseApp.draw——但它吃整块 Frame，我们要指定 area
        // 解决方案：Clone 逻辑是不可能的（有 &mut），改成用 firehose 的 block 画法
        // 简化：直接画一个边框 + 用 firehose 的内部 visible_rows 渲染成 List
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" events ")
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // 重用 firehose 的 visible_rows（带 kind_tag + color + summary）
        let rows = self.events.visible_rows();
        let available = inner.height as usize;
        let start = rows.len().saturating_sub(available);
        let lines: Vec<Line> = rows[start..]
            .iter()
            .map(|r| {
                Line::from(vec![
                    Span::styled(r.time.clone(), Style::default().fg(Color::DarkGray)),
                    Span::raw(" "),
                    Span::styled(short_or_pad(&r.who, 10), Style::default().fg(Color::Yellow)),
                    Span::raw(" "),
                    Span::styled(
                        short_or_pad(r.kind_tag, 22),
                        Style::default().fg(r.color).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" "),
                    Span::raw(short_or_pad(
                        &r.summary,
                        inner.width.saturating_sub(36) as usize,
                    )),
                ])
            })
            .collect();

        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        f.render_widget(para, inner);
    }

    fn draw_input(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" 你 ")
            .border_style(Style::default().fg(Color::Green));
        let text = Line::from(vec![
            Span::styled("> ", Style::default().fg(Color::Green)),
            Span::raw(&self.input),
            Span::styled(
                "_",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::SLOW_BLINK),
            ),
        ]);
        let para = Paragraph::new(text).block(block);
        f.render_widget(para, area);
    }

    fn draw_status(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let hint = " Enter 发送 | Backspace 删除 | Ctrl-C 退出 | 日志 /tmp/fuxi.log ";
        let para = Paragraph::new(hint).style(Style::default().fg(Color::Black).bg(Color::Gray));
        f.render_widget(para, area);
    }
}

fn render_dialogue_lines(line: &DialogueLine) -> Vec<Line<'_>> {
    match line {
        DialogueLine::User(t) => t
            .lines()
            .map(|ln| {
                Line::from(vec![
                    Span::styled("你> ", Style::default().fg(Color::Green)),
                    Span::raw(ln.to_string()),
                ])
            })
            .collect(),
        DialogueLine::Xuannv(t) => t
            .lines()
            .map(|ln| {
                Line::from(vec![
                    Span::styled(
                        "玄女> ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(ln.to_string()),
                ])
            })
            .collect(),
        DialogueLine::System(t) => vec![Line::from(Span::styled(
            t.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::ITALIC),
        ))],
    }
}

fn short_or_pad(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count >= max {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    } else {
        let mut out = s.to_string();
        out.push_str(&" ".repeat(max - count));
        out
    }
}

async fn drive_tui(bus: EventBus, fuxi: Arc<Fuxi>, xuannv_id: AgentId) -> Result<()> {
    // **关键**：把 stderr 重定向到文件，否则 tracing / claude inherited stderr /
    // panic 会直接砸到 ratatui 的 alt-screen 上污染对话区。
    // dup2 后 fd 2 指向 /tmp/fuxi.log；tracing subscriber 已绑在 Stderr::new，
    // 它写 fd 2 就自动进文件。
    if let Err(e) = redirect_stderr_to_log("/tmp/fuxi.log") {
        eprintln!("⚠ 无法重定向 stderr 到日志文件: {e}。TUI 可能被日志污染");
    }

    // 装 panic hook 先——raw mode 下 panic 会把终端搞死
    install_panic_hook();

    enable_raw_mode().context("enable_raw_mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = ReplApp::new(xuannv_id);
    let mut stream = bus.subscribe();

    let loop_res: Result<()> = async {
        loop {
            terminal.draw(|f| app.draw(f))?;
            if app.should_quit {
                return Ok(());
            }

            tokio::select! {
                // ── 事件流
                maybe_ev = stream.next() => match maybe_ev {
                    Some(Ok(ev)) => app.ingest(&ev),
                    Some(Err(e)) => tracing::warn!(error = %e, "bus 事件错误"),
                    None => return Ok(()),
                },
                // ── 键盘（阻塞 poll，50ms 超时回到 draw）
                maybe_key = tokio::task::spawn_blocking(|| {
                    if event::poll(Duration::from_millis(50)).unwrap_or(false) {
                        match event::read() {
                            Ok(TermEvent::Key(k)) => Some(k),
                            _ => None,
                        }
                    } else { None }
                }) => {
                    let Ok(Some(key)) = maybe_key else { continue };
                    // 只吃 Press，避免某些 terminal 把 Release 也报上来导致双触发
                    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                        continue;
                    }
                    if let Some(text) = app.handle_key(key.code, key.modifiers) {
                        // 先发 UserPrompted——这条走事件流，TUI 自己订阅时顺便渲染到对话区
                        let _ = bus.publish(Event {
                            meta: {
                                let mut m = EventMeta::now();
                                m.agent = Some(xuannv_id);
                                m
                            },
                            kind: EventKind::UserPrompted { text: text.clone() },
                        });
                        // 再 dispatch：每个 user turn 都挂一次新 active_tx，事件链路不会掉
                        let fuxi_cl = fuxi.clone();
                        let task = Task::new("user-turn", &text);
                        tokio::spawn(async move {
                            if let Err(e) = fuxi_cl.dispatch(xuannv_id, task).await {
                                tracing::warn!(error = %e, "repl dispatch 失败");
                            }
                        });
                    }
                }
            }
        }
    }
    .await;

    // 不论如何先 restore
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    loop_res
}

/// panic 时 best-effort restore terminal；否则 raw mode + alt screen 会把 shell 卡死。
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        prev(info);
    }));
}

/// 把当前进程的 stderr（fd 2）重定向到指定日志文件——Unix 下用 `dup2(2)`.
///
/// 为什么必须：tracing subscriber 写 fd 2；claude 子进程继承 fd 2；ratatui
/// 进 raw + alt-screen 模式时二者直接覆盖画面。重定向后一切都进文件，TUI 画面
/// 干净。
///
/// 非 Unix 直接返回 Err——v0.1 只保障 macOS/Linux。
#[cfg(unix)]
fn redirect_stderr_to_log(path: &str) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::options()
        .create(true)
        .append(true)
        .open(path)?;
    // SAFETY: dup2 对 valid fd 是安全调用；file 在作用域内有效。dup2 之后 fd 2
    // 独立引用 file 的底层 inode，file 被 drop 不影响 fd 2。
    let ret = unsafe { dup2(file.as_raw_fd(), 2) };
    if ret == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(unix))]
fn redirect_stderr_to_log(_path: &str) -> std::io::Result<()> {
    Err(std::io::Error::other("stderr redirect only on unix"))
}

#[cfg(unix)]
unsafe extern "C" {
    fn dup2(oldfd: i32, newfd: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use fuxi_core::event::{Event, EventMeta};

    fn mk_ev(agent: Option<AgentId>, kind: EventKind) -> Event {
        let mut meta = EventMeta::now();
        meta.agent = agent;
        Event { meta, kind }
    }

    #[test]
    fn typing_and_enter_returns_text() {
        let mut app = ReplApp::new(AgentId::new());
        app.handle_key(KeyCode::Char('h'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('i'), KeyModifiers::empty());
        let out = app.handle_key(KeyCode::Enter, KeyModifiers::empty());
        assert_eq!(out.as_deref(), Some("hi"));
        assert!(app.input.is_empty());
    }

    #[test]
    fn empty_enter_returns_none() {
        let mut app = ReplApp::new(AgentId::new());
        assert!(
            app.handle_key(KeyCode::Enter, KeyModifiers::empty())
                .is_none()
        );
    }

    #[test]
    fn backspace_deletes_last_char() {
        let mut app = ReplApp::new(AgentId::new());
        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('b'), KeyModifiers::empty());
        app.handle_key(KeyCode::Backspace, KeyModifiers::empty());
        assert_eq!(app.input, "a");
    }

    #[test]
    fn ctrl_c_requires_double_press_to_quit() {
        let mut app = ReplApp::new(AgentId::new());
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.should_quit);
        assert!(app.confirm_quit);
        // 期间任何其它键重置确认
        app.handle_key(KeyCode::Char('x'), KeyModifiers::empty());
        assert!(!app.confirm_quit);
        // 再两次 Ctrl-C 才真退
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.should_quit);
    }

    #[test]
    fn xuannv_responded_goes_to_dialogue() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        app.ingest(&mk_ev(
            Some(xid),
            EventKind::AgentResponded {
                text: "在。想做什么？".into(),
            },
        ));
        assert_eq!(app.dialogue.len(), 1);
        assert!(matches!(app.dialogue[0], DialogueLine::Xuannv(_)));
    }

    #[test]
    fn other_agent_responded_stays_in_events_only() {
        let xid = AgentId::new();
        let other = AgentId::new();
        let mut app = ReplApp::new(xid);
        app.ingest(&mk_ev(
            Some(other),
            EventKind::AgentResponded {
                text: "dev 门客在干活".into(),
            },
        ));
        assert_eq!(app.dialogue.len(), 0);
    }

    #[test]
    fn user_prompted_event_adds_user_line() {
        let mut app = ReplApp::new(AgentId::new());
        app.ingest(&mk_ev(
            None,
            EventKind::UserPrompted {
                text: "帮我看一下 X".into(),
            },
        ));
        assert_eq!(app.dialogue.len(), 1);
        assert!(matches!(app.dialogue[0], DialogueLine::User(_)));
    }

    #[test]
    fn thinking_toggles_flag() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        app.ingest(&mk_ev(Some(xid), EventKind::ThinkingStarted));
        assert!(app.xuannv_thinking);
        app.ingest(&mk_ev(Some(xid), EventKind::ThinkingFinished));
        assert!(!app.xuannv_thinking);
    }

    #[test]
    fn agent_dead_emits_system_line() {
        let xid = AgentId::new();
        let mut app = ReplApp::new(xid);
        app.ingest(&mk_ev(
            Some(xid),
            EventKind::AgentDead {
                cause: "cc exited".into(),
            },
        ));
        assert_eq!(app.dialogue.len(), 1);
        assert!(matches!(app.dialogue[0], DialogueLine::System(_)));
    }

    #[test]
    fn dialogue_cap_evicts_oldest() {
        let mut app = ReplApp::new(AgentId::new());
        for i in 0..(DIALOGUE_CAP + 10) {
            app.push_line(DialogueLine::System(format!("line-{i}")));
        }
        assert_eq!(app.dialogue.len(), DIALOGUE_CAP);
    }

    #[test]
    fn control_chars_in_input_are_ignored() {
        let mut app = ReplApp::new(AgentId::new());
        app.handle_key(KeyCode::Char('\0'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('\t'), KeyModifiers::empty());
        app.handle_key(KeyCode::Char('a'), KeyModifiers::empty());
        assert_eq!(app.input, "a");
    }

    // ───────── PATH 探测：保证 `fuxi` binary 在玄女能调到的位置 ─────────

    #[test]
    fn require_fuxi_in_path_errors_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path_env = dir.path().as_os_str();
        let res = require_fuxi_in_path("fuxi", Some(path_env));
        assert!(res.is_err(), "空 PATH 应当失败");
        let msg = res.unwrap_err().to_string();
        // error message 必须指向 install.sh，不让用户自己猜
        assert!(
            msg.contains("scripts/install.sh"),
            "error 必须指向 scripts/install.sh；实际：{msg}"
        );
    }

    #[test]
    fn require_fuxi_in_path_errors_when_path_env_unset() {
        let res = require_fuxi_in_path("fuxi", None);
        assert!(res.is_err(), "无 PATH env 也应当失败");
        assert!(res.unwrap_err().to_string().contains("scripts/install.sh"));
    }

    #[cfg(unix)]
    #[test]
    fn require_fuxi_in_path_finds_executable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin_path = dir.path().join("fuxi");
        std::fs::write(&bin_path, "#!/bin/sh\necho ok\n").unwrap();
        std::fs::set_permissions(&bin_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let res = require_fuxi_in_path("fuxi", Some(dir.path().as_os_str()));
        assert!(res.is_ok(), "应找到 binary；实际：{res:?}");
    }

    #[cfg(unix)]
    #[test]
    fn require_fuxi_in_path_skips_non_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let bin_path = dir.path().join("fuxi");
        // 写入但不给可执行位——chmod 默认 644
        std::fs::write(&bin_path, b"not exe").unwrap();
        let res = require_fuxi_in_path("fuxi", Some(dir.path().as_os_str()));
        assert!(res.is_err(), "无 +x 的文件不应被接受");
    }
}

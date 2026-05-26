//! `fuxi` 二进制入口。
//!
//! v0.1 子命令：
//! - `fuxi`（无参）—— 进入 REPL，用户直接和玄女对话（**用户视角的入口**）
//! - `fuxi demo` —— 端到端最小演示（P1 遗产，验证 cc 链路）
//! - `fuxi up` —— 平台长跑：EventBus + Firehose Hub + **daemon Unix socket**
//! - `fuxi watch` —— 连 Hub 打开 TUI 观察器
//! - `fuxi spawn/dispatch/intervene/status/list/kill` —— **玄女的工具子命令**
//!   （玄女的 CC 实例通过 Bash 调它们，人类一般不直接用）
//!
//! 用户视角铁律（见 `docs/superpowers/specs/2026-04-19-v0.1-scenario.md §1`）：
//! **用户只跟玄女对话**。这些子命令对玄女可见、对用户不可见。

use clap::{Parser, Subcommand};
use fuxi_cli::{
    banner, bug_cmd, demo, dist, insight_cmd, issue_cmd, memory_cmd, note_cmd, profile_cmd,
    project_cmd, repl, skill, subcommands, theme, topic_cmd, up, watch, xuannv_cmd,
};

#[derive(Debug, Parser)]
#[command(name = "fuxi", version, about = "伏羲·玄女门客军团的指挥台", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 端到端演示：spawn 一个 cc 门客，执行 prompt，实时打印事件。
    Demo(demo::Args),
    /// 启动伏羲平台（EventBus + Firehose Hub + daemon）长跑。
    Up(up::Args),
    /// 连上运行中的 Hub，打开 TUI 观察器。
    Watch(watch::Args),
    /// 【玄女工具】起一个门客。
    Spawn(subcommands::SpawnArgs),
    /// 【玄女工具】把任务派给指定门客。
    Dispatch(subcommands::DispatchArgs),
    /// 【玄女工具】向门客发话（追加式 / 打断式）。
    Intervene(subcommands::InterveneArgs),
    /// 【玄女工具】查看门客状态。
    Status(subcommands::StatusArgs),
    /// 【玄女工具】列出所有门客。
    List(subcommands::ListArgs),
    /// 【玄女工具】列出所有 dist worker 节点（拓扑视图）。
    Nodes(subcommands::NodesArgs),
    /// 【玄女工具】关停指定门客。
    Kill(subcommands::KillArgs),
    /// 【救急】直读 SQLite 看事件流（不走 daemon）。
    Events(subcommands::EventsArgs),
    /// 【玄女工具】请示用户前标记任务 Blocked。
    Block(subcommands::BlockArgs),
    /// 【玄女工具】task 资源动作（unblock 等）。
    #[command(subcommand)]
    Task(TaskCmd),
    /// 【已弃用】用 `fuxi task unblock` 代替；下个版本删除。
    Resume(subcommands::ResumeArgs),
    /// 点将台管理：list / stage / approve / reject / activate。
    Skill(skill::SkillArgs),
    /// 分布式节点：controller 入队 / worker 拉活回传（codex）
    #[command(subcommand)]
    Dist(dist::DistCmd),
    /// 【玄女工具】策府记忆存取（甲骨 + 河图洛书）。
    Memory(memory_cmd::MemoryArgs),
    /// 【玄女工具】更漏——trigger 管理（cron / once / fs / webhook）。
    #[command(subcommand)]
    Cron(CronCmd),
    /// IM 后端服务（家用部署：systemd 跑 `fuxi im start`）。
    #[command(subcommand)]
    Im(ImCmd),
    /// 项目管理：注册 / 列出 / 删除（Decision 21）。
    #[command(subcommand)]
    Project(ProjectCmd),
    /// 文件级交付产物（Decision 22）—— 手动 produce 用。
    #[command(subcommand)]
    Deliverable(DeliverableCmd),
    /// 【门客工具】轻量文件推送 —— 把一段小 markdown/纯文本直接贴进任务对话流。
    /// `fuxi note --task <id> [--from <agent-uuid>] <file>`。≤ 256KB；超限走 deliverable。
    Note(note_cmd::NoteArgs),
    /// 【玄女工具】用户身份卡（user_profile）—— set/get/list/unset。
    /// 跟 `fuxi memory record` 严格分流（事实流 vs 身份卡），spawn 注入用 summary 层。
    Profile(profile_cmd::ProfileArgs),
    /// 【玄女只读·仓颉写】河图洛书心法（hetu insight）—— list/record/supersede。
    /// 论文：抽象度决定可迁移性；spawn 注入门客 prompt 用。
    Insight(insight_cmd::InsightArgs),
    /// L3 持久 sandbox 管理（Decision 21）—— per-门客 per-project 长期工作区。
    #[command(subcommand)]
    Sandbox(SandboxCmd),
    /// 玄女控制——刷新教学（让她下次 fresh session 加载 dispatch-routing 最新版）。
    #[command(subcommand)]
    Xuannv(XuannvCmd),
    /// Phase 1 · topic 一等公民：new / list / switch / archive。
    Topic(topic_cmd::TopicArgs),
    /// 【玄女工具】上报 fuxi 平台 bug / 改进建议——落 PWA 通知 tab。
    Bug(bug_cmd::BugArgs),
    /// 【Claude/玄女工具】issue 工作流——list / show / close / reopen / link-fix。
    Issue(issue_cmd::IssueArgs),
    /// 【调试】打印启动 banner 后退出——给主人挑样式用。
    #[command(hide = true)]
    Banner,
}

#[derive(Debug, Subcommand)]
enum ProjectCmd {
    /// 注册一个 project（`fuxi project add <canonical-path> [--name <slug>]`）。
    Add(project_cmd::ProjectAddArgs),
    /// 列出所有已注册 project。
    List(project_cmd::ProjectListArgs),
    /// v2 跨节点 sandbox：加入 home 上已注册的 project（git clone + 本机 add +
    /// 通告 home host_nodes）。
    /// `fuxi project join --slug erp --controller https://im.qmledmq.cn:8443 \
    ///                    --token $TOKEN --remote-url ssh://home/home/e0-7/erp`
    Join(project_cmd::ProjectJoinArgs),
    /// 一屏显示 project 元信息 + sandboxes 数 + 交付数。
    Info(project_cmd::ProjectInfoArgs),
    /// 删除一个 project（连带 sandboxes / ephemeral / archive / deliverables）。
    Rm(project_cmd::ProjectRemoveArgs),
}

#[derive(Debug, Subcommand)]
enum DeliverableCmd {
    /// 手动产生一条 deliverable——给某 project 的某 task 落一组文件。
    /// `fuxi deliverable produce --project erp --task task-... --kind research_summary file1 file2`
    Produce(project_cmd::DeliverableProduceArgs),
}

#[derive(Debug, Subcommand)]
enum SandboxCmd {
    /// 列出某项目的所有 L3 持久 sandbox。
    /// `fuxi sandbox list --project erp`
    List(project_cmd::SandboxListArgs),
    /// 退役某项目下某 role 的 L3 sandbox（destructive：丢未 commit 的 WIP）。
    /// `fuxi sandbox retire --project erp --role luban`
    Retire(project_cmd::SandboxRetireArgs),
    /// 扫归档区删过期 L2 ephemeral worktree（Decision 21 phase 2 GC）。
    /// `fuxi sandbox sweep [--project erp] [--threshold-hours 24]`
    Sweep(project_cmd::SandboxSweepArgs),
}

#[derive(Debug, Subcommand)]
enum XuannvCmd {
    /// 刷新教学：清 oracle 里 xuannv session record，并通过 daemon 关掉当前
    /// 玄女进程。下次 `fuxi-im` 走 ensure_xuannv 时 fresh spawn → cc 重读
    /// `--append-system-prompt`（含 dispatch-routing.md 最新版）。
    Refresh,
    /// 上下文交接（task #8）—— 玄女在自己跨 45% 阈值后跑此命令把
    /// `~/.fuxi/xuannv-handoff.md` 写好；fuxi-im 后端检测到落档 → 等当前
    /// turn idle → kill 当前玄女 + spawn 新玄女并 `--append-system-prompt`
    /// 注入 prelude（handoff 内容）。
    #[command(subcommand)]
    Handoff(HandoffCmd),
    /// Jarvis 语音模式（**收到 `[语音]` 前缀消息时必调，公理 #8**）：
    /// 玄女把"想直接念给用户听的一句话"通过此命令上发——daemon publish
    /// `XuannvVoiceLine` 事件 + 注入 `meta.agent=xuannv_id`，macOS App 订
    /// `/api/conv` WS 拿到后调系统 TTS 念出来。
    ///
    /// **不 say 用户耳朵就听不到**——IM 文字是给 PWA 看的，App 听不见。
    /// 文字仍走 IM 正常对话流——这条命令只是"语音侧的副本"，不替代 IM。
    /// 一两句口语，≤500 字（CLI 硬上限会拒），不带 markdown / 代码 / emoji。
    /// `fuxi xuannv say "好的，派给鲁班了"`
    Say(xuannv_cmd::SayArgs),
    /// 玄女眼睛 v1（spec 2026-05-14）——召唤桌宠拍一帧 webcam / screen，
    /// stdout 输出绝对 path 给玄女后续 `Read` 看图。HTTP 阻塞，timeout 默认 10s。
    ///
    /// 触发时机：用户主动说「看看」/「这是什么」/「报错啥意思」时玄女才用，
    /// 不在 idle 期偷看（spec §边界 + roles/xuannv prelude）。
    /// `fuxi xuannv look --target webcam --hint "看看用户的报错"`
    Look(xuannv_cmd::LookArgs),
    /// ASR 热词管理——SenseVoiceSmall 不支持模型级 hotword，靠后处理正则替换。
    /// 玄女遇到用户说「这个词总被识别错」时，自己跑 `hotword add` 加规则，
    /// home asr.service 下次 transcribe 自动 reload（不用 restart 服务）。
    /// 规则文件：`~/.fuxi/asr-hotwords.json`，asr_server.py mtime watch 自动生效。
    #[command(subcommand)]
    Hotword(HotwordCmd),
    /// 声纹（speaker verification）—— 注册主人声纹后，ASR / wake 可拒陌生人触发。
    /// home 上 sv_server.py 跑 CAM++ 中文 SV 模型；用户用 mac sox 录一段 wav
    /// 上传到 home，玄女跑 `voiceprint enroll` 提 embedding 存 ~/.fuxi/voiceprint/。
    /// fail-open：未注册时全放行（开机即用，注册后才严格）。
    #[command(subcommand)]
    Voiceprint(VoiceprintCmd),
}

#[derive(Debug, Subcommand)]
enum VoiceprintCmd {
    /// 注册主人声纹（覆盖式 —— 重新跑会覆盖旧 embedding，便于重录更准音色）。
    /// 例：`fuxi xuannv voiceprint enroll --wav /tmp/yilin.wav`
    Enroll(xuannv_cmd::VoiceprintEnrollArgs),
    /// 测试某段 wav 是否匹配主人（不存盘，纯比对）。
    /// 例：`fuxi xuannv voiceprint verify --wav /tmp/test.wav`
    Verify(xuannv_cmd::VoiceprintVerifyArgs),
    /// 看 sv_server 状态：模型是否 loaded、是否已 enrolled、threshold。
    Status(xuannv_cmd::VoiceprintStatusArgs),
}

#[derive(Debug, Subcommand)]
enum HotwordCmd {
    /// 加/更新一条规则（同 match 视为更新）。
    /// 例：`fuxi xuannv hotword add --match '克劳德[寇口扣][德的]?' --replace 'claude code'`
    /// 或纯字面：`fuxi xuannv hotword add --literal --match '麦克' --replace 'mac'`（小心歧义）
    Add(xuannv_cmd::HotwordAddArgs),
    /// 列所有当前规则 + 编号（rm 用）。
    List,
    /// 删第 N 条（先 list 看编号）。
    Rm(xuannv_cmd::HotwordRmArgs),
}

#[derive(Debug, Subcommand)]
enum HandoffCmd {
    /// 写 handoff markdown：`fuxi xuannv handoff write '<≤500字 markdown 文本>'`
    /// 或者 `fuxi xuannv handoff write -` 从 stdin 读。
    Write(xuannv_cmd::HandoffWriteArgs),
    /// 打印当前落档的 handoff 文件内容（用于 debug + 调试新副本接班是否拿到内容）。
    Read,
}

#[derive(Debug, Subcommand)]
enum ImCmd {
    /// 启动 IM axum 服务（默认 :9100），含完整伏羲 platform。
    Start(subcommands::ImStartArgs),
    /// 设置 PWA 登入主密码（交互式 + bcrypt + ~/.fuxi/im_password.bcrypt）。
    #[command(name = "set-password")]
    SetPassword(subcommands::ImSetPasswordArgs),
    /// 用本机 HMAC key 签一个 ad-hoc token——给 smoke / curl 健康检查用。
    #[command(name = "issue-token")]
    IssueToken(subcommands::ImIssueTokenArgs),
}

#[derive(Debug, Subcommand)]
enum TaskCmd {
    /// 用户授权通过后解锁任务。
    Unblock(subcommands::TaskUnblockArgs),
}

#[derive(Debug, Subcommand)]
enum CronCmd {
    /// 登记 cron trigger。
    Add(subcommands::CronAddArgs),
    /// 登记一次性 trigger。
    Once(subcommands::CronOnceArgs),
    /// 登记 fs_watch trigger。
    Watch(subcommands::CronWatchArgs),
    /// 登记 webhook trigger。
    Webhook(subcommands::CronWebhookArgs),
    /// 列所有 triggers。
    List(subcommands::CronListArgs),
    /// 手动 fire。
    Fire(subcommands::CronFireArgs),
    /// 删 trigger。
    Remove(subcommands::CronRemoveArgs),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // TUI 模式必须**在 init_tracing 之前**把 stderr 重定向到文件——否则
    // fuxi 启动期（hub/daemon/spawn 玄女）的 tracing 会污染用户 shell，
    // 退 TUI 后 alt screen 撤掉时一把全冒出来。踩过，见 docs/session-review-2026-04-20.md。
    if is_tui_mode(&cli.cmd)
        && let Err(e) = redirect_stderr_to_log("/tmp/fuxi.log")
    {
        eprintln!("⚠ 无法把 stderr 重定向到 /tmp/fuxi.log: {e}（TUI 下日志可能污染画面）");
    }

    init_tracing();
    // 进程启动期把主题初始化为 FUXI_THEME 指定值——后续 REPL draw 全部走
    // theme::current()，/theme 命令可运行时热切。
    theme::init_from_env();
    match cli.cmd {
        None => repl::run(Default::default()).await,
        Some(Command::Demo(args)) => demo::run(args).await,
        Some(Command::Up(args)) => up::run(args).await,
        Some(Command::Watch(args)) => watch::run(args).await,
        Some(Command::Spawn(args)) => subcommands::run_spawn(args).await,
        Some(Command::Dispatch(args)) => subcommands::run_dispatch(args).await,
        Some(Command::Intervene(args)) => subcommands::run_intervene(args).await,
        Some(Command::Status(args)) => subcommands::run_status(args).await,
        Some(Command::List(args)) => subcommands::run_list(args).await,
        Some(Command::Nodes(args)) => subcommands::run_nodes(args).await,
        Some(Command::Kill(args)) => subcommands::run_kill(args).await,
        Some(Command::Events(args)) => subcommands::run_events(args).await,
        Some(Command::Block(args)) => subcommands::run_block(args).await,
        Some(Command::Task(t)) => match t {
            TaskCmd::Unblock(args) => subcommands::run_task_unblock(args).await,
        },
        Some(Command::Resume(args)) => subcommands::run_resume(args).await,
        Some(Command::Skill(args)) => skill::run(args).await,
        Some(Command::Dist(cmd)) => match cmd {
            dist::DistCmd::Enqueue(args) => dist::run_enqueue(args).await,
            dist::DistCmd::Worker(args) => dist::run_worker(args).await,
        },
        Some(Command::Memory(args)) => memory_cmd::run(args).await,
        Some(Command::Banner) => {
            banner::print_to_stdout(&theme::from_env());
            Ok(())
        }
        Some(Command::Cron(c)) => match c {
            CronCmd::Add(args) => subcommands::run_cron_add(args).await,
            CronCmd::Once(args) => subcommands::run_cron_once(args).await,
            CronCmd::Watch(args) => subcommands::run_cron_watch(args).await,
            CronCmd::Webhook(args) => subcommands::run_cron_webhook(args).await,
            CronCmd::List(args) => subcommands::run_cron_list(args).await,
            CronCmd::Fire(args) => subcommands::run_cron_fire(args).await,
            CronCmd::Remove(args) => subcommands::run_cron_remove(args).await,
        },
        Some(Command::Im(i)) => match i {
            ImCmd::Start(args) => subcommands::run_im_start(args).await,
            ImCmd::SetPassword(args) => subcommands::run_im_set_password(args).await,
            ImCmd::IssueToken(args) => subcommands::run_im_issue_token(args).await,
        },
        Some(Command::Project(p)) => match p {
            ProjectCmd::Add(args) => project_cmd::run_add(args).await,
            ProjectCmd::List(args) => project_cmd::run_list(args).await,
            ProjectCmd::Join(args) => project_cmd::run_join(args).await,
            ProjectCmd::Info(args) => project_cmd::run_info(args).await,
            ProjectCmd::Rm(args) => project_cmd::run_remove(args).await,
        },
        Some(Command::Deliverable(d)) => match d {
            DeliverableCmd::Produce(args) => project_cmd::run_deliverable_produce(args).await,
        },
        Some(Command::Note(args)) => note_cmd::run_note(args).await,
        Some(Command::Profile(args)) => profile_cmd::run(args).await,
        Some(Command::Insight(args)) => insight_cmd::run(args).await,
        Some(Command::Sandbox(s)) => match s {
            SandboxCmd::List(args) => project_cmd::run_sandbox_list(args).await,
            SandboxCmd::Retire(args) => project_cmd::run_sandbox_retire(args).await,
            SandboxCmd::Sweep(args) => project_cmd::run_sandbox_sweep(args).await,
        },
        Some(Command::Xuannv(x)) => match x {
            XuannvCmd::Refresh => xuannv_cmd::run_refresh().await,
            XuannvCmd::Handoff(h) => match h {
                HandoffCmd::Write(args) => xuannv_cmd::run_handoff_write(args).await,
                HandoffCmd::Read => xuannv_cmd::run_handoff_read().await,
            },
            XuannvCmd::Say(args) => xuannv_cmd::run_say(args).await,
            XuannvCmd::Look(args) => xuannv_cmd::run_look(args).await,
            XuannvCmd::Hotword(h) => match h {
                HotwordCmd::Add(args) => xuannv_cmd::run_hotword_add(args).await,
                HotwordCmd::List => xuannv_cmd::run_hotword_list().await,
                HotwordCmd::Rm(args) => xuannv_cmd::run_hotword_rm(args).await,
            },
            XuannvCmd::Voiceprint(v) => match v {
                VoiceprintCmd::Enroll(args) => xuannv_cmd::run_voiceprint_enroll(args).await,
                VoiceprintCmd::Verify(args) => xuannv_cmd::run_voiceprint_verify(args).await,
                VoiceprintCmd::Status(args) => xuannv_cmd::run_voiceprint_status(args).await,
            },
        },
        Some(Command::Bug(args)) => bug_cmd::run(args).await,
        Some(Command::Issue(args)) => issue_cmd::run(args).await,
        Some(Command::Topic(args)) => topic_cmd::run(args).await,
    }
}

/// 默认把日志写到 stderr，留 stdout 给 demo 的事件流输出。
/// `RUST_LOG` 可覆盖；未设时缺省 `info,fuxi=debug`。
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,fuxi=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

/// 是否是进 alt-screen TUI 的子命令。
fn is_tui_mode(cmd: &Option<Command>) -> bool {
    match cmd {
        None => true, // 无参 → repl
        Some(Command::Watch(_)) => true,
        Some(Command::Demo(a)) if a.tui => true,
        _ => false,
    }
}

/// 把进程的 stderr（fd 2）重定向到日志文件。Unix `dup2(2)`.
///
/// **时机**：**必须** init_tracing 之前调——tracing subscriber 用 `std::io::stderr()`
/// 底层写 fd 2。dup2 之后 fd 2 指向文件，后续所有 stderr 写入都进文件。
#[cfg(unix)]
fn redirect_stderr_to_log(path: &str) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::options()
        .create(true)
        .append(true)
        .open(path)?;
    // SAFETY: dup2 对 valid fd 是安全调用；file 在作用域内有效。
    let ret = unsafe { libc_dup2(file.as_raw_fd(), 2) };
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
    #[link_name = "dup2"]
    fn libc_dup2(oldfd: i32, newfd: i32) -> i32;
}

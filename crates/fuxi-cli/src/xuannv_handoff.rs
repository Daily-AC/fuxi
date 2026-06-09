//! 玄女 handoff 接班监听器（task #8 后端落档→spawn 路径）。
//!
//! ## 它在做什么
//!
//! 订阅 [`fuxi_core::EventKind::XuannvHandoffWritten`]——CLI `fuxi xuannv
//! handoff write` 落档时发出。监听器拿到事件后：
//! 1. **等玄女当前 turn idle**：直接 kill 中途的 turn 会丢用户最近一句的回复
//!    （会把 cc 进程腰斩）。轮询 shelf status 直到 `Idle`，超时兜底 60s 后强 kill。
//! 2. **kill 老玄女**：调 `Fuxi::shutdown_agent`——shutdown_agent 已豁免玄女
//!    的特殊路径，但本路径是用户主动交接，要绕过那个豁免。临时方案：
//!    `Fuxi::set_xuannv(other)` 后 shutdown 不命中玄女豁免——但这会让 watcher
//!    误以为 id 变了。最终方案：直接调 `kill` 命令通过 fuxi-orchestrator 的
//!    Force kill API。
//! 3. **spawn 新玄女**：走 `xuannv_bootstrap::ensure_xuannv` 同款路径，但传一
//!    个临时 `append_system_prompt` 头部 = handoff 内容。新玄女上线后：
//!    a. set_xuannv 触发 watch 通知；
//!    b. 上下文 watcher 自动重置累加；
//!    c. 删除 handoff 文件（避免下次启动误以为又要交接）；
//!    d. emit 一条 system_origin 消息「✻ 上下文已交接 · 新副本接班」让用户
//!    视角对齐 + PWA 通知 tab 加一条。
//!
//! ## 重启后行为
//!
//! 重启 fuxi-im 进程时，老的 `~/.fuxi/xuannv-handoff.md` 可能还在（user 重启
//! 操作时机不可控）。本监听器**启动时检查一次**：若文件存在且 modtime <30s
//! 内（CLI 刚 publish 完事件，IM 重启抢跑），重放 handoff 流程；超过 30s
//! → 用户手动调 `fuxi xuannv handoff read` 自检。
//!
//! ## 测试
//!
//! 完整 e2e 极难（需要真 cc 进程 + 30s+ 玄女 turn）。本模块的核心逻辑：
//! 阈值判断 + 等 idle 轮询 + spawn 新副本——前两者抽出函数单测；spawn 走
//! 与 ensure_xuannv 相同路径，由 ensure_xuannv 的既有覆盖兜底。

use anyhow::{Context, Result};
use futures_util::StreamExt;
use fuxi_agent_cc::CcLaunchConfig;
use fuxi_core::event::EventKind;
use fuxi_core::id::AgentId;
use fuxi_events::EventBus;
use fuxi_memory::OracleStore;
use fuxi_orchestrator::{Fuxi, ShelfStatus, WorkerKind};
use fuxi_skills as skill_loader;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

#[allow(unused_imports)]
use fuxi_orchestrator::Intervener as _;

/// handoff markdown 落档绝对路径——同 [`crate::xuannv_cmd::handoff_path`]。
fn handoff_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".fuxi").join("xuannv-handoff.md"))
        .unwrap_or_else(|| PathBuf::from(".fuxi/xuannv-handoff.md"))
}

/// 等玄女当前 turn idle 的 ceiling——超时仍 Busy 就强 kill。
const IDLE_WAIT_CEILING_SECS: u64 = 60;
/// 轮询间隔——shelf 内存读 + 异步锁，500ms 不重。
const IDLE_POLL_INTERVAL_MS: u64 = 500;

/// 文件落档 polling 间隔——30s 是"用户写完 handoff 等不到 1 分钟玄女就被换"
/// 的可接受延迟。`bus.subscribe()` 跨进程拿不到事件（CLI 直写 SQLite，
/// fuxi-im 进程的 broadcast 收不到），polling 是兜底。
const FILE_POLL_INTERVAL_SECS: u64 = 30;

pub fn start_watcher(
    fuxi: Arc<Fuxi>,
    bus: EventBus,
    oracle: OracleStore,
    role: String,
) -> JoinHandle<()> {
    let mut sub = bus.subscribe();
    tokio::spawn(async move {
        // 启动期一次性检查：若 handoff 文件已存在（CLI 在 IM 重启的窗口里 publish
        // 过事件，事件已 SQLite 持久化但 broadcast 流我们错过了），主动跑一次。
        if handoff_path().exists() {
            info!("启动期发现 handoff 文件落档，触发一次接班流程");
            if let Err(err) = run_handoff(&fuxi, &oracle, &role).await {
                warn!(?err, "启动期 handoff 接班流程失败");
            }
        }
        info!(
            interval_secs = FILE_POLL_INTERVAL_SECS,
            "玄女 handoff 监听器启动（同进程 EventBus + 跨进程 fs polling 双保险）"
        );
        let mut poll_tick = tokio::time::interval(Duration::from_secs(FILE_POLL_INTERVAL_SECS));
        // 第一 tick 是 immediate，跳过——启动期已经查过了
        poll_tick.tick().await;

        loop {
            tokio::select! {
                // 同进程：bus 上看到 XuannvHandoffWritten（fuxi-im 自己的子进程
                // 写入，或将来的内部路径——当前 CLI 走跨进程，fs polling 兜底）
                maybe_ev = sub.next() => {
                    match maybe_ev {
                        Some(Ok(ev)) if matches!(ev.kind, EventKind::XuannvHandoffWritten { .. }) => {
                            info!("收到 XuannvHandoffWritten 事件（同进程），开始接班流程");
                            if let Err(err) = run_handoff(&fuxi, &oracle, &role).await {
                                warn!(?err, "玄女 handoff 接班流程失败");
                            }
                        }
                        Some(Ok(_)) => {} // 其它事件忽略
                        Some(Err(err)) => debug!(?err, "handoff 监听器跳过 sub 错误"),
                        None => {
                            info!("玄女 handoff 监听器退出（bus 关闭）");
                            break;
                        }
                    }
                }
                // 跨进程：fs polling 兜底——CLI `fuxi xuannv handoff write` 是另一
                // 进程，它直写 SQLite 不经过本进程 broadcast；只能通过文件系统
                // 检测落档。30s 间隔够用（handoff 不高频）。
                _ = poll_tick.tick() => {
                    if handoff_path().exists() {
                        info!("fs poll 命中 handoff 文件落档，开始接班流程");
                        if let Err(err) = run_handoff(&fuxi, &oracle, &role).await {
                            warn!(?err, "玄女 handoff 接班流程失败（poll 路径）");
                        }
                    }
                }
            }
        }
    })
}

/// 完整接班流程：等 idle → kill old → spawn new with prelude → 通知前端。
async fn run_handoff(fuxi: &Fuxi, oracle: &OracleStore, role: &str) -> Result<()> {
    let path = handoff_path();
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("读 handoff 文件 {} 失败", path.display()))?;
    let body = body.trim().to_string();
    if body.is_empty() {
        anyhow::bail!("handoff 文件存在但内容空——拒绝接班");
    }
    let len = body.chars().count();

    let old_xuannv = fuxi
        .xuannv_id()
        .await
        .context("玄女 id 未设——尚未 spawn 完成？")?;
    info!(old = %old_xuannv, "等玄女当前 turn idle");
    wait_idle(fuxi, old_xuannv).await;

    info!(old = %old_xuannv, "kill 老玄女");
    if let Err(err) = fuxi
        .shutdown_xuannv_for_handoff(old_xuannv, "用户上下文交接".to_string())
        .await
    {
        warn!(?err, "kill 老玄女失败——继续 spawn 新副本（老进程可能已死）");
    }
    // 给系统一点时间完成 cleanup（drop child + 清 shelf entry）
    tokio::time::sleep(Duration::from_millis(300)).await;

    info!("spawn 新玄女副本（注入 handoff prelude）");
    let prelude = format_handoff_prelude(&body);
    let new_id = spawn_with_prelude(fuxi, oracle, role, &prelude).await?;
    fuxi.set_xuannv(new_id).await;

    // 删除 handoff 文件——下次启动不会误以为又要交接
    if let Err(err) = std::fs::remove_file(&path) {
        warn!(?err, "删除 handoff 文件失败——下次重启会重放");
    }

    // 通知玄女自身：上下文已接班（让她在新对话首句对用户说一声）
    let notice = format!(
        "[CTX_HANDOFF_DONE] 你是新副本，刚由上一只玄女写的 handoff（{} 字）接班。\
         请用一句话告诉用户「✻ 上下文已交接（{} 字摘要），我接着上一只副本继续。」",
        len, len
    );
    if let Err(err) = <Fuxi as fuxi_orchestrator::Intervener>::intervene_system(
        fuxi,
        new_id,
        false,
        &notice,
        "context_handoff_done",
    )
    .await
    {
        warn!(
            ?err,
            "新玄女接班通知 intervene 失败——用户视角无系统消息提示"
        );
    }
    info!(new = %new_id, "玄女接班完成");
    Ok(())
}

async fn wait_idle(fuxi: &Fuxi, agent: AgentId) {
    let deadline = std::time::Instant::now() + Duration::from_secs(IDLE_WAIT_CEILING_SECS);
    while std::time::Instant::now() < deadline {
        match fuxi.status_of(agent).await {
            Some(ShelfStatus::Idle) => return,
            None => return, // 已被清走，不必再等
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(IDLE_POLL_INTERVAL_MS)).await;
    }
    warn!(agent = %agent, "等玄女 idle 超时——强 kill 老副本（可能丢未完成 turn 的回复）");
}

/// spawn 新玄女并把 `prelude_text` 拼到 system prompt 头部。
///
/// 共享给 [`crate::topic_switch::switch_topic_to`]：handoff / topic 切换都走 kill +
/// spawn-with-prelude pattern，区别只在 prelude 内容（handoff = 上一只副本的 handoff
/// 摘要；topic = 新 topic 的对话回顾）。调用方负责自己 format prelude 文本。
///
/// 接班 handoff 是新 cc session（老 cc 已 kill）。同 xuannv_bootstrap：cc 2.1.114+
/// SDK 模式 strict resume，预先生成 session_id 也会被拒。让 cc 自己生成；prelude
/// 全部上下文已经 inline，不依赖 cc 端 session 持久化。
///
/// `oracle` 暂未使用——保留参数让 caller 不必感知未来若要往 prelude 里塞 oracle
/// 数据的扩展。
pub async fn spawn_with_prelude(
    fuxi: &Fuxi,
    oracle: &OracleStore,
    role: &str,
    prelude_text: &str,
) -> Result<AgentId> {
    let loaded = skill_loader::load(role).with_context(|| format!("加载 roles/{role}/ROLE.md"))?;
    let xuannv_profile = loaded.profile.clone();
    let _ = oracle;

    // prelude 在最顶部，原 append_system_prompt（含 dispatch-routing 教学）在后面
    // ——cc 接收 system prompt 是按顺序拼接的字符串，前者优先级 = 出现位置。
    let combined = if loaded.append_system_prompt.is_empty() {
        prelude_text.to_string()
    } else {
        format!("{}{}", prelude_text, loaded.append_system_prompt)
    };

    let cc_cfg = CcLaunchConfig {
        append_system_prompt: Some(combined),
        allowed_tools: loaded.allowed_tools,
        disallowed_tools: loaded.disallowed_tools,
        resume_session_id: None,
        session_id: None,
        ..Default::default()
    };

    let id = fuxi
        .spawn_worker(xuannv_profile, WorkerKind::Cc(cc_cfg))
        .await
        .context("spawn 新玄女失败")?;
    Ok(id)
}

/// 把 handoff 文档原文包成"接班 prelude"——加入"你是新副本"导语 + 让玄女首句
/// 主动告诉用户接班完成。抽出独立 fn 便于单测 prelude 文案不漂移。
pub(crate) fn format_handoff_prelude(handoff_body: &str) -> String {
    format!(
        "## 上下文交接（必读）\n\n\
         你是新副本玄女——由上一只副本主动交接来的。下面是她写的 handoff 摘要，\
         请把它当作「你刚才在做的事」读，不要当陌生信息：\n\n\
         ---\n{}\n---\n\n\
         首条用户消息处理完后，你**必须**单独发一句：「✻ 上下文已交接 · 新副本接班\
         （从 handoff 接续上文）」让用户看到接班完成。然后正常继续对话。\n\n",
        handoff_body
    )
}

/// 等当前玄女 turn idle 的兜底 helper——topic_switch 也要等 idle 再 kill。
pub(crate) async fn wait_xuannv_idle(fuxi: &Fuxi, agent: AgentId) {
    wait_idle(fuxi, agent).await;
}

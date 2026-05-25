//! `CangjieSpawner` 主线实装——把 `Arc<Fuxi>` 接到 `InsightExtractorTask`。
//!
//! 跟 `extractor_hook::FuxiExtractorSpawner` 同型——fuxi-orchestrator 不能依赖
//! fuxi-cli（cli 顶层），所以 trait 定在 orchestrator，adapter 放在这里。
//!
//! 调用方：`fuxi-cli/im.rs` 启动期 `load_cangjie_launch()` 加载 role + 配 spawner，
//! 再 `InsightExtractorTask::new(bus, hetu, spawner, cfg).spawn()` 起后台 loop。

use anyhow::Context;
use async_trait::async_trait;
use futures_util::StreamExt;
use fuxi_agent_cc::CcLaunchConfig;
use fuxi_core::agent::AgentProfile;
use fuxi_core::event::EventKind;
use fuxi_core::id::AgentId;
use fuxi_core::task::{Task, TaskState};
use fuxi_events::EventBus;
use fuxi_orchestrator::{
    CangjieSpawner, CangjieSpawnerError, CangjieSpawnerResult, Fuxi, WorkerKind,
};
use std::sync::Arc;
use std::time::Duration;

/// 把 `Arc<Fuxi>` 包成 `CangjieSpawner` 的 adapter。
///
/// 持有仓颉 role 的 profile + cc 启动配置（启动期一次加载）；每次 `spawn_and_run`
/// 复用它们派一个新仓颉门客（task-bound）跑 prompt，等 Done。
pub struct FuxiCangjieSpawner {
    fuxi: Arc<Fuxi>,
    bus: EventBus,
    profile: AgentProfile,
    cc_cfg: CcLaunchConfig,
}

impl FuxiCangjieSpawner {
    pub fn new(
        fuxi: Arc<Fuxi>,
        bus: EventBus,
        profile: AgentProfile,
        cc_cfg: CcLaunchConfig,
    ) -> Self {
        Self {
            fuxi,
            bus,
            profile,
            cc_cfg,
        }
    }
}

#[async_trait]
impl CangjieSpawner for FuxiCangjieSpawner {
    async fn spawn_and_run(
        &self,
        prompt: String,
        timeout: Duration,
    ) -> CangjieSpawnerResult<String> {
        // 1. 订阅 bus——必须在 dispatch 之前，否则 broadcast 漏发。
        let mut sub = self.bus.subscribe();

        // 2. 派活：先 claim 一只 idle cangjie 复用；找不到再 spawn 新的。
        //    issue eebe38ef：cangjie 是单 turn 无状态抽取，每次都 spawn 新的
        //    导致 shelf 累积（idle GC 600s 才回收，spawn 速度 >> GC 速度）。
        //    复用 idle 让稳态门客数趋近 1，与 cangjie 实际并发上限对齐。
        let task = Task::new("cangjie-extract", &prompt);
        let task_id = task.id;
        let agent_id = if let Some(idle) = self.fuxi.claim_idle_by_role(&self.profile.role).await {
            self.fuxi
                .dispatch_in_task(idle, task_id, "cangjie-extract", &prompt)
                .await
                .map_err(|e| Box::new(e) as CangjieSpawnerError)?;
            idle
        } else {
            self.fuxi
                .dispatch_to_any_in_task(
                    &self.profile.role,
                    task_id,
                    "cangjie-extract",
                    &prompt,
                    self.profile.clone(),
                    WorkerKind::Cc(self.cc_cfg.clone()),
                )
                .await
                .map_err(|e| Box::new(e) as CangjieSpawnerError)?
        };

        // 3. 带 timeout 等 Done。期间累积 AgentResponded 文本——仓颉 prompt 要求
        //    单 turn 直接出 JSON，但保险起见拼多段。
        let wait = tokio::time::timeout(timeout, async {
            let mut accumulated = String::new();
            while let Some(Ok(ev)) = sub.next().await {
                if ev.meta.agent != Some(agent_id) || ev.meta.task != Some(task_id) {
                    continue;
                }
                match ev.kind {
                    EventKind::AgentResponded { text, .. } => {
                        if !accumulated.is_empty() {
                            accumulated.push('\n');
                        }
                        accumulated.push_str(&text);
                    }
                    EventKind::TaskStateChanged {
                        to: TaskState::Done,
                        ..
                    } => {
                        return Ok::<String, CangjieSpawnerError>(accumulated);
                    }
                    EventKind::TaskStateChanged {
                        to: TaskState::Cancelled,
                        ..
                    }
                    | EventKind::TaskBlocked { .. }
                    | EventKind::AgentDead { .. } => {
                        return Err::<String, CangjieSpawnerError>(
                            format!("cangjie task failed before Done: {:?}", ev.kind).into(),
                        );
                    }
                    _ => {}
                }
            }
            Err::<String, CangjieSpawnerError>("cangjie bus stream closed before Done".into())
        })
        .await;

        match wait {
            Ok(Ok(text)) => Ok(text),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(format!("cangjie timed out after {timeout:?}").into()),
        }
    }

    async fn role_of(&self, agent: AgentId) -> Option<String> {
        self.fuxi
            .list_workers()
            .await
            .into_iter()
            .find(|c| c.id == agent)
            .map(|c| c.profile.role.clone())
    }
}

/// 从 `roles/cangjie/ROLE.md` 加载 profile + 构造 cc 启动配置。
///
/// 仓颉 ROLE.md frontmatter `allowed-tools: Read`——加载后 `CcLaunchConfig` 拿到
/// 极简 tool whitelist。失败让 caller 决定是否关停 insight extractor 能力。
pub fn load_cangjie_launch() -> anyhow::Result<(AgentProfile, CcLaunchConfig)> {
    let loaded = fuxi_skills::load("cangjie").context("加载 roles/cangjie/ROLE.md")?;
    let cc_cfg = CcLaunchConfig {
        append_system_prompt: if loaded.append_system_prompt.is_empty() {
            None
        } else {
            Some(loaded.append_system_prompt)
        },
        allowed_tools: loaded.allowed_tools,
        ..Default::default()
    };
    Ok((loaded.profile, cc_cfg))
}

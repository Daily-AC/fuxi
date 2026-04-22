//! `FactExtractorSpawner` 主线实装——把 fuxi-memory 的 trait 接到 `Fuxi`。
//!
//! 为什么放在 fuxi-cli 而不是 fuxi-orchestrator：
//! - `FactExtractorSpawner` 定义在 fuxi-memory。让 fuxi-orchestrator 依赖
//!   fuxi-memory 只为实现这个 trait 风险大——memory 未来可能想看事件，一旦
//!   想反向访问 orchestrator 就会循环依赖。
//! - fuxi-cli 顶层已经依赖所有 crate，在这里包 adapter 最自然。

use anyhow::Context;
use async_trait::async_trait;
use futures_util::StreamExt;
use fuxi_agent_cc::CcLaunchConfig;
use fuxi_core::agent::AgentProfile;
use fuxi_core::event::EventKind;
use fuxi_core::id::AgentId;
use fuxi_core::task::{Task, TaskState};
use fuxi_events::EventBus;
use fuxi_memory::{FactExtractorSpawner, SpawnerError, SpawnerResult};
use fuxi_orchestrator::{Fuxi, WorkerKind};
use std::sync::Arc;
use std::time::Duration;

/// 把 `Arc<Fuxi>` 包成 `FactExtractorSpawner` 的 adapter。
///
/// 持有 extractor role 的 profile + cc 启动配置（启动时一次加载）；每次
/// `spawn_and_run` 复用它们做 spawn。这里走严格 task-bound 派工路径，
/// 每次抽取都挂在一个明确 task_id 下，避免隐式 idle 复用语义。
pub struct FuxiExtractorSpawner {
    fuxi: Arc<Fuxi>,
    bus: EventBus,
    profile: AgentProfile,
    cc_cfg: CcLaunchConfig,
}

impl FuxiExtractorSpawner {
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
impl FactExtractorSpawner for FuxiExtractorSpawner {
    async fn spawn_and_run(&self, prompt: String, timeout: Duration) -> SpawnerResult<String> {
        // 1. 订阅 bus——必须在 dispatch_to_any 之前，否则 broadcast 漏发。
        let mut sub = self.bus.subscribe();

        // 2. 派活：走 dispatch_to_any_in_task（严格 task-bound）：
        //    - task_id 是该次抽取任务的稳定锚点
        //    - role 选人策略不复用 idle（由 orchestrator 明确 spawn 新实例）
        //    这样行为和 Decision 10 的“门客归任务”方向一致。
        let task = Task::new("extract", &prompt);
        let task_id = task.id;
        let agent_id = self
            .fuxi
            .dispatch_to_any_in_task(
                &self.profile.role,
                task_id,
                "extract",
                &prompt,
                self.profile.clone(),
                WorkerKind::Cc(self.cc_cfg.clone()),
            )
            .await
            .map_err(|e| Box::new(e) as SpawnerError)?;

        // 3. 带 timeout 等 Done
        let wait = tokio::time::timeout(timeout, async {
            let mut accumulated = String::new();
            while let Some(Ok(ev)) = sub.next().await {
                if ev.meta.agent != Some(agent_id) || ev.meta.task != Some(task_id) {
                    continue;
                }
                match ev.kind {
                    EventKind::AgentResponded { text } => {
                        if !accumulated.is_empty() {
                            accumulated.push('\n');
                        }
                        accumulated.push_str(&text);
                    }
                    EventKind::TaskStateChanged {
                        to: TaskState::Done,
                        ..
                    } => {
                        return Ok::<String, SpawnerError>(accumulated);
                    }
                    EventKind::TaskStateChanged {
                        to: TaskState::Cancelled,
                        ..
                    }
                    | EventKind::TaskBlocked { .. }
                    | EventKind::AgentDead { .. } => {
                        return Err::<String, SpawnerError>(
                            format!("extractor task failed before Done: {:?}", ev.kind).into(),
                        );
                    }
                    _ => {}
                }
            }
            Err::<String, SpawnerError>("extractor bus stream closed before Done".into())
        })
        .await;

        match wait {
            Ok(Ok(text)) => Ok(text),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(format!("extractor timed out after {:?}", timeout).into()),
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

/// 从 `roles/extractor/ROLE.md` 加载 profile + 构造 `CcLaunchConfig`（旧 `skills/.../SKILL.md` 仍兼容）。
///
/// 如果 skill 加载失败（未安装、frontmatter 损坏），返 Err——caller
/// 决定是否关闭 extractor 能力（比如 warn 后跳过）。
pub fn load_extractor_launch() -> anyhow::Result<(AgentProfile, CcLaunchConfig)> {
    let loaded = fuxi_skills::load("extractor").context("加载 roles/extractor/ROLE.md")?;
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

/// 从 env 读 ExtractorConfig。
///
/// 2026-04-21 翻转默认：默认 disabled——`FUXI_EXTRACTOR_ENABLED=1` 才开启自动抽取。
/// 玄女按 prompt 判断时机用 `fuxi memory record` 手工入策府是常态路径。
pub fn extractor_cfg_from_env() -> fuxi_memory::ExtractorConfig {
    let mut cfg = fuxi_memory::ExtractorConfig::default();
    if matches!(
        std::env::var("FUXI_EXTRACTOR_ENABLED").as_deref(),
        Ok("1") | Ok("true")
    ) {
        cfg.enabled = true;
    }
    cfg
}

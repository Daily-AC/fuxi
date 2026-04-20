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
/// `spawn_and_run` 复用它们做 spawn。spawn_worker 自带 idle 去重（M2.4）
/// 所以反复抽取不会堆 cc 子进程。
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
        // 1. spawn extractor 门客（会命中 M2.4 去重复用已有 idle）
        let agent_id = self
            .fuxi
            .spawn_worker(self.profile.clone(), WorkerKind::Cc(self.cc_cfg.clone()))
            .await
            .map_err(|e| Box::new(e) as SpawnerError)?;

        // 2. 订阅 bus 等该 agent+task 的 AgentResponded 累积文本
        //    订阅必须在 dispatch 之前——否则 broadcast 漏发给未订阅者
        let mut sub = self.bus.subscribe();

        // 3. dispatch 抽取任务
        let task = Task::new("extract", &prompt);
        let task_id = task.id;
        self.fuxi
            .dispatch(agent_id, task)
            .await
            .map_err(|e| Box::new(e) as SpawnerError)?;

        // 4. 带 timeout 等 Done
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

/// 从 env 读 ExtractorConfig。`FUXI_EXTRACTOR_ENABLED=0` 关；其他参数暂用默认。
pub fn extractor_cfg_from_env() -> fuxi_memory::ExtractorConfig {
    let mut cfg = fuxi_memory::ExtractorConfig::default();
    if matches!(
        std::env::var("FUXI_EXTRACTOR_ENABLED").as_deref(),
        Ok("0") | Ok("false")
    ) {
        cfg.enabled = false;
    }
    cfg
}

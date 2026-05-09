//! TDD：玄女 `Stopping` 堵塞自愈三连——根因见 `agent.rs::cancel`。
//!
//! 路径痛点：cancel 把 `Inner.status = Stopping`，但唯一摊回 `Idle` 的 turn
//! terminal（`ResultSuccess`/`ResultError`）在 cc 处于 thinking/tool-loop 中段
//! 被打断时**不一定到达**——导致 status 永远 `Stopping`，pending 队列只长
//! 不 drain，玄女假死 44 分钟。
//!
//! 修法两条腿：
//! 1. 收 `control_response { request_id == last_interrupt_request_id,
//!    response.subtype == "success" }` 立即走和 turn terminal 同款 drain
//! 2. 30s watchdog 兜底：cancel 后还卡 `Stopping` 强制摊回 + 发
//!    `AgentInterrupted{reason:"stopping_timeout"}` 让玄女知情
//!
//! 测试用真 `WsChannel` + tokio_tungstenite 假 CLI 反连——既覆盖 pump_loop
//! 真路径，又不用 spawn `claude` 二进制。

use std::time::Duration;

use futures_util::sink::SinkExt;
use futures_util::stream::StreamExt;
use fuxi_agent_cc::CcAgent;
use fuxi_core::Agent;
use fuxi_core::agent::{AgentProfile, AgentStatus};
use fuxi_core::event::{Event, EventKind};
use fuxi_core::id::TaskId;
use fuxi_core::task::Task;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as TkMsg;

/// 起一个用真 WsChannel + 假 CLI 的 CcAgent 测试 fixture。
/// 返回：(agent, fake-cli WS 写流, fake-cli WS 读流, dispatch 出来的事件 rx)
async fn spawn_agent_with_fake_cli() -> (
    CcAgent,
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        TkMsg,
    >,
    futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    mpsc::Receiver<Event>,
) {
    let profile = AgentProfile {
        name: "xuannv-test".into(),
        role: "xuannv".into(),
        cli: "claude-code".into(),
        system_prompt: String::new(),
        tags: vec![],
        extra: Default::default(),
    };
    let (agent, url) = CcAgent::for_test_with_fake_cli(profile)
        .await
        .expect("test fixture build");

    // 假 CLI 反连
    let (ws, _resp) = connect_async(&url).await.expect("fake cli connect");
    let (writer, mut reader) = ws.split();

    // wait_connect 在 fixture 内部已 await——这里单独再确认一下不是必须，
    // 但等 system/init 进 channel 后 cli_session_id 才填好。先发 init。
    let mut writer = writer;
    let init = json!({
        "type": "system",
        "subtype": "init",
        "session_id": "cli-sid-test",
        "model": "haiku",
        "cwd": "/tmp",
    });
    writer
        .send(TkMsg::Text(serde_json::to_string(&init).unwrap().into()))
        .await
        .expect("send init");
    // 让 channel 把 init 吃进 incoming 并更新 cli_session_id
    tokio::time::sleep(Duration::from_millis(40)).await;

    // dispatch 让 agent 进入 Busy（wire 一条 user 消息出去；fake-cli reader 会拿到）
    let task = Task::new("test-turn", "hello");
    let rx = agent.dispatch(task).await.expect("dispatch");

    // 把 fake-cli 收到的"user"消息抽走，避免后续断言把它当噪声
    let _user_msg = tokio::time::timeout(Duration::from_secs(2), reader.next())
        .await
        .expect("reader recv timeout")
        .expect("some")
        .expect("ws ok");

    (agent, writer, reader, rx)
}

/// TDD #1：cancel 之后喂一条 `control_response { subtype: "success" }`
/// 必须把 status 摊回 `Idle` 并 drain 之前 enqueue 的 pending。
#[tokio::test]
async fn cancel_then_control_response_success_drains_pending() {
    let (agent, mut writer, mut reader, _ev_rx) = spawn_agent_with_fake_cli().await;

    // 此时 agent.status = Busy
    assert_eq!(
        agent.test_status().await,
        AgentStatus::Busy,
        "dispatch 后应是 Busy"
    );

    // busy 状态下 send_message → 入 pending（不直送）
    agent
        .send_message(TaskId::new(), "follow-up-1")
        .await
        .expect("send while busy");
    agent
        .send_message(TaskId::new(), "follow-up-2")
        .await
        .expect("send while busy 2");
    assert_eq!(
        agent.test_pending_len().await,
        2,
        "busy 时两条 follow-up 应都进 pending"
    );

    // cancel 发 control_request{interrupt}
    agent.cancel(TaskId::new()).await.expect("cancel");
    assert_eq!(
        agent.test_status().await,
        AgentStatus::Stopping,
        "cancel 后应是 Stopping"
    );
    let request_id = agent
        .test_last_interrupt_request_id()
        .await
        .expect("cancel 必须记下 last_interrupt_request_id");

    // 把 fake-cli 收到的 control_request 抽掉（它含我们刚发的 request_id）
    let interrupt_frame = tokio::time::timeout(Duration::from_secs(2), reader.next())
        .await
        .expect("reader recv timeout")
        .expect("some")
        .expect("ws ok");
    let interrupt_value: Value = match interrupt_frame {
        TkMsg::Text(s) => serde_json::from_str(s.trim_end_matches('\n')).expect("json"),
        other => panic!("expected text, got {other:?}"),
    };
    assert_eq!(interrupt_value["type"], "control_request");
    assert_eq!(
        interrupt_value["request_id"].as_str(),
        Some(request_id.as_str())
    );
    assert_eq!(interrupt_value["request"]["subtype"], "interrupt");

    // 假 CLI 回一条 control_response { subtype: "success" } —— 模拟 cc 收到
    // 打断后只发 ack 不发 ResultSuccess 的 worst case
    let ack = json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
        }
    });
    writer
        .send(TkMsg::Text(serde_json::to_string(&ack).unwrap().into()))
        .await
        .expect("send ack");

    // 等 pump_loop 处理 ack → 触发 drain
    let mut waited = Duration::ZERO;
    while waited < Duration::from_secs(2) {
        if agent.test_status().await == AgentStatus::Idle && agent.test_pending_len().await == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        waited += Duration::from_millis(20);
    }
    assert_eq!(
        agent.test_status().await,
        AgentStatus::Idle,
        "interrupt-ack 必须把 status 摊回 Idle"
    );
    assert_eq!(
        agent.test_pending_len().await,
        0,
        "interrupt-ack 必须 drain pending"
    );

    // 验 fake-cli 真收到了 drain 出去的两条 follow-up
    let mut got_bodies: Vec<String> = Vec::new();
    while got_bodies.len() < 2 {
        let frame = tokio::time::timeout(Duration::from_secs(2), reader.next())
            .await
            .expect("reader recv timeout for drain")
            .expect("some")
            .expect("ws ok");
        if let TkMsg::Text(s) = frame {
            let v: Value = serde_json::from_str(s.trim_end_matches('\n')).expect("json");
            if v["type"] == "user" {
                let body = v["message"]["content"][0]["text"]
                    .as_str()
                    .expect("text")
                    .to_string();
                got_bodies.push(body);
            }
        }
    }
    assert_eq!(
        got_bodies,
        vec!["follow-up-1".to_string(), "follow-up-2".to_string()],
        "drain 必须按 FIFO 顺序送 follow-up"
    );

    let _ = agent.shutdown().await;
}

/// TDD #2：cancel 后**不**喂任何 ack，30s watchdog 必须强制摊回 + 发
/// `AgentInterrupted{reason:"stopping_timeout"}` 到 active_tx。
#[tokio::test(start_paused = true)]
async fn stopping_watchdog_recovers_after_timeout() {
    let (agent, _writer, _reader, mut ev_rx) = spawn_agent_with_fake_cli().await;

    // dispatch 完毕的事件可能已堆几条 system/init 之类——清掉，避免误判
    while ev_rx.try_recv().is_ok() {}

    agent
        .send_message(TaskId::new(), "stuck-followup")
        .await
        .expect("enqueue");

    agent.cancel(TaskId::new()).await.expect("cancel");
    // paused runtime 下 spawn 出来的 watchdog 必须先被 poll 一次才能记下 sleep
    // deadline；否则后面的 `advance` 推进发生在 sleep "register" 之前，永远不
    // 会触发。yield 几次让 spawn task 第一次进 cpu。
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    assert_eq!(agent.test_status().await, AgentStatus::Stopping);

    // 推时间过 30s——start_paused 让 sleep 不真等
    tokio::time::advance(Duration::from_secs(31)).await;
    // yield 几次让 watchdog task 上 cpu
    for _ in 0..20 {
        tokio::task::yield_now().await;
        if agent.test_status().await == AgentStatus::Idle {
            break;
        }
    }

    assert_eq!(
        agent.test_status().await,
        AgentStatus::Idle,
        "watchdog 必须把 status 摊回 Idle"
    );

    // 验 AgentInterrupted{reason:"stopping_timeout"} 已送到 active_tx
    let mut saw_timeout = false;
    while let Ok(ev) = ev_rx.try_recv() {
        if let EventKind::AgentInterrupted { reason } = &ev.kind
            && reason == "stopping_timeout"
        {
            saw_timeout = true;
            break;
        }
    }
    assert!(
        saw_timeout,
        "watchdog 必须发 AgentInterrupted{{reason:\"stopping_timeout\"}}"
    );

    let _ = agent.shutdown().await;
}

/// TDD #3：连发两次 cancel——旧 watchdog 不能错误触发。
/// 用 generation counter 取消语义：第二次 cancel 把 gen+1，旧 watchdog 醒来发现
/// 自己的 gen 落后，silent return。
#[tokio::test(start_paused = true)]
async fn new_cancel_cancels_old_watchdog() {
    let (agent, _writer, _reader, mut ev_rx) = spawn_agent_with_fake_cli().await;
    while ev_rx.try_recv().is_ok() {}

    // 第一次 cancel
    agent.cancel(TaskId::new()).await.expect("cancel 1");
    let gen1 = agent.test_cancel_generation().await;
    // 让第一只 watchdog 先 poll 进 sleep（理由同 stopping_watchdog_recovers_after_timeout）
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }

    // 立刻第二次 cancel——生成 gen2 应严格 > gen1
    agent.cancel(TaskId::new()).await.expect("cancel 2");
    let gen2 = agent.test_cancel_generation().await;
    assert!(
        gen2 > gen1,
        "第二次 cancel 必须 bump generation：{gen1} → {gen2}"
    );
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }

    // 推 31s，旧 watchdog（gen1）应 silent 自杀，不发事件
    tokio::time::advance(Duration::from_secs(31)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }

    // 状态应是 Idle（最新那条 watchdog gen2 跑完触发摊回——这是合预期的）
    assert_eq!(agent.test_status().await, AgentStatus::Idle);

    // 但 AgentInterrupted{stopping_timeout} 只能有 **一条**——不能因 gen1 旧 watchdog 也发一次而双发
    let mut count = 0;
    while let Ok(ev) = ev_rx.try_recv() {
        if let EventKind::AgentInterrupted { reason } = &ev.kind
            && reason == "stopping_timeout"
        {
            count += 1;
        }
    }
    assert_eq!(
        count, 1,
        "只能有一条 stopping_timeout 事件——旧 watchdog 必须被新 cancel 取消，实测 count={count}"
    );

    let _ = agent.shutdown().await;
}

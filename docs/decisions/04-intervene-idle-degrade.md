# Decision 04 · intervene Idle 门客自动退化 dispatch

**日期**：2026-04-20  
**状态**：已实装（commit `ffd5dfa`）；反玄女诊断的 short-term 方案

## 背景

2026-04-20 用户实测（图 12）：spawn 3 个鲁班门客后用 `fuxi intervene --to <id> hi` 发消息，**门客全都不理**。

玄女自己诊断出根因：

```
fuxi-agent-cc/src/agent.rs:266-275 `send_message`（intervene 追加模式）：
「追加式介入：保持 active_tx 不换，直接发第二条 user message」

它假定**已有一个正在跑的 dispatch** (active_tx 已建立)。但场景是：
1. spawn 三个鲁班 → Idle，从未 dispatch 过，active_tx = None
2. fuxi intervene 把 "hi" 通过 WS 送给了 cc （日志里 sent follow-up user message 都到了）
3. 但没人接 cc 的回复—— active_tx 是空的，cc 的事件没有 receiver

日志里三次 intervene 后确实没有任何 thinking_started / agent_responded 事件，
印证了这点。

这是个 bug / UX 问题：intervene 在 Idle（无活跃 task）状态下应该：
 - 要么报错"门客空闲，请用 dispatch 派活"
 - 要么自动当作一次 dispatch
```

玄女给了两个选项。

## 决策

选 **选项 2：自动退化成 dispatch**，**反**玄女推荐的"short-term 报错"。

实装：`Fuxi::intervene` 入口先查 `shelf.status_of(agent_id)`，若 Idle：

```rust
let intervention_ev_id = {
    let mut meta = EventMeta::now();
    meta.agent = Some(agent_id);
    let id = meta.id;
    let _ = self.bus.publish(Event {
        meta,
        kind: EventKind::UserInterventionSent {
            target: agent_id,
            mode: "append_via_dispatch".to_string(),  // 退化标记
            text: text.to_string(),
        },
    });
    id
};
let task = Task::new("intervention", text);
self.dispatch(agent_id, task).await?;
// 抄送玄女（同 Busy 路径）
return Ok(());
```

## 为什么反玄女的推荐

玄女说 "short-term: intervene 检到 active_tx.is_none() 时报错"更安全。我反：

1. **UX 层**：用户派活的心智模型不是 "active_tx"，是 "我对这个门客说话"。Idle 对他来说是"你在吗"——要求他"先 dispatch 再 intervene"是把内部状态漏给用户
2. **玄女的 skill 也这么教**：`dispatch-protocol.md` 里告诉玄女「用户中途插话 → intervene」——玄女判断 idle/busy 全靠 `fuxi status` 后调不同 mode。但**她**可能漏判；自动退化让她不用知道这个细节
3. **语义一致**：`UserInterventionSent` 事件照发 + 抄送玄女正常走，bus 层面对订阅者无感
4. **代价小**：无非多一条 Task::new("intervention")，dispatch pump 跑完自动 Idle 回到原状

## 代价

- 多发了一条 `TaskCreated` / `TaskStateChanged` / `TaskDelivered` 事件（被当作新 task）
- `mode="append_via_dispatch"` 标记让订阅者能区分，但大多数订阅者不 care
- 没有"dispatch 失败"退路 —— 若 dispatch 本身报错，intervene 也报错（OK）

## 测试

新单测 `intervene_on_idle_auto_degrades_to_dispatch` 验证：
- 退化标记 `mode=append_via_dispatch` 发出
- `stub.dispatches` 计数 +1
- `stub.sends` 计数保持 0（没走原 send_message 路径）

## 未来

v2 若引入"真正的非 task 型介入"（如对话风格调整、注入 memory snippet），再区分 `intervene_config` vs `intervene_task`。当前单一概念够用。

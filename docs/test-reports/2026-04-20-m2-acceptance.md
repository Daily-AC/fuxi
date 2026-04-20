# M2 验收测试 · 2026-04-20（M2.1 + M2.2 + M2.3）

> commit: `5304eab` · 分支 `feat/fuxi-v0.1`
>
> 按下面三条逐项测。每条的【结果】行填 `✅ Pass` / `❌ Fail` / `⚠️ 部分通过` + 一句备注。全过 → 继续 M2.4/M2.5；有 Fail → 列到文档底。

---

## 0 · 环境准备

```bash
cd /Users/e0_7/fuxi
git log --oneline -3   # 应看到 5304eab
cargo install --path crates/fuxi-cli --force
which fuxi             # 应在 ~/.cargo/bin/fuxi
which codex            # M2.2 需要，没装就跳过 codex 测
```

**M2.2 codex 测前置**（可选）：

```bash
codex --version        # 能出版本号说明 CLI 在
# ChatGPT 账号登录用户可直接用；API key 用户须：
# export FUXI_CODEX_MODEL=gpt-4o-mini  # 或你 API key 可用的模型
```

**备份旧日志方便看新输出**：

```bash
mv /tmp/fuxi.log /tmp/fuxi.log.bak-$(date +%s) 2>/dev/null || true
```

【环境就绪】：

---

## 1 · M2.1 · busy 时发消息不丢（消息队列）

**为什么要测**：上一轮用户"§10 玄女 busy 时我发的消息凭空消失"。根因：`send_message` 直送 WS，cc 正在 tool loop 不 poll stdin → 吞。这轮加了 `PendingOutbox`。

**步骤**：

1. `fuxi` 启动 TUI。
2. 对玄女说：`详细解释 Rust 所有权的 3 条核心规则，每条至少写 4 句话并举一个具体代码例子`（让她进入较长的输出 turn）。
3. **她开始输出时**（能看到字在刷/有 thinking 状态），**立即连续**发 3 条短消息（每发完按 Enter 再打下一条）：
   - 第 1 条：`M21-test-1`
   - 第 2 条：`M21-test-2`
   - 第 3 条：`M21-test-3`
4. 等她当前 turn 结束（state 回 Idle）。观察她对这 3 条的处理。

**预期**：

- 三条消息**全部送达**，不丢（哪怕响应晚几秒）
- 她的下一 turn 应该看到 `M21-test-1/2/3` 三条，按顺序响应或合并回复
- TUI 对话区能看到三条用户消息都出现在对话里（竖条 `▍ `）

**反例**：如果只有最新一条到、或全部没到 → M2.1 没修好。

**调试辅助**：`tail -f /tmp/fuxi.log` 可以看 `pending: queued` / `pending: draining` 之类的日志（如果加了 tracing）。

**结果**：

---

## 2 · M2.2 · codex 门客能起能派

**为什么要测**：上一轮"§10 不能起 codex"。现在 `WorkerKind::Codex` 分支 + `fuxi-skills` loader 读 `metadata.cli` + 新 `skills/luban-codex/SKILL.md` 都到位。

**步骤**：

1. 确认 codex CLI 可用（`codex --version`）。没装跳过本条测。
2. `fuxi` 启动 TUI。
3. 对玄女说：`用 codex 起一个 luban-codex 门客，让他回一句 "hello from codex"`（玄女应该调 `fuxi spawn --role luban-codex` + `fuxi dispatch`）。
4. 观察事件面板（F2）和对话区。

**预期**：

- 事件面板出现 `● 上线 · luban-codex (cli=codex)` 或类似（cli 字段含 codex 标签）
- 右栏 meta 面板 active 切到 luban-codex 时能看到 `cli=codex`
- 门客真的回一句 "hello from codex" 或玄女复述门客的回复
- 没有 `invalid_request_error` 类错误（若有，多半是 FUXI_CODEX_MODEL 环境变量没设）

**反例**：
- `spawn 失败 unknown cli tag` → daemon 路由断
- `luban-codex` 起了但仍是 cc → loader 没读 metadata.cli（`cat skills/luban-codex/SKILL.md` 查 frontmatter 是否含 `metadata.cli: codex`）
- codex 起来但 `invalid_request_error` → 设 `export FUXI_CODEX_MODEL=xxx` 后重启 fuxi

**结果**：

---

## 3 · M2.3 · 玄女不再 poll `fuxi status`

**为什么要测**：上一轮图 4 玄女自承"对不起，轮询 15 次阻塞了主线"。这轮重写了 skill 的 axioms #3 + SKILL.md soul + dispatch-protocol §3/§4，明确"headless = 没背景线程，派完闭嘴"。

**步骤**：

1. `rm /tmp/fuxi.log; fuxi`（清日志，新开 session，**不**要 resume 的话先 `mv ~/.fuxi/memory.db ~/.fuxi/memory.db.bak`）。
2. 对玄女说：`起一个鲁班，让他慢慢数 1 到 30，每数一个 sleep 1 秒`（这会让鲁班 busy 30 秒）。
3. 玄女应该派完活说一句 "派令已发" 类的话，然后**停下**。
4. **不要打扰她**，等 30 秒（看你的手表，别问她"好了吗"）。
5. 这 30 秒里观察：
   - TUI 对话区玄女**不应该**自己输出新内容
   - 右栏 active 应该是鲁班，tasks=1，worker state busy
   - `tail -n 200 /tmp/fuxi.log | grep -c "fuxi status"` 应 **0 次或 1 次**（"fuxi status" 不应被玄女反复调）
6. 鲁班完活后，`SystemEventBridge` 应该把"门客 X 任务已完成"的 prompt 注入玄女，她自然回复。

**预期**：

- 30 秒等待期玄女零输出（她在闭嘴）
- `fuxi.log` 里 `fuxi status` 调用次数 ≤ 1（派活前可能查一次 role 列表，派完活后不 poll）
- 鲁班完活后玄女主动汇报"鲁班完活了，数完了 30"

**反例**：
- 等待期玄女反复输出"让我看看鲁班情况…调 `fuxi status` 查一下…" → skill 没起效
- 她在 30 秒里调 10+ 次 `fuxi status` → M2.3 没收敛到

**备忘**：M2.3 是**概率性**改善，不是硬保证——cc 的默认"积极帮助"倾向偶尔会绕过 skill 约束。如果 3 次测试里 2 次过就算 pass。

**结果**：

---

## 4 · 综合判定

**全过** → 告诉我 "M2 前三件 ok，开 M2.4+M2.5"，我下一 session 推进。

**有 Fail**：

```
[填] 条目: (1/2/3)
[填] 现象:
[填] /tmp/fuxi.log 相关行:
[填] 是否阻塞下一步:
```

---

## 5 · 测试元信息

| 项 | 值 |
|---|---|
| 测试日期 | [待填] |
| 终端 | [待填] |
| codex CLI 版本 | [待填 或 skipped] |
| 总用时 | [待填] |
| 最终结论 | [待填] |

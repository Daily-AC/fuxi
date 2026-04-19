# 派门客 → 汇报 → 回收 协议

## 1. 接需求

用户说话 → 我**先用一句中文回应**（"收到，我让一个鲁班去做"）。
不复述用户原话，不解释自己怎么思考。

## 2. 起兵 + 派活

```bash
ID=$(fuxi spawn --role luban | tail -n1)
fuxi dispatch --to "$ID" '<把用户意图翻成清晰的工序，说人话>'
```

派活的 `<msg>` 必须包含：
- 目标（要做什么）
- 边界（哪些不动）
- 验收（怎么算完）

## 3. 中途观察

事件流自己渲染，我不轮询。**只在三种节点开口对用户说话**：
- 起兵（spawn / dispatch 完成时）
- 重大转折（门客 blocked / 失败 / 改变方向）
- 收尾（汇报结果）

## 4. 用户中途插话

判断目标门客状态：
- **idle** → `fuxi intervene --to <id> --mode append`
- **busy** → `fuxi intervene --to <id> --mode interrupt`

不要自己揣测——拿不准就 `fuxi status` 看一眼。

## 5. 门客请示授权

门客到达需用户授权的节点（commit / push / 删文件 / 改全局配置）会停在
`awaiting_*` 状态。我**代它向用户请示**：

> 「鲁班想 commit："feat: 新增 X 模块"——可以吗？」

拿到明确"同意"再 `fuxi dispatch --to <id> '继续 commit'`。
**不擅自放行**。

## 6. 汇报

任务完成 → 简短一句：改了什么 + 测试结果 + 是否需要 commit。
不写 plan 文档，不复读门客的输出，不溢美。

## 7. 收兵

```bash
fuxi kill "$ID"
```

任务真的结束才 kill。中途用户改方向不 kill——保留 session，新派任务。

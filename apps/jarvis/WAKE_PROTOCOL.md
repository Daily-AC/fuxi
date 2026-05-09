# Jarvis Wake Protocol

Mac 贾维斯 ↔ home `fuxi-wake-server` 之间的协议契约。两边按这份对接。

## 端点

```
wss://wake.qmledmq.cn/api/wake
ws://127.0.0.1:9101/api/wake     # 本地 dev
```

服务监听端口默认 **9101**（与 fuxi-im 9100 错开）。家用部署 nginx 反代到 wake 子域 + 通配符证书。

## 鉴权

WebSocket 升级握手时带：
```
Authorization: Bearer <pair_token>
```

`pair_token` = mac App 在设置面板里填，跟 fuxi-im pair 走同一颗 token（home server 共享一份 secrets，wake-server 启动时 read `~/.fuxi/wake.token` 文件，文件 600 权限）。

握手成功 server 回 `101 Switching Protocols`；token 错回 `401`。

## 上行（mac → home）

### 二进制帧 = 音频
- `WebSocket.Message.data`
- 内容：**16 kHz mono PCM s16le** 裸字节
- 推荐每帧 640 samples（40 ms）= 1280 bytes
- 不要做静音裁剪 / 不要 OPUS（v0.1 简化；带宽 256 kbps，家庭宽带和蜂窝都 OK）

### 文本帧 = JSON 控制
- `{"type":"hello","client":"jarvis-mac","version":"0.1.0"}` —— 第一帧；server 收到后初始化讯飞 session
- `{"type":"bye"}` —— 客户端主动下线
- `{"type":"pong","at":"<rfc3339>"}` —— 心跳响应
- `{"type":"keywords","words":["玄女","贾维斯"]}` —— 客户端要求切换关键词集（v0.1 server 端写死 `["玄女"]`，先不实现切换）

## 下行（home → mac）

全部是文本帧 JSON：

- `{"type":"ready","keywords":["玄女"]}` —— `hello` 后回；表示 SDK session 已就绪可以收音频
- `{"type":"wake","keyword":"玄女","score":0.85,"at":"<rfc3339>"}` —— 唤醒命中
- `{"type":"ping","at":"<rfc3339>"}` —— 服务端心跳，**5 秒一次**；客户端必须回 `pong`
- `{"type":"error","code":"<code>","message":"<人话>"}` —— 错误下发
- `{"type":"bye"}` —— 服务端主动断（升级 / 重启等）

### error code 词表

| code | 含义 | 客户端行为 |
|---|---|---|
| `unauthorized` | token 不对（一般连前就被 401 拒，这条只在过期场景） | 触发 fallback，弹设置页提示重新 pair |
| `sdk_unavailable` | 讯飞 SDK 初始化失败 / 装机量耗尽 / 授权过期 | 触发 fallback，记 log 提醒续费 |
| `audio_format_invalid` | 上行音频格式不对 | 不重连，弹错给开发者 |
| `rate_limited` | 频率超限 | 退避后重连 |

## 心跳与超时

- 服务端每 5s 发 `ping`，客户端必须 5s 内回 `pong`（10s 总窗口）
- 客户端如 30s 内未收到任何下行帧 → 视为断线 → 关 socket → 走 fallback + 重连退避
- 服务端如 15s 内未收到任何上行帧 → 关连接

## 重连退避

客户端断线重连：1s → 2s → 4s → 8s → 16s → 30s 上限。重连成功重置为 1s。

重连失败累计 3 次 → 切换到 `LocalWakeFallback`（Apple Speech 持续监听）；再每隔 60s 试一次主连接，恢复后切回。

## 事件去重

服务端两次 `wake` 事件之间必须有 ≥ 1.5s 静音段（防止单次"玄女"被切两个发音段误报两次）。客户端不再做去重。

## 可观测

- 服务端 log 每个连接的 `client_id`（Bearer token 的前 8 位）+ 唤醒计数 + 累计音频时长
- HTTP `GET /health` 返回 `{"status":"ok","sdk":"ready|degraded|down","awake_count":<int>}`
- HTTP `GET /metrics` Prometheus 格式（v0.2 加，v0.1 不必）

## 测试模式

服务端环境变量 `FUXI_WAKE_MOCK=keyword` 时绕开讯飞 SDK，命中规则改为：每 30s 上行有连续 1s 帧（任意内容）→ 触发一次 `wake` 事件。便于 mac 端联调时不依赖 SDK。

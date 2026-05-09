# fuxi-wake-server 部署

家用 Linux 上跑的玄女唤醒守护——给 Mac 贾维斯反向通道。协议契约见
`apps/jarvis/WAKE_PROTOCOL.md`。

## 总体形态

```
Mac 贾维斯 ──── wss://wake.qmledmq.cn/api/wake ────▶ nginx :8443
                    Bearer <wake.token>                  │
                                                         ▼ proxy_pass
                                                  fuxi-wake-server :9101
                                                         │
                                                         ▼
                                                   ┌──────────────┐
                                                   │  WakeEngine  │
                                                   │ MockEngine   │← v0.1 默认
                                                   │ XfyunEngine  │← 等 SDK
                                                   └──────────────┘
```

## 部署步骤（home 节点）

### 0. 一次性准备

```bash
# 在 dev 机交叉编译 Linux x86_64 binary（home 用 x86_64 假设）
cd /Users/e0_7/fuxi
cargo build --release -p fuxi-wake-server --target x86_64-unknown-linux-gnu

# scp 到 home
scp target/x86_64-unknown-linux-gnu/release/fuxi-wake-server home:.local/bin/
```

如果 dev 是 mac arm64 没装 linux toolchain，直接在 home 上 `cargo build --release` 也可。

### 1. 写 token

```bash
ssh home
mkdir -p ~/.fuxi
# 32 字节随机——跟 fuxi-im pair token 同一颗（Mac 端两边填同一个）
openssl rand -hex 32 > ~/.fuxi/wake.token
chmod 600 ~/.fuxi/wake.token
```

把 token 内容贴进 Mac 贾维斯 → 设置 → 唤醒 → Wake Token。

### 2. systemd 服务

```bash
sudo cp /home/e0-7/fuxi/deploy/wake/fuxi-wake.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now fuxi-wake.service
sudo systemctl status fuxi-wake.service
```

健康检查（绕 nginx，验本机直连）：

```bash
curl -s http://127.0.0.1:9101/health | jq
# {"status":"ok","sdk":"ready","awake_count":0}
```

### 3. nginx 反代

```bash
sudo cp /home/e0-7/fuxi/deploy/wake/nginx.conf /etc/nginx/sites-available/wake
sudo ln -sf /etc/nginx/sites-available/wake /etc/nginx/sites-enabled/wake
sudo nginx -t && sudo systemctl reload nginx
```

DDNS-go + 通配符证书 `qmledmq.cn` 已就位（家庭机 baseline）。新加 `wake.qmledmq.cn`
子域时，DDNS-go 配置加一条 A 记录指向公网 IP 即可。

### 4. 联调

mac 端贾维斯填：
- Wake Server URL: `wss://wake.qmledmq.cn:8443/api/wake`
- Wake Token: `<上面 ~/.fuxi/wake.token 内容>`

或本机 dev：
- `ws://127.0.0.1:9101/api/wake`（贾维斯跟 home 同机时；Mac 反连家用要走 wss）

mock 模式下大约 30s 一次假唤醒——验链路用。

## 切讯飞 SDK（task #5 已 ship FFI）

v0.2 起 `engine/xfyun.rs` 已接真 FFI（`#[cfg(xfyun_ffi)]` Linux x86_64）。home 切讯飞步骤：

### 1. SDK 落地

```bash
# rsync 整 SDK 目录到 home
rsync -avP /Users/e0_7/fuxi/Linux_ivw_e867a88f2_v1.0.11_v2.2.15-rc5/ home:/opt/fuxi-wake-sdk/
```

### 2. 在 home 编 release binary

```bash
ssh home
cd ~/fuxi
FUXI_XFYUN_SDK_DIR=/opt/fuxi-wake-sdk \
    cargo build --release -p fuxi-wake-server
sudo cp target/release/fuxi-wake-server /home/e0-7/.local/bin/
```

build.rs 看到 `FUXI_XFYUN_SDK_DIR` + Linux x86_64 → 自动 bindgen + 链接 `libaikit.so`，
设 `cfg(xfyun_ffi)` 走 `engine/xfyun/linux.rs` 真 FFI 路径。

### 3. workDir + ENV 三件套

```bash
# workDir 必须可写 + 持久——讯飞 license 会落这里，systemd unit 已写死
sudo mkdir -p /var/lib/fuxi-wake
sudo chown e0-7:e0-7 /var/lib/fuxi-wake

# ENV 三件套（讯飞控制台拿）写到一个 systemd drop-in，避免 service 文件含密钥
sudo systemctl edit fuxi-wake
# 在 override 里写：
# [Service]
# Environment=FUXI_XFYUN_APPID=<appid>
# Environment=FUXI_XFYUN_API_KEY=<api_key>
# Environment=FUXI_XFYUN_API_SECRET=<api_secret>
```

### 4. 切到 xfyun 模式

改 `/etc/systemd/system/fuxi-wake.service` ExecStart 行——去掉 `--mock`：

```diff
- ExecStart=/home/e0-7/.local/bin/fuxi-wake-server --bind 0.0.0.0:9101 --mock
+ ExecStart=/home/e0-7/.local/bin/fuxi-wake-server --bind 0.0.0.0:9101
```

ENV `FUXI_WAKE_MOCK` 不设（service 文件里没有），daemon 默认走 xfyun 路径。

```bash
sudo systemctl daemon-reload && sudo systemctl restart fuxi-wake
journalctl -u fuxi-wake -n 80 --no-pager
```

启动期日志应能看到：
```
xfyun: AIKIT_Init ok
xfyun: AIKIT_EngineInit ok
xfyun: AIKIT_LoadData ok
wake-server: serving
```

### 5. 首次激活联网

讯飞 SDK 第一次跑会联 `aee.xf-yun.com` 拿 license——home 必须出公网（DDNS-go +
公网 IP 已就位的话默认通）。激活后 license 落 `/var/lib/fuxi-wake/`，90 天离线可用。

如 systemd 启动失败：
- 看 `journalctl` 有没有 `AIKIT_Init failed:<err>`——常见 err=18801（appID 错）/ 18807（认证失败）
- `ss -tlnp | grep 9101` 确认无残留进程占端口
- 网络 issue：临时 `curl -v https://aee.xf-yun.com` 探一下出公网

## 排错

| 现象 | 排查 |
|---|---|
| Mac 端 401 | `cat ~/.fuxi/wake.token` vs 贾维斯设置面板填的 token 字面比对 |
| Mac 端 30s 超时 | `journalctl -u fuxi-wake -n 50` 看 hello / ready 是否打到 |
| `connect_async` ECONNREFUSED | nginx -t 看有没有冲突；`ss -tlnp \| grep 9101` 看 fuxi-wake 是否真起 |
| mock 模式 30s 没收到 wake | 客户端要持续发 PCM 帧——空连接不喂 audio engine 的 feed 不会 fire |
| sdk_unavailable | XfyunEngine 返这个错说明讯飞 SDK 装机量耗 / auth 错 / appid 不对 |
| 突然 fallback | mac 30s 内未收到任何下行帧 → 服务端可能挂或网络丢包，先看 health endpoint |

## 协议改动

如要改 wire 形态（加新 wake 字段、改 ping 频率等），**两边同步改**：
- `crates/fuxi-wake-server/src/protocol.rs` 服务端 enum
- `apps/jarvis/Sources/Voice/WakeWord.swift::WakeFrame` mac 端
- `apps/jarvis/WAKE_PROTOCOL.md` 契约文档

三处不一致 = 静默不工作。提 PR 前 grep 一遍三处都改。

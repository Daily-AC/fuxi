# 接班 handoff · 2026-06-04 · fuxi-im 实时消息 bug + 呆萌重构收尾

> 上一会话上下文太大，换新会话续。本文件是开工指引。先读 memory 索引（尤其
> `project_im_daimeng_redesign_shipped_2026_06_04` / `reference_cc_version_pin` /
> `reference_home_deploy` / `feedback_mock_match_real_wire`），再读本文件。

## 当前状态（都已 ship 到 main + home）

呆萌风重构**已全套上线并验证**：奶油糖果 token + 质感层 + 玄女 8 帧透明桌宠（会挥手/说话/惊讶/开心/戳俏皮话）+ 4-tab 导航（家/聊天/任务/更多）+ 15 页 5 原型重绘 + PWA 图标奶油化。单测 417 / e2e 24 / lint 0 全绿。spec/plan 在 `docs/superpowers/{specs,plans}/2026-06-04-*`。分支 `feat/fuxi-im-daimeng-redesign` 已 merge 到 main（`ac91f1d`）+ push。

**刚修过**：home claude 半夜自动升到 2.1.161 把玄女弄死（`--sdk-url` 被拒）——已回滚 2.1.114 + 加 `DISABLE_AUTOUPDATER=1` systemd drop-in 根治。玄女现在正常（实测回「在，哥哥」）。详见 [[reference_cc_version_pin]]。

## 🔴 待修 P0：玄女回复不实时（要切 tab 才刷出来）

**现象**（用户原话）：在 IM 上玄女回复的消息要切一下 tab 才能刷出来，不像飞书那种实时性。

**已知线索**（上一会话定位到的）：
- 消息流逻辑本身没坏：`src/views/pages/XuannvPage.tsx` 里 `const [messages,setMessages]=createSignal`，WS `onMessage:(e)=>handleEvent(ev)` → `setMessages(prev=>applyEvent(prev,ev))`，`<Conversation messages={messages}/>` 传的是响应式 accessor。这套是对的。
- 切 tab 能刷出来 = **切 tab 重挂载 XuannvPage → onMount 重拉 `/api/conv/messages` 历史**（`setMessages(prev=>mergeMessages(prev,seeded))`），所以消息其实在后端、只是没实时 push 到视图。
- 所以 bug 大概率在：**WS（`/api/conv`）在聊天页挂载期间没连上/没收到实时帧**，而不是渲染层。

**怀疑方向（按优先级，用 superpowers:systematic-debugging 逐个证伪）**：
1. **WS 没连上**：页面顶栏有 `online()` 指示（`{online()?"在线":"重连中"}`，XuannvPage.tsx:333）。先看真机上聊天时它显示啥。`startReconnectingSocket`（`src/lib/reconnect.ts`）+ `setOnline`。WS 走 `?token=` 鉴权（浏览器 WS 不能设 header）——token 对不对、URL 对不对。
2. **新导航的挂载交互**：旧模型 XuannvPage 是默认 tab 0 常驻；现在是 tab 1，Solid `<Switch>/<Match>` 只渲染 active tab → 离开聊天 tab 就 unmount、WS 断。**但用户是停在聊天页时不实时**，所以这条可能不是主因，但要排除「家 tab 在前台时 WS 生命周期」的影响。
3. **玄女 id 漂移**（[[feedback_dynamic_agent_id_via_watch]]）：我重启过玄女，她 agent id 变了（现 `agent-2c36811b`）。若前端/后端 conv WS 按**快照的旧 xuannv id** 过滤，新玄女的输出就不会实时 push（但历史重拉无视 id 所以能刷出来）——高度吻合现象！查后端 `/api/conv` WS 推送是否按 xuannv id 过滤、是否走 watch::Receiver 实时取 id。日志见 `journalctl -u fuxi-im | grep "ws /api/conv accept"`（会打 `xuannv=agent-xxx`）。
4. **后端 bridge 没把玄女输出推到 conv 流**：上一会话 curl `POST /api/intervene` → 200 + `agent_responded` 事件入库，但没确认该事件**实时**经 `/api/conv` WS 推给在线 client。查 `crates/fuxi-im/src/handlers/conv.rs` + `fuxi_orchestrator::bridge`。

**怎么真机调**：用 Playwright 开 `https://im.qmledmq.cn:8443`（或 home localhost:9100）连真后端，开 devtools 看 WS 帧；或加临时 log。**注意**：上一会话的单测/e2e 全是 mock 后端——这个 bug mock 测不出来（[[feedback_mock_match_real_wire]] 的同类陷阱），**必须真后端验**。

**真后端 curl/鉴权配方**（mint 一个测试 token）：
```bash
ssh home 'python3 -c "import base64,hashlib,hmac,json;k=open(\"/home/e0-7/.fuxi/im_hmac.key\",\"rb\").read().strip();b=json.dumps({\"device_id\":\"t\",\"name\":\"t\",\"expires_at\":\"2027-01-01T00:00:00Z\"},separators=(\",\",\":\")).encode();s=hmac.new(k,b,hashlib.sha256).digest();f=lambda x:base64.urlsafe_b64encode(x).decode().rstrip(\"=\");print(f(b)+\".\"+f(s))"'
# → 拿到 token，curl -H "Authorization: Bearer <token>" http://127.0.0.1:9100/api/...
# WS 用 ?token=<token>。上一会话实测 14 个 GET 全 200、intervene 200、玄女回复正常。
```

## 关键事实 / 部署（省得再踩）
- **纯前端部署 = `cd crates/fuxi-im/web && npm run build && env -u HTTPS_PROXY rsync -az --delete dist/ home:.local/share/fuxi/im-web/`**。PWA 是运行时 ServeDir（`crates/fuxi-cli/src/im.rs` `ServeDir::new(web_root)`，web_root=`~/.local/share/fuxi/im-web/`），**不用重编 Rust、不用重启服务**。安卓 app 走远程 URL `https://im.qmledmq.cn:8443`，rsync 完刷新即见。
- 改后端 Rust 才走 [[reference_home_deploy]]（rsync crates + cargo build + cp binary 两份 + 重启）。
- 截图 gate 工具：`crates/fuxi-im/web/scripts/shot.mjs <out.png> [tabTestid] [moreTile] [dataJson]`（mock 后端自动登入截图，移动视口）。
- 真数据 wire 形式：conv message 的 `content` 是 `{"text":"..."}` **JSON 对象**不是裸 string（mock fixture 用的是 string，差异是 latent bug 源，见 [[feedback_mock_match_real_wire]]）。前端 `fromStoredMessage` 处理对象形式。
- 吉祥物源图在 `crates/fuxi-im/web/design-assets/`（gitignore，可由 gpt-image-2 i2i + rembg 重生）。
- 用户硬规则：UI 禁 emoji（[[feedback_no_emoji_ui_too]]）、质感拉满反塑料反 AI（[[feedback_premium_texture_no_plastic]]）。

## 次要 / 可选
- 安卓原生启动屏/状态栏已改奶油 + launcher 换玄女头像（commit `8f0fab5`），新 APK 已 `fuxi deliverable` 交付到「交付物」tab（`xuannv-daimeng.apk`）。用户装它才生效原生图标/启动屏；in-app UI 走远程 URL 已是新的。

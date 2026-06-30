# CF DNS records 清理清单（qmledmq.cn @ 2026-06-30）

Zone ID: `791621b616f83a44a34e4796adbe0920`
快照时间: 2026-06-30
当前 NS: `jermaine.ns.cloudflare.com` + `perla.ns.cloudflare.com`

总 42 条 records。下面按用途分组，每组给「推荐动作」+ 理由。**勾完发给我，我用 CF API 批量改/删**。

---

## A. 基础设施 records（必留，4 条）

| 类型 | name | content | TTL | proxied | 说明 |
|---|---|---|---|---|---|
| A | `home.qmledmq.cn` | `124.126.5.223` | 300 | × | **主入口 A 记录**，所有家里 CNAME 都指它，未来 ddns 写这条 |
| A | `qmledmq.cn` | `124.126.5.223` | 300 | × | 裸域名，方便 `https://qmledmq.cn:8443` 直访 |
| CNAME | `*.qmledmq.cn` | `home.qmledmq.cn` | auto | × | **三级 wildcard，cert SAN 覆盖**。所有显式三级 CNAME 在它存在时是冗余 |
| CNAME | `*.lab.qmledmq.cn` | `home.qmledmq.cn` | auto | × | **四级 wildcard for lab**，cert SAN 覆盖 |

**推荐**：☑ 全留。

---

## B. 显式三级 CNAME → home.qmledmq.cn（**可全清，被 wildcard 兜底**）

共 24 条。`*.qmledmq.cn` wildcard 已存在且 cert SAN 已覆盖，**这批显式 CNAME 一删就净**。除非你想给某个子域名走不同 IP（如指向 VPS / cfargotunnel），否则留着是噪音。

| name | 当前 → | 备注 / 你曾用做什么 |
|---|---|---|
| `acm-oj.qmledmq.cn` | home | acm 题目站？ |
| `bf.qmledmq.cn` | home | ? |
| `blog.qmledmq.cn` | home | 博客 |
| `chat.qmledmq.cn` | home | ? |
| `drive.qmledmq.cn` | home | 网盘 |
| `edu.qmledmq.cn` | home | ? |
| `fuxi.qmledmq.cn` | home | **fuxi PWA 入口**（im.qmledmq.cn 现在更新，fuxi 可能旧） |
| `gugu-api.qmledmq.cn` | home | ? |
| `hub.qmledmq.cn` | home | ? |
| `im.qmledmq.cn` | home | **fuxi IM PWA 当前入口** |
| `jot.qmledmq.cn` | home | 笔记应用？ |
| `kaiwu.qmledmq.cn` | home | ? |
| `lab.qmledmq.cn` | home | lab 根（注：`*.lab.qmledmq.cn` 是另一条 wildcard，**这条单独保不保看你要不要 `https://lab.qmledmq.cn:8443` 走通**） |
| `oj.qmledmq.cn` | home | online judge |
| `oled.qmledmq.cn` | home | mood-server OLED 站？ |
| `play.qmledmq.cn` | home | ? |
| `router.qmledmq.cn` | home | ? |
| `sia.qmledmq.cn` | home | sia |
| `story.qmledmq.cn` | home | story 服务 |
| `talk.qmledmq.cn` | home | ? |
| `term.qmledmq.cn` | home | terminal WS |
| `tmp-yxl.qmledmq.cn` | home | 一次性？ |
| `voice.qmledmq.cn` | home | jarvis / wake / sovits? |
| `warmme.qmledmq.cn` | home | ? |

**推荐动作**（请勾）：

- [ ] **全删**（最干净，全靠 `*.qmledmq.cn` 兜底） ← 推荐
- [ ] 只删带 `?` 的（记不清的）
- [ ] 全留（保留可读性）
- [ ] 自定义勾，下面逐项标 ☑/☐：

```
acm-oj   bf   blog   chat   drive   edu   fuxi   gugu-api
hub      im   jot    kaiwu  lab     oj    oled   play
router   sia  story  talk   term    tmp-yxl  voice  warmme
```

> 注：删 CNAME **不影响 Caddy 工作**，因为 wildcard CNAME 仍兜底。但如果某子域名在 CF 后台看不到，新人接手时不知道这服务存在 → **建议保留少数关键 entry（im / fuxi / lab / sia / voice）**，其他清。

---

## C. CF Tunnel CNAME（11 条，**问你是否还在用**）

全部指向 `42a9693a-8809-4ef3-b011-a797340d3498.cfargotunnel.com`，全部 `[proxied]`（橙云）。这是另一套部署路径——通过 CF Tunnel 暴露内网服务到公网，**不经家宽 8443，不依赖动态 IP**。

| name | 看名字猜 |
|---|---|
| `apps.qmledmq.cn` | apps 集合页 |
| `inkblade.qmledmq.cn` | 公司产品？ |
| `inkledger.qmledmq.cn` | 公司产品？ |
| `inkorder.qmledmq.cn` | 公司产品？ |
| `jjfb.qmledmq.cn` | ? |
| `liuyi.qmledmq.cn` | ? |
| `mood.qmledmq.cn` | mood-server? |
| `moyun.qmledmq.cn` | ? |
| `ssh.qmledmq.cn` | ssh-over-https tunnel? |
| `tmp.qmledmq.cn` | 一次性 |
| `yapyap-api.qmledmq.cn` | ? |

**关键问题**：tunnel `42a9693a-...` 现在还在跑吗？跑在哪台机？

- 如果**还在跑**：留这些 records，但请告诉我跑在哪（runbook 要写进去）
- 如果**已废弃**：全删 + 找到 tunnel 在 CF dashboard 里 delete 掉（CF Zero Trust → Networks → Tunnels）

**推荐动作**（请勾）：

- [ ] tunnel 还在跑，全留
- [ ] tunnel 已废弃，全删 records + delete tunnel
- [ ] 只留部分，逐项标：apps☐ inkblade☐ inkledger☐ inkorder☐ jjfb☐ liuyi☐ mood☐ moyun☐ ssh☐ tmp☐ yapyap-api☐

---

## D. 奇异 A records（2 条，问你这是什么）

| 类型 | name | content | TTL | proxied | 谜团 |
|---|---|---|---|---|---|
| A | `clash.qmledmq.cn` | `86.53.183.23` | 1 (auto) | × | **不是家宽 IP**。VPS？clash 订阅链接？ |
| A | `cliproxy.qmledmq.cn` | `86.53.183.23` | 1 (auto) | ✓ | 同 IP，CF 代理（橙云）。cli 工具反代？ |

`86.53.183.23` 反查 = 英国 BT (Sky Broadband) 家宽段。看着像你某个境外 VPS / 朋友家机器。**问你是什么、还在用吗**：

- [ ] 还在用，留
- [ ] 不知道 / 不用了，删
- [ ] 留一条删一条（标：clash☐ cliproxy☐）

---

## E. ACME challenge 残留（1 条，**建议删**）

| 类型 | name | content |
|---|---|---|
| TXT | `_acme-challenge.home.qmledmq.cn` | `"d4Q1o9oPjeBsQ8UXRYjuBf3ZGaeKsDwPjF8MSZvRKJY"` |

acme.sh DNS-01 challenge 完成后**应该 cleanup**，但 acme.sh 有时不清干净。这条 TXT 残留：
- 不影响功能（challenge 早过了）
- 但下次续签写新 TXT 时可能撞 CF API `81058 An identical record already exists`，要 retry

**推荐**：☑ 删。下次 acme.sh 自己写新的。

---

## F. NS records / 阿里云万网 hichina 残留

CF 这边 NS = jermaine + perla。**hichina 残留只可能在阿里云万网那边**。

**自助清理步骤**（只有你能做，Claude 没万网账号）：

1. 登 https://wanwang.aliyun.com
2. 顶栏「域名」→ 域名列表 → 找到 `qmledmq.cn`
3. 右边「管理」→ 左侧「DNS 修改」→ 确认 NS 是 CF 的两条
4. 左侧「域名解析（DNS Domain Name Resolution）」服务 → 如果还能看到 records：
   - 截图备份
   - 全删（或暂停整个解析服务）
5. 拿截图回来给我对一下有没有当前 CF 里没有的「孤儿 records」

> 顺手验：`dig +short NS qmledmq.cn @8.8.8.8` 应只返 CF 那两条；`dig +short NS qmledmq.cn @dns27.hichina.com` 看 hichina 是否还认这 zone。

`dnspod`（腾讯云 DNS）**没出现过**，是你记忆里把 hichina 跟 dnspod 混了。

---

## 一次性批量操作（你勾完后我跑）

```bash
# CF API token：见 memory project_home_server_windows_handoff_2026_06_30
# 或本地 /Users/e0_7/fuxi/secrets/cf-token（建议长期方案见 project_secrets_cli_plan_2026_06_29）
TOKEN=<CF_API_TOKEN>
ZONE=791621b616f83a44a34e4796adbe0920

# 列 records（含 id）
curl -sS -H "Authorization: Bearer $TOKEN" \
  "https://api.cloudflare.com/client/v4/zones/$ZONE/dns_records?per_page=200" \
  | jq '.result[] | {id, name, type, content}'

# 删单条
curl -X DELETE -H "Authorization: Bearer $TOKEN" \
  "https://api.cloudflare.com/client/v4/zones/$ZONE/dns_records/<RECORD_ID>"
```

我会先 list + dump 一份 records ID 备份到 `~/fuxi/docs/cf-dns-backup-2026-06-30.json` 再删，可以 replay。

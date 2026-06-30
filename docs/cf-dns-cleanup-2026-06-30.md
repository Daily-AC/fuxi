# CF DNS records 清理记录（qmledmq.cn @ 2026-06-30）

Zone ID: `791621b616f83a44a34e4796adbe0920`
执行时间: 2026-06-30 16:25-16:30
执行方式: CF API (curl + python3) 批量 DELETE
Backup: `~/.claude/jobs/ec81ecb3/tmp/cf-records-backup-2026-06-30.json`（含全 42 条 records 完整元数据）

---

## 终态（4 条）

```
A      home.qmledmq.cn      124.126.5.223       (ddns-go 维护)
A      qmledmq.cn           124.126.5.223
CNAME  *.qmledmq.cn         home.qmledmq.cn     (三级 wildcard)
CNAME  *.lab.qmledmq.cn     home.qmledmq.cn     (四级 wildcard)
```

---

## 删除记录（38 条）

### B. 显式三级 CNAME → home.qmledmq.cn（24 条）

被 `*.qmledmq.cn` wildcard 兜底，删除后任意子域名仍能访问。

`acm-oj / bf / blog / chat / drive / edu / fuxi / gugu-api / hub / im / jot / kaiwu / lab / oj / oled / play / router / sia / story / talk / term / tmp-yxl / voice / warmme`

### C. CF Tunnel CNAME → 42a9693a-...cfargotunnel.com（11 条）

`apps / inkblade / inkledger / inkorder / jjfb / liuyi / mood / moyun / ssh / tmp / yapyap-api`

**注**：tunnel 对象本身保留在 CF Zero Trust 里，未来重新通过 tunnel 暴露公网时走 Zero Trust → Public Hostnames 自动重建 CNAME。

### D. 奇异 A records → 86.53.183.23（2 条）

`clash / cliproxy`（英国 BT/Sky 段 IP，VPS 不再使用）

### E. ACME challenge 残留（1 条）

`_acme-challenge.home.qmledmq.cn` TXT — acme.sh DNS-01 没清干净的残留，下次续签自动写新的。

---

## hichina 残留（待用户登 wanwang.aliyun.com 自助清）

CF zone 这边 NS 切到 CF active，但**阿里云万网域名后台**可能仍有旧 records（不生效但属历史垃圾）：
1. 登 https://wanwang.aliyun.com
2. 域名 qmledmq.cn → 解析设置
3. 截图备份 → 全删 / 暂停整个解析服务

`dnspod` 没出现过，是历史记忆混淆为 hichina。

---

## 验证

- ✓ CF API 列 final = 4 records
- ✓ `dig im.qmledmq.cn / fuxi.qmledmq.cn / voice.qmledmq.cn / newrandom.qmledmq.cn @1.1.1.1` 全部解析成功（命中 `*.qmledmq.cn` wildcard）
- ✓ `curl https://im.qmledmq.cn:8443/ --resolve im.qmledmq.cn:8443:124.126.5.223` HTTP 200（Caddy 兜底 placeholder respond）
- ✓ 端到端 wildcard 链路 work

# acme-sh

> Let's Encrypt wildcard cert 自动续签。DNS-01 challenge via Cloudflare。

## 部署位置
- 机器：winhome 上的 WSL Ubuntu 24.04
- Binary：`/root/.acme.sh/acme.sh`（root 装）
- Service manager：cron（自动 daily check + renew if expiring）
- Working dir：`/root/.acme.sh/`

## 端口 / 地址
- _n/a_（出网到 Let's Encrypt + CF API）

## 配置
- 主：`/root/.acme.sh/account.conf`（含 CF API token / acme account）
- cert 输出：`/etc/nginx/ssl/qmledmq.cn.{crt,key}`（install hook 配的）
- cert 仓库：`/root/.acme.sh/qmledmq.cn_ecc/`

## SAN（当前）
```
qmledmq.cn
*.qmledmq.cn          # 三级 wildcard
*.lab.qmledmq.cn      # 四级 wildcard for lab
```
有效期 → 2026-09-27（每月 acme.sh 自动 renew）。要加新四级 wildcard 见 [home-runbook.md#常见运维-2](../../home-runbook.md)。

## 依赖
- CF API token（`Zone:DNS:Edit` for qmledmq.cn）—— acme.sh env var `CF_Token` 在 `account.conf`
- 凭据：见 [refs/secrets-locations.md](../refs/secrets-locations.md)
- 下游：[caddy-cert-sync](caddy-cert-sync.md) 拉 cert 到 Windows

## 启动 / 停止 / 重启
```bash
# WSL Ubuntu 内 root
/root/.acme.sh/acme.sh --list                  # 看当前 cert + 到期日
/root/.acme.sh/acme.sh --cron                  # 手动跑一次 cron check
/root/.acme.sh/acme.sh --issue --force --dns dns_cf \
  -d qmledmq.cn -d "*.qmledmq.cn" -d "*.lab.qmledmq.cn" --keylength ec-256   # 强制 reissue

# cron 已注册：
crontab -l | grep acme
```

## 健康检查
```bash
# WSL 内
openssl x509 -in /etc/nginx/ssl/qmledmq.cn.crt -noout -dates -ext subjectAltName
# Windows 端是否同步过来
diff <(openssl x509 -in /etc/nginx/ssl/qmledmq.cn.crt -outform PEM) <(cat /mnt/c/Caddy/certs/qmledmq.cn.crt)
```

## 日志
- acme.sh 自带 log：`/root/.acme.sh/acme.sh.log`

## 变更历史
- 2026-06-29：装好 + 首次 issue（SAN = qmledmq.cn / *.qmledmq.cn / *.lab.qmledmq.cn）。有效期 2026-09-27。

## 已知问题 / 坑
- ⚠️ acme.sh DNS-01 完成后**有时不清 `_acme-challenge.<domain>` TXT 记录**，留 CF 上孤本。下次 issue 可能撞 CF API `81058 An identical record already exists` 要 retry。
- ⚠️ 要加 tier-2 wildcard（如 `*.foo.qmledmq.cn`）必须**所有 SAN 一起写在 issue 命令里**，缺一个就丢一个。

## 引用
- [caddy-cert-sync](caddy-cert-sync.md)
- [caddy](caddy.md)
- 操作步骤详：[home-runbook.md#常见运维-2](../../home-runbook.md)
- [refs/secrets-locations.md](../refs/secrets-locations.md)

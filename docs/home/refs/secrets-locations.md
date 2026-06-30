# secrets 位置（不存值，存指针）

> 文档**不存**任何明文 secret。只记「key 名 + 所在文件 + 怎么取」。长期方案见 [memory `project_secrets_cli_plan_2026_06_29`](#)（统一 SOPS+age）。

## CF API token (qmledmq.cn zone DNS edit)

- **用途**：acme.sh DNS-01 challenge / ddns-go IP 写入 / qm 之外的 manual CF API 操作
- **位置**：
  - WSL Ubuntu 24.04：`/root/.acme.sh/account.conf` → `CF_Token=<value>`
  - winhome：`C:\Caddy\ddns-go\config.yaml` → `DnsConf[0].DNS.Secret`
  - mac 项目内存：[memory `project_home_server_windows_handoff_2026_06_30`](#)（短期工程用）
- **scope**：Zone:DNS:Edit for qmledmq.cn
- **怎么轮换**：CF Dashboard → My Profile → API Tokens → rotate；改完三处都要换

## Cloudflare API token (NSSM Caddy 进程级 env)

- **用途**：Caddy 走 ACME DNS-01 时的 fallback（当前 caddy auto_https 已关，不实际使用）
- **位置**：NSSM service `Caddy` 的 `AppEnvironmentExtra` 含 `CLOUDFLARE_API_TOKEN=<value>`
- 看：`nssm get Caddy AppEnvironmentExtra`

## ssh key (mac → winhome)

- **用途**：`ssh winhome / winhome-pub`
- **位置**：
  - 私钥：mac `~/.ssh/id_ed25519`
  - 公钥已写：winhome `C:\ProgramData\ssh\administrators_authorized_keys`（一行）
- ACL：SYSTEM:F + Administrators:F

## GitHub PAT

- **用途**：gh CLI auth
- **位置**：
  - mac：gh 默认 keyring（macOS Keychain）
  - winhome：`%USERPROFILE%\.config\gh\hosts.yml`（可能 plain text，gh 默认）
  - WSL Ubuntu 24.04：`/root/.config/gh/hosts.yml`（明文，gh 警告了）

## GitLab PAT (g.ktvsky.com)

- **用途**：glab CLI / curl GitLab API（提 issue / comments）
- **位置**：`/Users/e0_7/Library/Application Support/glab-cli/config.yml`（mac）

## wanctl session token

- **用途**：mac MCP wanctl 工具调用
- **位置**：每个 MCP session 自己持有，relay 重启失效
- rebind 凭证：mac 内存里（每次 `wanctl_login` 返回，未持久化）

## fuxi 内部 secrets（暂列，详见 fuxi project doc）

- CC token / Codex token / FCM key / VAPID key / HMAC key / 飞书 app / 讯飞 app
- 当前散落，待 secrets CLI 整治

## 引用
- 长期方案：[memory `project_secrets_cli_plan_2026_06_29`](#) SOPS + age
- 别犯过的错：[memory `feedback_no_inline_secrets_in_docs`](#)（GitHub secret scanning 拦过 commit）

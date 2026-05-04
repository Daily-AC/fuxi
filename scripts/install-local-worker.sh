#!/usr/bin/env bash
# 把 *本地 macOS 机* 注册为 fuxi 的分布式 worker 节点。
#
# 一行运行（用户标准入口）：
#   bash <(curl -s https://im.qmledmq.cn:8443/setup-local-worker.sh)
#
# 参数：
#   $1 — 可选 node_id（默认 `<hostname-short>-local`，例 `mac-local`）
#
# 流程：
#   0  preflight：fuxi binary、jq、launchctl 都得在
#   1  交互问主密码
#   2  POST /api/dist/setup-worker → 拿 hmac_secret + dist_token + controller URL
#   3  写 ~/.fuxi/dist-worker.env（chmod 600）
#   4  装 ~/Library/LaunchAgents/com.fuxi.worker.plist
#   5  launchctl bootout 老 + bootstrap 新（幂等）
#   6  提示用户去 PWA 节点 tab 看是否在线
#
# 失败回滚：脚本任何一步 fail，已写的 env 文件 + plist 都清掉，避免半态卡住下次重跑。
#
# spec：docs/superpowers/specs/2026-04-27-im-dist-接通-design.md §"install-local-worker.sh 设计"

set -euo pipefail

# ── 常量 ────────────────────────────────────────────────────────────
HOME_URL="${FUXI_HOME_URL:-https://im.qmledmq.cn:8443}"
NODE_NAME="${1:-$(hostname -s | tr '[:upper:]' '[:lower:]')-local}"

ENV_FILE="${HOME}/.fuxi/dist-worker.env"
PLIST="${HOME}/Library/LaunchAgents/com.fuxi.worker.plist"
LABEL="com.fuxi.worker"
LAUNCHD_TARGET="gui/$(id -u)/${LABEL}"
LAUNCHD_DOMAIN="gui/$(id -u)"

# 失败回滚——任意 step exit !=0 触发，清掉部分写入避免半态。
# 注意：若用户 Ctrl-C 在 read 之前，ENV_FILE/PLIST 都还没建，rm -f 安全 noop。
ROLLBACK_DONE=0
rollback() {
    [[ "${ROLLBACK_DONE}" == "1" ]] && return
    ROLLBACK_DONE=1
    echo
    echo "[rollback] 清掉本次写入的 env 文件 + plist（避免半态）"
    rm -f "${ENV_FILE}" "${PLIST}" 2>/dev/null || true
    launchctl bootout "${LAUNCHD_TARGET}" 2>/dev/null || true
}
trap rollback ERR INT TERM

# ── 0. preflight ────────────────────────────────────────────────────
echo "── preflight ──"
echo "  HOME_URL : ${HOME_URL}"
echo "  NODE     : ${NODE_NAME}"
echo

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "  !! 本脚本仅支持 macOS（用 launchd）" >&2
    echo "     Linux 节点请等 Linux systemd 模板（spec § 拆活: 后期）" >&2
    exit 2
fi

# fuxi binary：优先 PATH；缺则从 GitHub Release 自动下载（P2.8 一键安装）
FUXI_BIN="$(command -v fuxi || true)"
if [[ -z "${FUXI_BIN}" ]]; then
    echo "  !! PATH 里没 fuxi binary，从 GitHub Release 自动下载…"
    case "$(uname -s)-$(uname -m)" in
        Darwin-arm64)  ASSET="fuxi-macos-aarch64" ;;
        Linux-x86_64)  ASSET="fuxi-linux-x86_64" ;;
        *)
            echo "  !! 不支持的平台 $(uname -s)-$(uname -m)" >&2
            echo "     已支持：Darwin-arm64 / Linux-x86_64" >&2
            echo "     需要其它平台请 cargo install --path /Users/e0_7/fuxi 自 build" >&2
            exit 2
            ;;
    esac
    URL="https://github.com/Daily-AC/fuxi/releases/latest/download/${ASSET}.tar.gz"
    SHA_URL="${URL}.sha256"
    TMP="$(mktemp -d)"
    trap 'rm -rf "$TMP"' EXIT
    echo "     下载 ${URL}"
    if ! curl -fsSL "$URL" -o "$TMP/fuxi.tar.gz"; then
        echo "  !! 下载失败（无 release 或网络问题）" >&2
        echo "     fallback：cargo install --path /Users/e0_7/fuxi" >&2
        exit 2
    fi
    # 校验 sha256（best-effort，sha 文件 404 时跳过）
    if curl -fsSL "$SHA_URL" -o "$TMP/sha" 2>/dev/null; then
        EXPECTED="$(cat "$TMP/sha" | tr -d '[:space:]')"
        if command -v sha256sum >/dev/null; then
            ACTUAL="$(sha256sum "$TMP/fuxi.tar.gz" | awk '{print $1}')"
        else
            ACTUAL="$(shasum -a 256 "$TMP/fuxi.tar.gz" | awk '{print $1}')"
        fi
        if [[ "$EXPECTED" != "$ACTUAL" ]]; then
            echo "  !! sha256 校验不过：expected=$EXPECTED actual=$ACTUAL" >&2
            exit 2
        fi
        echo "     sha256 校验通过：${ACTUAL:0:12}…"
    fi
    tar -xzf "$TMP/fuxi.tar.gz" -C "$TMP"
    mkdir -p "$HOME/.local/bin"
    install -m 0755 "$TMP/fuxi" "$HOME/.local/bin/fuxi"
    FUXI_BIN="$HOME/.local/bin/fuxi"
    echo "     已装到 $FUXI_BIN"
    # 自动 prepend PATH（不动 shell rc，本进程内能用）
    case ":$PATH:" in
        *":$HOME/.local/bin:"*) ;;
        *) export PATH="$HOME/.local/bin:$PATH" ;;
    esac
fi
echo "  fuxi     : ${FUXI_BIN}"

# claude (cc) binary：worker spawn cc 必须能找到。launchd 默认 PATH 极简，
# `~/.local/bin`（Anthropic claude 官方安装常见落点）不在里面。脚本里
# `command -v claude` 探到的真路径，写进 plist `--cc-bin <abs>` 显式指定，
# 同时把 `~/.local/bin` 前置到 plist PATH 双保险（cc 内部 spawn 子进程仍走 PATH）。
# 2026-04-27 用户实测踩过：3 个测试 job 全部 ok=false 1 秒内回报，因为 cc spawn 失败。
CLAUDE_BIN="$(command -v claude || true)"
if [[ -z "${CLAUDE_BIN}" ]]; then
    if [[ -x "${HOME}/.local/bin/claude" ]]; then
        CLAUDE_BIN="${HOME}/.local/bin/claude"
    else
        echo "  !! 找不到 claude（cc）binary——worker spawn cc 一定失败" >&2
        echo "     先装 claude：npm i -g @anthropic-ai/claude-code 或参考 docs.anthropic.com/claude-code/" >&2
        exit 2
    fi
fi
echo "  claude   : ${CLAUDE_BIN}"

# jq：解析 setup-worker JSON 返回
if ! command -v jq >/dev/null; then
    echo "  !! 缺 jq——macOS 装：brew install jq" >&2
    exit 2
fi

# launchctl：macOS 自带，但显式探一下
if ! command -v launchctl >/dev/null; then
    echo "  !! 缺 launchctl——macOS 系统应自带，环境异常" >&2
    exit 2
fi

# curl 探 home_url 通否（早失败：DNS / 公网 IP / 服务挂）
if ! curl -fsS --max-time 10 "${HOME_URL}/healthz" >/dev/null; then
    echo "  !! ${HOME_URL}/healthz 不通——home 上 fuxi-im 在跑吗？" >&2
    exit 2
fi
echo "  /healthz : ok"
echo

# ── 1. 问主密码 ──────────────────────────────────────────────────────
echo "── 主密码 ──"
echo "  这是用户在 home 上 \`fuxi im set-password\` 时设的那个密码。"
echo "  仅用于鉴权 → 服务器返 HMAC secret + dist token；密码本身不落本地。"
read -srp "  fuxi 主密码: " PASSWORD; echo

if [[ -z "${PASSWORD}" ]]; then
    echo "  !! 密码不能为空" >&2
    exit 2
fi

# ── 2. POST setup-worker → 拿 hmac_secret + dist_token ─────────────
echo
echo "── 注册 worker ──"
SETUP_BODY=$(jq -nc --arg pw "${PASSWORD}" --arg nid "${NODE_NAME}" \
    '{password: $pw, node_id: $nid}')

# 不要把 PASSWORD 暴露到 ps（用 -d @- + stdin 传 body）。--data-binary 不解析 @
SETUP_RESP=$(printf '%s' "${SETUP_BODY}" | curl -fsS --max-time 30 \
    -X POST "${HOME_URL}/api/dist/setup-worker" \
    -H "Content-Type: application/json" \
    --data-binary @-) || {
    echo "  !! POST /api/dist/setup-worker 失败" >&2
    echo "     检查：主密码是否正确？/api/dist/setup-worker 是否实装（β #56）？" >&2
    exit 2
}
unset PASSWORD SETUP_BODY  # 不再需要，立即清出 process env

HMAC_SECRET=$(echo "${SETUP_RESP}" | jq -r '.hmac_secret // empty')
DIST_TOKEN=$(echo "${SETUP_RESP}" | jq -r '.dist_token // empty')
CONTROLLER_URL=$(echo "${SETUP_RESP}" | jq -r '.controller_url // empty')

# 后端字段不全 = 接口契约破坏，硬退
if [[ -z "${HMAC_SECRET}" || -z "${DIST_TOKEN}" ]]; then
    echo "  !! setup-worker 响应缺字段（hmac_secret / dist_token）" >&2
    echo "     原始响应（脱敏前先截断）：${SETUP_RESP:0:200}" >&2
    exit 2
fi

# controller_url 后端可能不返（v1 简化），fallback 到 ${HOME_URL}（不含 /dist）
# worker 内部 dist.rs:1555 自己拼 `{controller}/dist/register`——这里再加 /dist
# 会得到 //dist/dist/register 双重路径。setup-worker 端点也走相同约定（β #67）。
if [[ -z "${CONTROLLER_URL}" ]]; then
    CONTROLLER_URL="${HOME_URL}"
fi
# 防御性：去掉用户/后端意外塞的尾部 / 或 /dist 后缀
CONTROLLER_URL="${CONTROLLER_URL%/}"
CONTROLLER_URL="${CONTROLLER_URL%/dist}"
echo "  注册 OK (node=${NODE_NAME}, controller=${CONTROLLER_URL})"

# ── 3. 写 env 文件 ──────────────────────────────────────────────────
echo
echo "── 写 ${ENV_FILE} ──"
mkdir -p "$(dirname "${ENV_FILE}")"
# 先写到 tmp 再 mv：避免 SIGINT 留下半截文件
ENV_TMP="${ENV_FILE}.tmp.$$"
cat >"${ENV_TMP}" <<EOF
# fuxi dist worker env，由 install-local-worker.sh 写入。
# 不要手动改——重跑脚本即可幂等覆盖。
FUXI_DIST_HMAC_SECRET=${HMAC_SECRET}
FUXI_DIST_TOKEN=${DIST_TOKEN}
FUXI_DIST_CONTROLLER=${CONTROLLER_URL}
FUXI_DIST_NODE_ID=${NODE_NAME}
EOF
chmod 600 "${ENV_TMP}"
mv -f "${ENV_TMP}" "${ENV_FILE}"
echo "  ✓ 0600 写入"

# ── 4. 装 launchd plist ────────────────────────────────────────────
echo
echo "── 装 ${PLIST} ──"
mkdir -p "$(dirname "${PLIST}")"
PLIST_TMP="${PLIST}.tmp.$$"

# Clash TUN 拦本机反连——CLAUDE.md 里强调的同坑。worker 起 cc 也得 NO_PROXY。
# 同时设 PATH，让 cc / claude / git / 其它 spawn 出来的子进程能找到工具链。
# (launchd 默认 PATH 极度精简，~/.cargo/bin 和 brew bin 都不在)
cat >"${PLIST_TMP}" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LABEL}</string>

  <key>ProgramArguments</key>
  <array>
    <string>${FUXI_BIN}</string>
    <string>dist</string>
    <string>worker</string>
    <string>--controller</string><string>${CONTROLLER_URL}</string>
    <string>--node</string><string>${NODE_NAME}</string>
    <string>--cc-bin</string><string>${CLAUDE_BIN}</string>
    <string>--tag</string><string>local</string>
    <string>--tag</string><string>mac</string>
    <string>--max-concurrency</string><string>2</string>
  </array>

  <key>EnvironmentVariables</key>
  <dict>
    <key>FUXI_DIST_HMAC_SECRET</key><string>${HMAC_SECRET}</string>
    <key>FUXI_DIST_TOKEN</key><string>${DIST_TOKEN}</string>
    <key>HOME</key><string>${HOME}</string>
    <key>PATH</key><string>${HOME}/.local/bin:${HOME}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    <key>NO_PROXY</key><string>127.0.0.1,localhost</string>
    <key>RUST_LOG</key><string>info,fuxi=info,fuxi_cli::dist=debug</string>
  </dict>

  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key><false/>
  </dict>
  <key>ThrottleInterval</key><integer>10</integer>

  <key>StandardOutPath</key><string>/tmp/fuxi-worker.log</string>
  <key>StandardErrorPath</key><string>/tmp/fuxi-worker.err.log</string>

  <key>WorkingDirectory</key><string>${HOME}</string>
</dict>
</plist>
EOF
chmod 644 "${PLIST_TMP}"
mv -f "${PLIST_TMP}" "${PLIST}"
echo "  ✓ 写入"

# ── 5. launchctl bootout 老 + bootstrap 新（幂等）─────────────────
echo
echo "── launchctl bootstrap ──"
# bootout 老的：第一次跑没老的 → exit 36/113/...，吞掉
launchctl bootout "${LAUNCHD_TARGET}" 2>/dev/null || true
# bootstrap 新的——非零 exit 触发 trap rollback
if ! launchctl bootstrap "${LAUNCHD_DOMAIN}" "${PLIST}"; then
    echo "  !! launchctl bootstrap 失败" >&2
    echo "     看日志：tail -50 /tmp/fuxi-worker.err.log" >&2
    exit 2
fi
# kickstart：触发立刻起一次（KeepAlive 也会，但显式更稳）
launchctl kickstart -k "${LAUNCHD_TARGET}" 2>/dev/null || true
echo "  ✓ ${LABEL} 已 bootstrap 到 ${LAUNCHD_DOMAIN}"

# 解除 trap——后面只有信息打印，失败也不该回滚
trap - ERR INT TERM

# ── 6. 收尾 ─────────────────────────────────────────────────────────
echo
echo "════════════════════════════════════════════════════════════════"
echo "  ✓ 节点 ${NODE_NAME} 已注册 + 自启"
echo
echo "  下一步："
echo "    1. 打开 PWA 节点 tab：${HOME_URL}/#/nodes"
echo "       应在 ~10s 内看到 ${NODE_NAME} 上线"
echo "    2. 玄女 tab 试 \"@${NODE_NAME} 跑 ls ~\" 派活到本机"
echo
echo "  日志："
echo "    tail -f /tmp/fuxi-worker.log     # stdout（INFO/DEBUG）"
echo "    tail -f /tmp/fuxi-worker.err.log # stderr（ERROR/WARN）"
echo
echo "  管理："
echo "    launchctl print ${LAUNCHD_TARGET}   # 查状态"
echo "    launchctl bootout ${LAUNCHD_TARGET} # 停 + 卸载"
echo "    rm ${ENV_FILE} ${PLIST}             # 彻底清"
echo
echo "  幂等：再次跑本脚本（带或不带 node_id）会覆盖上次注册——主密码"
echo "        会重新签新 HMAC secret + token，旧的失效。"
echo "════════════════════════════════════════════════════════════════"

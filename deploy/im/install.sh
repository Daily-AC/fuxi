#!/usr/bin/env bash
# fuxi-im 一键部署脚本
#
# 流程（dry-run 模式打印每步命令但不执行）：
# 1. 本机 cargo build --release -p fuxi-cli
# 2. 本机 pnpm/vite build PWA dist（若 dist 缺失或 --rebuild-web 强制重建）
# 3. scp fuxi 二进制到 home:/home/e0-7/.local/bin/fuxi
# 4. rsync PWA dist 到 home:/home/e0-7/.local/share/fuxi/im-web/
# 5. scp deploy/im/nginx.conf 到 home:/tmp，sudo cp 到 sites-enabled/im，nginx -t && reload
# 6. scp deploy/im/fuxi-im.service 到 home:/tmp，sudo cp 到 system unit dir，daemon-reload + enable --now
# 7. 验证 systemctl status fuxi-im + curl https://im.qmledmq.cn:8443/healthz
#
# 用法：
#   ./deploy/im/install.sh --dry-run     列命令
#   ./deploy/im/install.sh --apply       实跑
#   ./deploy/im/install.sh --apply --rebuild-web  强制重 build PWA
#
# ssh 别名：~/.ssh/config 里 Host home 应已配（端口 2222 / 用户 e0-7）。

set -euo pipefail

# ── 路径常量 ─────────────────────────────────────────────────────────
FUXI_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEPLOY_DIR="${FUXI_ROOT}/deploy/im"
NGINX_CONF="${DEPLOY_DIR}/nginx.conf"
SYSTEMD_UNIT="${DEPLOY_DIR}/fuxi-im.service"
LOCAL_BIN="${FUXI_ROOT}/target/release/fuxi"
LOCAL_WEB_DIST="${FUXI_ROOT}/crates/fuxi-im/web/dist"

REMOTE_HOST="home"
REMOTE_BIN="/home/e0-7/.local/bin/fuxi"
REMOTE_WEB="/home/e0-7/.local/share/fuxi/im-web"
REMOTE_NGINX_TARGET="/etc/nginx/sites-enabled/im"
REMOTE_SYSTEMD_TARGET="/etc/systemd/system/fuxi-im.service"
HEALTHZ_URL="https://im.qmledmq.cn:8443/healthz"
PWA_URL="https://im.qmledmq.cn:8443/"
# 远端 build：源码同步到 home，由 home 的 cargo 编出 x86_64-linux 二进制
# （本地 macOS 编出来的 Mach-O 在 Linux 上 status=203/EXEC，2026-04-26 踩过）
REMOTE_SRC_DIR="/home/e0-7/fuxi"
REMOTE_BUILT_BIN="${REMOTE_SRC_DIR}/target/release/fuxi"
REMOTE_CARGO="/home/e0-7/.cargo/bin/cargo"

# ── 模式 ────────────────────────────────────────────────────────────
MODE=""
REBUILD_WEB="0"

usage() {
    cat <<USAGE >&2
用法：$0 (--dry-run|--apply) [--rebuild-web]

  --dry-run      打印每步将执行的命令但不执行
  --apply        真实执行
  --rebuild-web  即使 dist/ 已存在也强制 vite build
USAGE
    exit 2
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) MODE="dry"; shift ;;
        --apply)   MODE="apply"; shift ;;
        --rebuild-web) REBUILD_WEB="1"; shift ;;
        -h|--help) usage ;;
        *) echo "未知参数: $1" >&2; usage ;;
    esac
done
[[ -z "${MODE}" ]] && usage

# ── 执行包装 ────────────────────────────────────────────────────────
# `run` 模式分流：dry 只打印，apply 真跑（失败即 exit）。
# 注意：在 dry 模式下也仍然做"不可逆且无副作用"的本地预检（文件存在、ssh
# 别名能 resolve），让 dry 能尽早暴露环境问题。
run() {
    if [[ "${MODE}" == "dry" ]]; then
        printf '  $ %s\n' "$*"
    else
        printf '  $ %s\n' "$*"
        # shellcheck disable=SC2294
        eval "$@"
    fi
}

step() { echo; echo "── $* ──"; }

# ── 0. 预检（apply / dry 都跑）────────────────────────────────────
step "0. 预检"
echo "  fuxi root  : ${FUXI_ROOT}"
echo "  deploy dir : ${DEPLOY_DIR}"
echo "  ssh target : ${REMOTE_HOST}"

if ! ssh -o ConnectTimeout=15 "${REMOTE_HOST}" "echo ok" >/dev/null 2>&1; then
    echo "  !! ssh ${REMOTE_HOST} 不通——检查 ~/.ssh/config Host home 配置 / 公网 IP DDNS" >&2
    if [[ "${MODE}" == "apply" ]]; then exit 1; fi
fi
[[ -f "${NGINX_CONF}" ]]    || { echo "  !! 缺 ${NGINX_CONF}" >&2; exit 1; }
[[ -f "${SYSTEMD_UNIT}" ]]  || { echo "  !! 缺 ${SYSTEMD_UNIT}" >&2; exit 1; }

# ── 0.5 远端环境 fail-fast 检查（apply 模式才真验，dry 只列）──────
# 四条 preflight（team-lead 要求；d 是 26 号事故后补盲点）：
#   a) 无残留 fuxi 进程（避免 sock_path 撞）
#   b) 9100 端口空（避免 axum bind 失败）
#   c) /etc/nginx/sites-enabled/im 不存在（避免误覆盖）
#   d) 全 nginx 配置无人占 im.qmledmq.cn（c 只查 sites-enabled，conf.d/ 是盲点）
# 任何一条失败即停——继续跑会半途崩，难收拾。
if [[ "${MODE}" == "apply" ]]; then
    echo
    echo "  preflight a) 远端无残留 fuxi 进程："
    # `pgrep -fa fuxi` 会把本条 ssh 命令自身也匹配进去（它的命令行含 "fuxi"
    # 字符）——必须 grep -v 掉自身和 grep 自己，再过滤剩下的；剩 0 行才算"无残留"。
    pgrep_out=$(ssh "${REMOTE_HOST}" "pgrep -fa fuxi | grep -Ev 'pgrep|grep ' || true" 2>&1)
    if [[ -n "${pgrep_out}" ]]; then
        echo "    ${pgrep_out}"
        echo "  !! 远端有 fuxi 进程在跑——先 ssh home 'pkill fuxi' 或确认是否能停" >&2
        exit 1
    fi
    echo "    no fuxi running"

    echo "  preflight b) 9100 端口空闲："
    port_out=$(ssh "${REMOTE_HOST}" 'ss -tln 2>/dev/null | grep :9100 || echo "9100 free"' 2>&1)
    echo "    ${port_out}"
    if [[ "${port_out}" != "9100 free" ]]; then
        echo "  !! 9100 已被占用——查谁在用：ssh home 'ss -tlnp | grep :9100'" >&2
        exit 1
    fi

    echo "  preflight c) nginx vhost 文件不冲突："
    nginx_out=$(ssh "${REMOTE_HOST}" 'test -e /etc/nginx/sites-enabled/im && echo CONFLICT || echo OK' 2>&1)
    echo "    ${nginx_out}"
    if [[ "${nginx_out}" != "OK" ]]; then
        echo "  !! /etc/nginx/sites-enabled/im 已存在——人工确认是否覆盖再继续" >&2
        exit 1
    fi

    # team-lead 加的 preflight d——上次踩过的盲点：仅查 sites-enabled 不够，
    # /etc/nginx/conf.d/*.conf 也会被 nginx 主 conf 兜进去。任何地方有人占
    # `im.qmledmq.cn` server_name 都会导致 conflicting server name + 我的
    # vhost 被 nginx 静默忽略。
    echo "  preflight d) 全 nginx 配置无人占 im.qmledmq.cn："
    grep_out=$(ssh "${REMOTE_HOST}" 'sudo grep -rln "im.qmledmq.cn" /etc/nginx/ 2>/dev/null || true' 2>&1)
    if [[ -n "${grep_out}" ]]; then
        echo "    ${grep_out}"
        echo "  !! 上述文件已占 im.qmledmq.cn——清掉再继续" >&2
        exit 1
    fi
    echo "    (无)"
else
    echo
    echo "  (dry-run 模式) 实跑前会做四条 preflight："
    echo "    $ ssh ${REMOTE_HOST} 'pgrep -fa fuxi || echo \"no fuxi running\"'"
    echo "    $ ssh ${REMOTE_HOST} 'ss -tln 2>/dev/null | grep :9100 || echo \"9100 free\"'"
    echo "    $ ssh ${REMOTE_HOST} 'test -e /etc/nginx/sites-enabled/im && echo CONFLICT || echo OK'"
    echo "    $ ssh ${REMOTE_HOST} 'sudo grep -rln \"im.qmledmq.cn\" /etc/nginx/'"
fi

# ── 1. 远端 cargo build release ─────────────────────────────────────
# 为何不本地 build：home 是 x86_64-linux，本机 macOS 编出来的 Mach-O 不能跑
# （systemd status=203/EXEC）。最简方案：rsync 源码到 home，远端 cargo build。
# 增量 build 缓存复用 home 的 ~/.cargo + target/，重跑很快。
step "1. rsync 源码 + 远端 cargo build --release"
# 同步源码：排除 target/、本地 web/node_modules、git 大对象、IDE 文件——
# 只送编译需要的。--delete 让删除的文件远端也清掉，但留下 target/ 让缓存复用。
run "rsync -az --delete \
    --exclude '/target' \
    --exclude '/.git' \
    --exclude 'node_modules' \
    --exclude '/crates/fuxi-im/web/test-results' \
    --exclude '/crates/fuxi-im/web/playwright-report' \
    --exclude '*.log' \
    --exclude '/.claude' \
    ${FUXI_ROOT}/ ${REMOTE_HOST}:${REMOTE_SRC_DIR}/"
run "ssh ${REMOTE_HOST} 'cd ${REMOTE_SRC_DIR} && ${REMOTE_CARGO} build --release -p fuxi-cli'"

# ── 2. PWA dist build（按需）────────────────────────────────────────
step "2. PWA dist 检查 / build"
if [[ "${REBUILD_WEB}" == "1" || ! -d "${LOCAL_WEB_DIST}" || -z "$(ls -A "${LOCAL_WEB_DIST}" 2>/dev/null)" ]]; then
    run "cd ${FUXI_ROOT}/crates/fuxi-im/web && pnpm install --frozen-lockfile"
    run "cd ${FUXI_ROOT}/crates/fuxi-im/web && pnpm run build"
else
    echo "  ${LOCAL_WEB_DIST} 已存在且非空——跳过 vite build（用 --rebuild-web 强制）"
fi

# ── 3. 安装 fuxi binary（远端 cp 而非 scp）──────────────────────────
step "3. 远端 cp ${REMOTE_BUILT_BIN} → ${REMOTE_BIN}"
run "ssh ${REMOTE_HOST} 'mkdir -p $(dirname ${REMOTE_BIN})'"
run "ssh ${REMOTE_HOST} 'cp ${REMOTE_BUILT_BIN} ${REMOTE_BIN} && chmod 755 ${REMOTE_BIN}'"
# 跑一次 --version 兜底验证：架构对 + 没炸
run "ssh ${REMOTE_HOST} '${REMOTE_BIN} --version'"

# ── 4. 推送 PWA dist ────────────────────────────────────────────────
step "4. rsync PWA dist → ${REMOTE_HOST}:${REMOTE_WEB}/"
run "ssh ${REMOTE_HOST} 'mkdir -p ${REMOTE_WEB}'"
# rsync --delete 让远端和本地一致：旧 hash 资产清掉避免 sw.js 引用过期文件
run "rsync -az --delete ${LOCAL_WEB_DIST}/ ${REMOTE_HOST}:${REMOTE_WEB}/"

# ── 5. nginx vhost ──────────────────────────────────────────────────
step "5. nginx vhost"
run "scp ${NGINX_CONF} ${REMOTE_HOST}:/tmp/fuxi-im.nginx.conf"
run "ssh ${REMOTE_HOST} 'sudo cp /tmp/fuxi-im.nginx.conf ${REMOTE_NGINX_TARGET}'"
run "ssh ${REMOTE_HOST} 'sudo nginx -t'"
run "ssh ${REMOTE_HOST} 'sudo systemctl reload nginx'"

# ── 6. systemd unit ─────────────────────────────────────────────────
step "6. systemd unit"
run "scp ${SYSTEMD_UNIT} ${REMOTE_HOST}:/tmp/fuxi-im.service"
run "ssh ${REMOTE_HOST} 'sudo cp /tmp/fuxi-im.service ${REMOTE_SYSTEMD_TARGET}'"
run "ssh ${REMOTE_HOST} 'sudo systemctl daemon-reload'"
run "ssh ${REMOTE_HOST} 'sudo systemctl enable --now fuxi-im'"

# ── 7. 验证 ─────────────────────────────────────────────────────────
step "7. 验证"
run "ssh ${REMOTE_HOST} 'systemctl status fuxi-im --no-pager | head -20'"
run "curl -fsS --max-time 10 ${HEALTHZ_URL}"
echo
echo "  最终验证（脚本不自动跑，需人工）："
echo "    手机 Safari 打开: ${PWA_URL}"
echo "    TUI 跑 /pair 拿 PIN → 手机配对 → 看到任务列表"

step "完成"
case "${MODE}" in
    dry)   echo "DRY-RUN 完成——以上命令未执行。SendMessage team-lead 审核后再跑 --apply。" ;;
    apply) echo "APPLY 完成。systemd 应已 active；curl ${HEALTHZ_URL} 应 200。" ;;
esac

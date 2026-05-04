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

# SSH/SCP/RSYNC 抗 ProxyCommand 抖断（2026-04-27 多次踩坑迭代）：
# 1. b18c8ed 部署期间 cargo build 56s 静默 → 链路掉 → 加 ServerAliveInterval=30
#    + CountMax=3（90s 容忍窗口）
# 2. 进一步发现首发 ssh 也常抖：~/.ssh/config 的 Host home 用
#    `ProxyCommand bash -c 'nc $(curl ... cloudflare-dns.com ...) %p'`，
#    每个新 ssh 进程都跑一次 curl 解 DNS 拼 nc，公网链路偶 timeout，
#    错误是 `Connection closed by UNKNOWN port 65535`（nc 端，ssh handshake
#    都没开始，keepalive 救不了）。修法：ControlMaster 复用单条物理连接，
#    后续 ssh / scp / rsync 全走那条已建好的 socket，跳过 ProxyCommand
#    重新解析。脚本退出时 trap 关 master。
SSH_CM_DIR="${HOME}/.ssh/cm-fuxi-deploy"
SSH_CM_PATH="${SSH_CM_DIR}/home.sock"
mkdir -p "${SSH_CM_DIR}"
SSH_OPTS="-o ServerAliveInterval=30 -o ServerAliveCountMax=3 -o ControlMaster=auto -o ControlPath=${SSH_CM_PATH} -o ControlPersist=300 -o ConnectTimeout=15 -o ConnectionAttempts=3"
# rsync 用的 ssh 走 -e 'ssh ...'——同款选项
RSYNC_RSH="ssh ${SSH_OPTS}"

# 脚本退出时把 ControlMaster 关掉，避免脚本退后还有遗留 ssh 进程
cleanup_ssh_master() {
    if [[ -S "${SSH_CM_PATH}" ]]; then
        ssh -O exit -o ControlPath="${SSH_CM_PATH}" "${REMOTE_HOST}" 2>/dev/null || true
    fi
}
trap cleanup_ssh_master EXIT

# ── 模式 ────────────────────────────────────────────────────────────
MODE=""
REBUILD_WEB="0"
WEB_ONLY="0"
AUTO_TAKEOVER="0"

usage() {
    cat <<USAGE >&2
用法：$0 (--dry-run|--apply) [--rebuild-web] [--web-only] [--auto-takeover]

  --dry-run        打印每步将执行的命令但不执行
  --apply          真实执行
  --rebuild-web    即使 dist/ 已存在也强制 vite build
  --web-only       仅 PWA 快速部署（rust 不动 / systemd 不重启 / nginx 不改 vhost）。
                   路径：vite build → rsync dist → nginx reload；适合 ε 阶段性
                   PWA 迭代。预期 ~30s。隐含 --rebuild-web。
  --auto-takeover  preflight 失败时自动接管而非 abort：
                     a) 残留 fuxi 进程 → systemctl stop fuxi-im
                     c) 已有 sites-enabled/im → cp .bak.<ts> 后覆盖
                     d) 仅当占用 server_name 的是我们自己的 sites-enabled/im 时通过
                   首次部署到全新机器、或被自己半截部署残留卡住时用。
                   对"非我们的"端口占用 / 第三方 server_name 冲突仍然 abort——
                   那是真冲突，得人工。
USAGE
    exit 2
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) MODE="dry"; shift ;;
        --apply)   MODE="apply"; shift ;;
        --rebuild-web) REBUILD_WEB="1"; shift ;;
        --web-only) WEB_ONLY="1"; REBUILD_WEB="1"; shift ;;
        --auto-takeover) AUTO_TAKEOVER="1"; shift ;;
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

if ! ssh ${SSH_OPTS} -o ConnectTimeout=15 "${REMOTE_HOST}" "echo ok" >/dev/null 2>&1; then
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
#
# --web-only 模式跳过 a/b/c：service 在跑是预期（不重启），nginx vhost 已在
# 也是预期（不改 vhost）。只保留 d 的"server_name 不被别处占用"作为最低 sanity。
if [[ "${WEB_ONLY}" == "1" ]]; then
    echo
    echo "  (--web-only) 跳过 preflight a/b/c（service 在跑、9100 占用、vhost 已在 都是预期）"
    if [[ "${MODE}" == "apply" ]]; then
        echo "  preflight d) 全 nginx 配置无人占 im.qmledmq.cn（兜底防被劫持）："
        grep_out=$(ssh ${SSH_OPTS} "${REMOTE_HOST}" 'sudo grep -rln "im.qmledmq.cn" /etc/nginx/ 2>/dev/null || true' 2>&1)
        # web-only 模式期望 grep 命中**只**有 sites-enabled/im（我们自己装的），其它都是异常
        unexpected=$(echo "${grep_out}" | grep -v "/etc/nginx/sites-enabled/im$" || true)
        if [[ -n "${unexpected}" ]]; then
            echo "    ${grep_out}"
            echo "  !! 除 sites-enabled/im 外还有人占 im.qmledmq.cn——清掉再继续" >&2
            exit 1
        fi
        echo "    (仅 sites-enabled/im 自己)"
    fi
elif [[ "${MODE}" == "apply" ]]; then
    echo
    echo "  preflight a) 远端无残留 fuxi 进程："
    # `pgrep -fa fuxi` 会把本条 ssh 命令自身也匹配进去（它的命令行含 "fuxi"
    # 字符）——必须 grep -v 掉自身和 grep 自己，再过滤剩下的；剩 0 行才算"无残留"。
    pgrep_out=$(ssh ${SSH_OPTS} "${REMOTE_HOST}" "pgrep -fa fuxi | grep -Ev 'pgrep|grep ' || true" 2>&1)
    if [[ -n "${pgrep_out}" ]]; then
        echo "    ${pgrep_out}"
        if [[ "${AUTO_TAKEOVER}" == "1" ]]; then
            echo "  (--auto-takeover) 自动 systemctl stop fuxi-im 并 pkill 残留"
            ssh ${SSH_OPTS} "${REMOTE_HOST}" 'sudo systemctl stop fuxi-im 2>/dev/null || true; pkill -f "fuxi " 2>/dev/null || true; sleep 1' || true
            # 复检
            pgrep_out2=$(ssh ${SSH_OPTS} "${REMOTE_HOST}" "pgrep -fa fuxi | grep -Ev 'pgrep|grep ' || true" 2>&1)
            if [[ -n "${pgrep_out2}" ]]; then
                echo "    ${pgrep_out2}"
                echo "  !! takeover 后仍有 fuxi 进程残留——人工排查" >&2
                exit 1
            fi
            echo "    takeover ok, no fuxi running"
        else
            echo "  !! 远端有 fuxi 进程在跑——先 ssh home 'pkill fuxi' 或加 --auto-takeover" >&2
            exit 1
        fi
    else
        echo "    no fuxi running"
    fi

    echo "  preflight b) 9100 端口空闲："
    port_out=$(ssh ${SSH_OPTS} "${REMOTE_HOST}" 'ss -tln 2>/dev/null | grep :9100 || echo "9100 free"' 2>&1)
    echo "    ${port_out}"
    if [[ "${port_out}" != "9100 free" ]]; then
        # takeover 不接管 9100：如果 a 已 stop 了 fuxi-im 还有人在 9100，那是别人占的，必须人工
        echo "  !! 9100 已被占用（auto-takeover 不接管端口冲突，可能是别人）——查：ssh home 'ss -tlnp | grep :9100'" >&2
        exit 1
    fi

    echo "  preflight c) nginx vhost 文件不冲突："
    nginx_out=$(ssh ${SSH_OPTS} "${REMOTE_HOST}" 'test -e /etc/nginx/sites-enabled/im && echo CONFLICT || echo OK' 2>&1)
    echo "    ${nginx_out}"
    if [[ "${nginx_out}" != "OK" ]]; then
        if [[ "${AUTO_TAKEOVER}" == "1" ]]; then
            ts=$(date +%Y%m%d-%H%M%S)
            echo "  (--auto-takeover) 备份现有 vhost 到 /etc/nginx/sites-enabled/im.bak.${ts}"
            ssh ${SSH_OPTS} "${REMOTE_HOST}" "sudo cp /etc/nginx/sites-enabled/im /etc/nginx/sites-enabled/im.bak.${ts}" || {
                echo "  !! 备份失败" >&2; exit 1; }
        else
            echo "  !! /etc/nginx/sites-enabled/im 已存在——人工确认或加 --auto-takeover" >&2
            exit 1
        fi
    fi

    # team-lead 加的 preflight d——上次踩过的盲点：仅查 sites-enabled 不够，
    # /etc/nginx/conf.d/*.conf 也会被 nginx 主 conf 兜进去。任何地方有人占
    # `im.qmledmq.cn` server_name 都会导致 conflicting server name + 我的
    # vhost 被 nginx 静默忽略。
    echo "  preflight d) 全 nginx 配置无人占 im.qmledmq.cn："
    grep_out=$(ssh ${SSH_OPTS} "${REMOTE_HOST}" 'sudo grep -rln "im.qmledmq.cn" /etc/nginx/ 2>/dev/null || true' 2>&1)
    if [[ -n "${grep_out}" ]]; then
        if [[ "${AUTO_TAKEOVER}" == "1" ]]; then
            # takeover 模式：仅当命中的全是 sites-enabled/im* 自家文件才放行（c 已备份）
            unexpected=$(echo "${grep_out}" | grep -Ev '/etc/nginx/sites-enabled/im(\.bak\..*)?$' || true)
            if [[ -n "${unexpected}" ]]; then
                echo "    ${grep_out}"
                echo "  !! 除自家 sites-enabled/im* 外还有人占 im.qmledmq.cn——auto-takeover 不接管第三方，清掉再继续" >&2
                exit 1
            fi
            echo "    (--auto-takeover) 占用全是自家 sites-enabled/im*，放行"
        else
            echo "    ${grep_out}"
            echo "  !! 上述文件已占 im.qmledmq.cn——清掉再继续或加 --auto-takeover" >&2
            exit 1
        fi
    else
        echo "    (无)"
    fi
else
    echo
    echo "  (dry-run 模式) 实跑前会做四条 preflight："
    echo "    $ ssh ${REMOTE_HOST} 'pgrep -fa fuxi || echo \"no fuxi running\"'"
    echo "    $ ssh ${REMOTE_HOST} 'ss -tln 2>/dev/null | grep :9100 || echo \"9100 free\"'"
    echo "    $ ssh ${REMOTE_HOST} 'test -e /etc/nginx/sites-enabled/im && echo CONFLICT || echo OK'"
    echo "    $ ssh ${REMOTE_HOST} 'sudo grep -rln \"im.qmledmq.cn\" /etc/nginx/'"
    if [[ "${AUTO_TAKEOVER}" == "1" ]]; then
        echo
        echo "  (--auto-takeover) 命中 a/c/d 时会自动接管：stop fuxi-im、备份 vhost、放行自家 server_name；"
        echo "                   端口冲突 (b) + 第三方 server_name 占用 仍 abort。"
    fi
fi

# ── 1. 远端 cargo build release ─────────────────────────────────────
# 为何不本地 build：home 是 x86_64-linux，本机 macOS 编出来的 Mach-O 不能跑
# （systemd status=203/EXEC）。最简方案：rsync 源码到 home，远端 cargo build。
# 增量 build 缓存复用 home 的 ~/.cargo + target/，重跑很快。
if [[ "${WEB_ONLY}" == "1" ]]; then
    step "1. (--web-only) 跳过 rsync 源码 + cargo build"
else
    step "1. rsync 源码 + 远端 cargo build --release"
    # 同步源码：排除 target/、本地 web/node_modules、git 大对象、IDE 文件——
    # 只送编译需要的。--delete 让删除的文件远端也清掉，但留下 target/ 让缓存复用。
    # `-c` checksum 比对而不是 mtime+size 快查——为防 mtime collision（曾踩：
    # 本地新版 fuxi.rs 与 home 旧版 mtime 完全相同，size 不同，rsync quick check
    # 跳过，cargo build 用旧代码爆 E0599）。源码树小，CPU 代价可忽略。
    run "rsync -azc --delete -e '${RSYNC_RSH}' \
        --exclude '/target' \
        --exclude '/.git' \
        --exclude 'node_modules' \
        --exclude '/crates/fuxi-im/web/test-results' \
        --exclude '/crates/fuxi-im/web/playwright-report' \
        --exclude '*.log' \
        --exclude '/.claude' \
        ${FUXI_ROOT}/ ${REMOTE_HOST}:${REMOTE_SRC_DIR}/"
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'cd ${REMOTE_SRC_DIR} && ${REMOTE_CARGO} build --release -p fuxi-cli'"
fi

# ── 2. PWA dist build（按需）────────────────────────────────────────
step "2. PWA dist 检查 / build"
if [[ "${REBUILD_WEB}" == "1" || ! -d "${LOCAL_WEB_DIST}" || -z "$(ls -A "${LOCAL_WEB_DIST}" 2>/dev/null)" ]]; then
    run "cd ${FUXI_ROOT}/crates/fuxi-im/web && pnpm install --frozen-lockfile"
    run "cd ${FUXI_ROOT}/crates/fuxi-im/web && pnpm run build"
else
    echo "  ${LOCAL_WEB_DIST} 已存在且非空——跳过 vite build（用 --rebuild-web 强制）"
fi

# ── 3. 安装 fuxi binary（远端 cp 而非 scp）──────────────────────────
if [[ "${WEB_ONLY}" == "1" ]]; then
    step "3. (--web-only) 跳过 binary 安装"
else
    step "3. 远端 cp ${REMOTE_BUILT_BIN} → ${REMOTE_BIN}"
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'mkdir -p $(dirname ${REMOTE_BIN})'"
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'cp ${REMOTE_BUILT_BIN} ${REMOTE_BIN} && chmod 755 ${REMOTE_BIN}'"
    # 跑一次 --version 兜底验证：架构对 + 没炸
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} '${REMOTE_BIN} --version'"
fi

# ── 4. 推送 PWA dist ────────────────────────────────────────────────
step "4. rsync PWA dist → ${REMOTE_HOST}:${REMOTE_WEB}/"
run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'mkdir -p ${REMOTE_WEB}'"
# rsync --delete 让远端和本地一致：旧 hash 资产清掉避免 sw.js 引用过期文件
# `-c` checksum 同 step 1 理由——dist 一般 hash 命名（content-addressed），但
# index.html / manifest.webmanifest / sw.js 不是 hash 命名，可能与旧版 mtime 一致 size 一致
# 而内容微改（version field/timestamp 等），mtime 快查会跳过。
run "rsync -azc --delete -e '${RSYNC_RSH}' ${LOCAL_WEB_DIST}/ ${REMOTE_HOST}:${REMOTE_WEB}/"

# ── 5. nginx vhost ──────────────────────────────────────────────────
if [[ "${WEB_ONLY}" == "1" ]]; then
    step "5. (--web-only) 仅 nginx reload（不重写 vhost）"
    # vhost 内容不改，但 reload 让 nginx 把新 dist 的文件描述符重 open——保险
    # 起见走一次（cost 几十毫秒，避免 cache 行为偶发）。
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'sudo systemctl reload nginx'"
else
    step "5. nginx vhost"
    run "scp ${SSH_OPTS} ${NGINX_CONF} ${REMOTE_HOST}:/tmp/fuxi-im.nginx.conf"
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'sudo cp /tmp/fuxi-im.nginx.conf ${REMOTE_NGINX_TARGET}'"
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'sudo nginx -t'"
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'sudo systemctl reload nginx'"
fi

# ── 6. systemd unit ─────────────────────────────────────────────────
if [[ "${WEB_ONLY}" == "1" ]]; then
    step "6. (--web-only) 跳过 systemd unit 安装 + restart"
else
    step "6. systemd unit"
    run "scp ${SSH_OPTS} ${SYSTEMD_UNIT} ${REMOTE_HOST}:/tmp/fuxi-im.service"
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'sudo cp /tmp/fuxi-im.service ${REMOTE_SYSTEMD_TARGET}'"
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'sudo systemctl daemon-reload'"
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'sudo systemctl enable --now fuxi-im'"
fi

# ── 7. 验证 ─────────────────────────────────────────────────────────
step "7. 验证"
if [[ "${WEB_ONLY}" == "1" ]]; then
    # web-only：service 没重启，只验"还活着"+ "PWA 已换新 dist"（看 index.html 的 asset hash）
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'systemctl is-active fuxi-im'"
    run "curl -fsS --max-time 10 ${HEALTHZ_URL}"
    # grep index-*.js 提示新 hash——team-lead 比对 vite build 输出确认
    run "curl -fsS --max-time 10 ${PWA_URL} | grep -oE 'index-[A-Za-z0-9_-]+\\.js' | head -1"
else
    run "ssh ${SSH_OPTS} ${REMOTE_HOST} 'systemctl status fuxi-im --no-pager | head -20'"
    run "curl -fsS --max-time 10 ${HEALTHZ_URL}"
fi
echo
echo "  最终验证（脚本不自动跑，需人工）："
echo "    手机 Safari 打开: ${PWA_URL}"
if [[ "${WEB_ONLY}" == "1" ]]; then
    echo "    PWA 应该刷新即生效；service worker 自动拉新 sw.js"
else
    echo "    TUI 跑 /pair 拿 PIN → 手机配对 → 看到任务列表"
fi

step "完成"
case "${MODE}" in
    dry)   echo "DRY-RUN 完成——以上命令未执行。SendMessage team-lead 审核后再跑 --apply。" ;;
    apply) echo "APPLY 完成。systemd 应已 active；curl ${HEALTHZ_URL} 应 200。" ;;
esac

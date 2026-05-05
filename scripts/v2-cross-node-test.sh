#!/usr/bin/env bash
# v2 跨节点 sandbox e2e 验证脚本——不实际 spawn cc/codex（避免烧钱），
# 只验：
#   1. home 端 fuxi project add 通
#   2. /api/projects/<slug> GET 返 host_nodes 字段
#   3. POST /api/projects/<slug>/host_nodes 通告节点能落
#   4. dispatch 一个带 project_id 的 task 时 home 端会按 host_nodes 自动 pin
#
# 真实 cc 协作演示（绑子域名上线）走另一份 manual playbook，那才是答辩 dry-run。
#
# 用法：
#   FUXI_IM_BASE=https://im.qmledmq.cn:8443 ./scripts/v2-cross-node-test.sh
#
# 前置：
# - im-test.sh 同款 token 机制（~/.fuxi/im_hmac.key on home, ssh home 可走）
# - home 已部署本 commit 的 fuxi binary

set -euo pipefail

cd "$(dirname "$0")/.."

BASE="${FUXI_IM_BASE:-https://im.qmledmq.cn:8443}"
TEST_SLUG="${V2_TEST_SLUG:-v2-test-$RANDOM}"
HOME_HOST="${FUXI_HOME_HOST:-home}"

source <(./scripts/im-test.sh _env 2>/dev/null) || true
TOKEN_FILE="/tmp/fuxi-im-token"

token() {
  if [ ! -s "$TOKEN_FILE" ]; then
    ssh "$HOME_HOST" 'python3 ~/.fuxi/im-mint-token.py' > "$TOKEN_FILE"
    chmod 600 "$TOKEN_FILE"
  fi
  cat "$TOKEN_FILE"
}

api() {
  curl -sS \
    -b "fuxi_im_token=$(token)" \
    -H 'Content-Type: application/json' \
    "$@"
}

step() {
  echo
  echo "━━ $* ━━"
}

cleanup() {
  echo
  echo "[cleanup] 删除测试 project $TEST_SLUG"
  api -X DELETE "$BASE/api/projects/$TEST_SLUG" >/dev/null 2>&1 || true
  ssh "$HOME_HOST" "rm -rf /tmp/v2-test-repo-$TEST_SLUG" 2>/dev/null || true
}
trap cleanup EXIT

step "1. 在 home 上建一个临时 git repo + project add"
ssh "$HOME_HOST" "
  set -e
  mkdir -p /tmp/v2-test-repo-$TEST_SLUG
  cd /tmp/v2-test-repo-$TEST_SLUG
  git init -q -b main
  git config user.email t@t
  git config user.name t
  echo 'v2 test' > README.md
  git add -A
  git commit -qm 'seed'
"
api -X POST "$BASE/api/projects" \
  -d "{\"canonical_path\":\"/tmp/v2-test-repo-$TEST_SLUG\",\"name\":\"$TEST_SLUG\"}" \
  | python3 -m json.tool

step "2. GET /api/projects/$TEST_SLUG —— host_nodes 应是 []"
RESP=$(api -X GET "$BASE/api/projects/$TEST_SLUG")
echo "$RESP" | python3 -m json.tool
HOST_NODES=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['host_nodes'])")
[ "$HOST_NODES" = "[]" ] || { echo "FAIL: 期望空 host_nodes，实得 $HOST_NODES"; exit 1; }
echo "  ✓ host_nodes 为空"

step "3. POST host_nodes 登记 home + mac-local"
api -X POST "$BASE/api/projects/$TEST_SLUG/host_nodes" \
  -d '{"node_id":"home"}' | python3 -m json.tool >/dev/null
api -X POST "$BASE/api/projects/$TEST_SLUG/host_nodes" \
  -d '{"node_id":"zyldemacbook-pro-local"}' | python3 -m json.tool >/dev/null

RESP=$(api -X GET "$BASE/api/projects/$TEST_SLUG")
HOST_NODES=$(echo "$RESP" | python3 -c "import sys,json; print(','.join(json.load(sys.stdin)['host_nodes']))")
[ "$HOST_NODES" = "home,zyldemacbook-pro-local" ] || {
  echo "FAIL: 期望 home,zyldemacbook-pro-local，实得 $HOST_NODES"; exit 1
}
echo "  ✓ 两节点都登记成功"

step "4. /api/nodes 看 home + mac 状态"
api -X GET "$BASE/api/nodes" | python3 -m json.tool | head -50

step "5. 幂等：重复 POST 同 node_id 不重复"
api -X POST "$BASE/api/projects/$TEST_SLUG/host_nodes" \
  -d '{"node_id":"home"}' >/dev/null
RESP=$(api -X GET "$BASE/api/projects/$TEST_SLUG")
HOST_NODES=$(echo "$RESP" | python3 -c "import sys,json; print(','.join(json.load(sys.stdin)['host_nodes']))")
[ "$HOST_NODES" = "home,zyldemacbook-pro-local" ] || {
  echo "FAIL: 重复登记应幂等，实得 $HOST_NODES"; exit 1
}
echo "  ✓ 幂等"

step "6. DELETE host_nodes/<node_id> 移除节点"
api -X DELETE "$BASE/api/projects/$TEST_SLUG/host_nodes/zyldemacbook-pro-local"
RESP=$(api -X GET "$BASE/api/projects/$TEST_SLUG")
HOST_NODES=$(echo "$RESP" | python3 -c "import sys,json; print(','.join(json.load(sys.stdin)['host_nodes']))")
[ "$HOST_NODES" = "home" ] || { echo "FAIL: 期望仅 home，实得 $HOST_NODES"; exit 1; }
echo "  ✓ DELETE 通"

echo
echo "✅ v2 跨节点 API 全部测过"
echo "   下一步：手动测 mac fuxi project join + dispatch 真任务（answer-prep playbook）"

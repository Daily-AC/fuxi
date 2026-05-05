#!/usr/bin/env bash
# fuxi-im 自签 token + curl 包装：让 Claude 自己跟玄女对话 / 看门客 / 看任务
# Why: 用户不想每次手动测；用 ~/.fuxi/im_hmac.key 自签 token 跟 PWA 同权限
#
# 用法：
#   im-test.sh say "你好玄女"            # 给玄女发消息（user_intervention）
#   im-test.sh read [limit] [conv]      # 读对话历史，默认 xuannv 30 条
#   im-test.sh nodes                    # 列节点 + worker 状态
#   im-test.sh tasks                    # 列 running + completed 任务
#   im-test.sh tail [from_cursor]       # WS 流（需 websocat）
#   im-test.sh tail-poll [ms]           # 不依赖 ws 的轮询版（每 ms 拉一次最新对话）
#   im-test.sh kill <agent_id>          # ssh home fuxi kill <id>
#   im-test.sh events <kind> <hours>    # 直接查 events.db（绕 API）
#
# Token 缓存在 /tmp/fuxi-im-token；过期/不存在自动 ssh home 重签。

set -euo pipefail

BASE="${FUXI_IM_BASE:-https://im.qmledmq.cn:8443}"
TOKEN_FILE="/tmp/fuxi-im-token"
HOME_HOST="${FUXI_HOME_HOST:-home}"

mint_token() {
  ssh "$HOME_HOST" 'python3 ~/.fuxi/im-mint-token.py' > "$TOKEN_FILE"
  chmod 600 "$TOKEN_FILE"
}

token() {
  if [ ! -s "$TOKEN_FILE" ]; then
    mint_token
  fi
  cat "$TOKEN_FILE"
}

api() {
  local method="$1" path="$2"
  shift 2
  curl -sS -X "$method" \
    -b "fuxi_im_token=$(token)" \
    -H 'Content-Type: application/json' \
    "$@" \
    "$BASE$path"
}

# 发现 401 自动重签一次再试
api_retry() {
  local method="$1" path="$2"
  shift 2
  local resp http
  resp=$(curl -sS -w '\n%{http_code}' -X "$method" \
    -b "fuxi_im_token=$(token)" \
    -H 'Content-Type: application/json' \
    "$@" "$BASE$path")
  http=$(echo "$resp" | tail -n1)
  if [ "$http" = "401" ]; then
    echo "[token expired, re-minting]" >&2
    mint_token
    resp=$(curl -sS -w '\n%{http_code}' -X "$method" \
      -b "fuxi_im_token=$(token)" \
      -H 'Content-Type: application/json' \
      "$@" "$BASE$path")
    http=$(echo "$resp" | tail -n1)
  fi
  echo "$resp" | sed '$d'
  [ "$http" -lt 400 ]
}

cmd="${1:-help}"
shift || true

case "$cmd" in
  say)
    text="${1:-}"
    [ -z "$text" ] && { echo "用法: im-test.sh say <text>" >&2; exit 1; }
    api_retry POST /api/intervene \
      --data "$(jq -nc --arg t "$text" '{text:$t,interrupt:false,target:null,mentions:[],attachments:[]}')" \
      | jq .
    ;;
  read)
    limit="${1:-30}"
    conv="${2:-xuannv}"
    api_retry GET "/api/conv/messages?conv=$conv&limit=$limit" \
      | jq '.messages | reverse | .[] | "\(.ts) [\(.role)] \(.content | tostring | .[0:200])"' -r
    ;;
  nodes)
    api_retry GET /api/nodes | jq '.nodes[] | {node_id, online, inflight_jobs, workers: [.workers[]? | {agent_id, status, current_task_title}]}'
    ;;
  tasks)
    api_retry GET /api/tasks | jq '{running: [.running[] | {id: .id[0:8], title, status, members: [.members[]? | "\(.agent_id[0:8]):\(.status)"]}], completed_count: (.completed | length), latest_completed: [.completed[0:5][] | {id: .id[0:8], title}]}'
    ;;
  tail)
    from="${1:-}"
    if ! command -v websocat >/dev/null 2>&1; then
      echo "需要 websocat。或用: im-test.sh tail-poll" >&2
      exit 1
    fi
    url="${BASE/https:/wss:}/api/conv${from:+?from=$from}"
    websocat --header "Cookie: fuxi_im_token=$(token)" "$url"
    ;;
  tail-poll)
    interval_ms="${1:-2000}"
    last_id=""
    while true; do
      resp=$(api_retry GET '/api/conv/messages?conv=xuannv&limit=5')
      if [ -n "$last_id" ]; then
        echo "$resp" | jq -r --arg lid "$last_id" '.messages | reverse | map(select(.id > $lid)) | .[] | "\(.ts) [\(.role)] \(.content | tostring | .[0:200])"'
      else
        echo "$resp" | jq -r '.messages | reverse | .[-1] | "\(.ts) [\(.role)] \(.content | tostring | .[0:200])"'
      fi
      new_last=$(echo "$resp" | jq -r '.messages[0].id // empty')
      [ -n "$new_last" ] && last_id="$new_last"
      sleep "$(awk "BEGIN {print $interval_ms/1000}")"
    done
    ;;
  kill)
    aid="${1:-}"
    [ -z "$aid" ] && { echo "用法: im-test.sh kill <agent_id>" >&2; exit 1; }
    ssh "$HOME_HOST" "/home/e0-7/.local/bin/fuxi kill --id $aid"
    ;;
  events)
    kind="${1:-}"
    hours="${2:-1}"
    [ -z "$kind" ] && { echo "用法: im-test.sh events <kind_tag> [hours=1]" >&2; exit 1; }
    ssh "$HOME_HOST" "sqlite3 -readonly ~/.fuxi/events.db \"SELECT at, agent, task, substr(payload,1,200) FROM events WHERE kind_tag='$kind' AND at > datetime('now','-$hours hour') ORDER BY at DESC LIMIT 30\""
    ;;
  mint)
    mint_token
    echo "minted: $(token | cut -c1-40)..."
    ;;
  *)
    sed -n '2,17p' "$0" >&2
    exit 1
    ;;
esac

#!/usr/bin/env bash
# scripts/smoke-p0.sh — 部署后健康检查 · v1-session9 P0 修复回归
#
# 在装了 fuxi-im 的机器上跑（home 部署机），覆盖 P0.A / P0.D 这两条**确定性**
# 后端契约。视觉/触感的 P0.C / P0.E 仍要人眼看；P0.B 已在 cargo unit test
# 覆盖（subcommands::tests::derive_title_*）。
#
# 用法：
#   ./scripts/smoke-p0.sh                      本机直跑
#   ssh home 'bash -s' < scripts/smoke-p0.sh   远端跑
#
# 退出码：0 全过；非 0 = 至少一条 fail。每条单独 echo 标记（PASS / FAIL）。

set -u  # 不要 set -e —— 即使某条 fail 也跑完所有断言

FUXI_BIN="${FUXI_BIN:-${HOME}/.cargo/bin/fuxi}"
IM_HOST="${IM_HOST:-127.0.0.1:9100}"   # plain HTTP 内监听；nginx 之外
HMAC_KEY="${HMAC_KEY:-${HOME}/.fuxi/im_hmac.key}"

fails=0
mark() {
  local name="$1" ok="$2" detail="${3:-}"
  if [[ "$ok" == "1" ]]; then
    printf '  \033[32mPASS\033[0m  %s\n' "$name"
  else
    printf '  \033[31mFAIL\033[0m  %s  %s\n' "$name" "$detail"
    fails=$((fails+1))
  fi
}

echo "== smoke-p0 =="
echo "fuxi=$FUXI_BIN  host=$IM_HOST"
echo

# ─── 0. 前置：fuxi 二进制 + im 服务在跑 ──────────────────────────────
if [[ ! -x "$FUXI_BIN" ]]; then
  mark "前置:fuxi 可执行" 0 "$FUXI_BIN 不存在或不可执行"
  exit 1
fi
mark "前置:fuxi 可执行" 1
if ! curl -fsS --max-time 3 "http://$IM_HOST/healthz" >/dev/null 2>&1; then
  mark "前置:/healthz 200" 0 "fuxi-im 没在 :$IM_HOST 跑？"
  exit 1
fi
mark "前置:/healthz 200" 1

# ─── P0.A · 玄女 cc 进程 cmdline 含 --disallowed-tools + 默认派活教学 ─
echo
echo "── P0.A · 玄女 disallowed-tools + 默认派活教学 ──"

# 玄女 cc 进程 cmdline 包含 `--append-system-prompt`；用户的小希 cc 跑 `--system-prompt`。
# 用 `--append-system-prompt` 锁玄女、避免误中其它 cc。
XUANNV_CMDLINE="$(ps -eo cmd | grep -F -- '--append-system-prompt' | grep 'claude --sdk-url' | grep -v grep | head -1)"
if [[ -z "$XUANNV_CMDLINE" ]]; then
  mark "P0.A:玄女 cc 进程存活" 0 "ps 没找到 claude --sdk-url 进程（带 --append-system-prompt）"
  XUANNV_CMDLINE=""  # 让后面的断言全 fail，便于一次性看清
else
  mark "P0.A:玄女 cc 进程存活" 1
fi

# disallowed-tools 必含的 6 个核心工具——cc 把它们空格分隔传 `--disallowed-tools <args>`，
# 所以 grep 要跨过中间的空格 + 逗号，用 `.*` 不要 `[^ ]*`。
DISALLOWED_FIELD="$(echo "$XUANNV_CMDLINE" \
  | grep -oE -- '--disallowed-tools[[:space:]]+[A-Za-z,]+' || true)"
for tool in Edit Write Task Agent Glob Grep; do
  if echo "$DISALLOWED_FIELD" | grep -qw "$tool" \
     || echo "$DISALLOWED_FIELD" | grep -qE "(^|,)$tool(,|$)"; then
    mark "P0.A:--disallowed-tools 含 ${tool}" 1
  else
    mark "P0.A:--disallowed-tools 含 ${tool}" 0 "字段=${DISALLOWED_FIELD:-(找不到)}"
  fi
done

# 反回归：cmdline 不应硬编 --model（fallback 已空，env 未设时不发 flag）。
# 用户实测撞到 `--model sonnet` 在某种 auth 下被拒（1M context Extra usage 门）。
# FUXI_CC_MODEL=xxx 显式覆盖也不应是 sonnet（其它模型 OK）。
MODEL_FIELD="$(echo "$XUANNV_CMDLINE" | grep -oE -- '--model[[:space:]]+[A-Za-z0-9._-]+' || true)"
if [[ -z "$MODEL_FIELD" ]]; then
  mark "P0.A:玄女 cmdline 不硬编 --model（让 cc 走账号默认）" 1
elif echo "$MODEL_FIELD" | grep -qE -- '--model[[:space:]]+sonnet$'; then
  mark "P0.A:玄女 cmdline 不硬编 --model（让 cc 走账号默认）" 0 \
    "回归了：$MODEL_FIELD（在某些 auth 下解到 1M 变体被拒）"
else
  mark "P0.A:玄女 cmdline 不硬编 --model（让 cc 走账号默认）" 1
  echo "      （检测到 ${MODEL_FIELD}——FUXI_CC_MODEL 显式覆盖，OK）"
fi

# system prompt 必含「默认派活」段
if echo "$XUANNV_CMDLINE" | grep -q "默认派活"; then
  mark "P0.A:system prompt 含「默认派活」" 1
else
  mark "P0.A:system prompt 含「默认派活」" 0 "ROLE.md 旧版？refresh 没生效？"
fi
if echo "$XUANNV_CMDLINE" | grep -q "公理 #7"; then
  mark "P0.A:system prompt 含「公理 #7」" 1
else
  mark "P0.A:system prompt 含「公理 #7」" 0
fi

# ─── 鉴权：用本机 HMAC key 签一个 1 小时 token ────────────────────
echo
echo "── 鉴权：fuxi im issue-token ──"
TOKEN="$("$FUXI_BIN" im issue-token --key "$HMAC_KEY" --name smoke-p0 --device-id smoke-p0 2>/dev/null)"
if [[ -z "$TOKEN" || "$TOKEN" != *.* ]]; then
  mark "鉴权:issue-token 出 token" 0 "二进制可能没有该子命令（旧版？）；P0.D 跳过"
  echo
  echo "== 总计：$fails 条 fail =="
  exit $fails
fi
mark "鉴权:issue-token 出 token" 1

# ─── P0.D · /api/{workers,tasks}/:id/events HTTP wire shape ────────
echo
echo "── P0.D · /events HTTP 返 {events, next_cursor} 不再裸数组 ──"

# 用一个肯定不存在的 agent_id —— 后端按 filter 跑出空 events 数组，wire shape 仍要正确
GHOST_AGENT="00000000-0000-0000-0000-000000000000"
RESP="$(curl -sS --max-time 5 \
  -H "Cookie: fuxi_im_token=$TOKEN" \
  "http://$IM_HOST/api/workers/$GHOST_AGENT/events?limit=1" 2>&1)"

# 1) 顶层不是裸数组（ '[' 开头 = 老 bug 复活）
if [[ "${RESP:0:1}" == "[" ]]; then
  mark "P0.D:/api/workers/:id/events 不是裸数组" 0 "回复以 '[' 开头：$RESP"
else
  mark "P0.D:/api/workers/:id/events 不是裸数组" 1
fi

# 2) 顶层是 object 含 events 字段
if echo "$RESP" | grep -q '"events"'; then
  mark "P0.D:响应含 events 字段" 1
else
  mark "P0.D:响应含 events 字段" 0 "$RESP"
fi

# 3) next_cursor 字段存在（null 也算）
if echo "$RESP" | grep -q '"next_cursor"'; then
  mark "P0.D:响应含 next_cursor 字段" 1
else
  mark "P0.D:响应含 next_cursor 字段" 0 "$RESP"
fi

# tasks 端同理 —— 用一个肯定不存在的 task uuid
GHOST_TASK="00000000-0000-0000-0000-000000000000"
RESP2="$(curl -sS --max-time 5 \
  -H "Cookie: fuxi_im_token=$TOKEN" \
  "http://$IM_HOST/api/tasks/$GHOST_TASK/events?limit=1" 2>&1)"

if [[ "${RESP2:0:1}" == "[" ]]; then
  mark "P0.D:/api/tasks/:id/events 不是裸数组" 0 "回复以 '[' 开头：$RESP2"
else
  mark "P0.D:/api/tasks/:id/events 不是裸数组" 1
fi

if echo "$RESP2" | grep -q '"events".*"next_cursor"\|"next_cursor".*"events"'; then
  mark "P0.D:/api/tasks 响应同含 events + next_cursor" 1
else
  mark "P0.D:/api/tasks 响应同含 events + next_cursor" 0 "$RESP2"
fi

# ─── 总结 ──────────────────────────────────────────────────────────
echo
if [[ "$fails" == "0" ]]; then
  echo -e "== \033[32m全过\033[0m =="
  exit 0
fi
echo -e "== \033[31m$fails 条 fail\033[0m =="
exit "$fails"

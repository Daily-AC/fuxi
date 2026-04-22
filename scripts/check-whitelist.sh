#!/usr/bin/env bash
set -euo pipefail

# 用法：
#   scripts/check-whitelist.sh
#   scripts/check-whitelist.sh --staged
#   scripts/check-whitelist.sh --whitelist docs/handoff/whitelist-files.txt

WHITELIST_FILE="docs/handoff/whitelist-files.txt"
STAGED_ONLY=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --staged)
      STAGED_ONLY=1
      shift
      ;;
    --whitelist)
      WHITELIST_FILE="${2:-}"
      shift 2
      ;;
    *)
      echo "未知参数: $1" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$WHITELIST_FILE" || ! -f "$WHITELIST_FILE" ]]; then
  echo "白名单文件不存在: $WHITELIST_FILE" >&2
  exit 2
fi

RAW_RULES=()
while IFS= read -r line; do
  RAW_RULES+=("$line")
done < <(grep -v '^[[:space:]]*#' "$WHITELIST_FILE" | sed '/^[[:space:]]*$/d')

if [[ ${#RAW_RULES[@]} -eq 0 ]]; then
  echo "白名单为空: $WHITELIST_FILE" >&2
  exit 2
fi

CHANGED=()
if [[ $STAGED_ONLY -eq 1 ]]; then
  while IFS= read -r line; do
    CHANGED+=("$line")
  done < <(git diff --cached --name-only --diff-filter=ACMRTUXB)
else
  while IFS= read -r line; do
    CHANGED+=("$line")
  done < <(git status --porcelain | awk '{print $2}')
fi

if [[ ${#CHANGED[@]} -eq 0 ]]; then
  echo "OK: 当前无改动。"
  exit 0
fi

is_allowed() {
  local path="$1"
  local rule
  for rule in "${RAW_RULES[@]}"; do
    if [[ "$rule" == */ ]]; then
      [[ "$path" == "$rule"* ]] && return 0
    else
      [[ "$path" == "$rule" ]] && return 0
    fi
  done
  return 1
}

declare -a OUT_OF_SCOPE
for path in "${CHANGED[@]}"; do
  if ! is_allowed "$path"; then
    OUT_OF_SCOPE+=("$path")
  fi
done

if [[ ${#OUT_OF_SCOPE[@]} -gt 0 ]]; then
  echo "FAIL: 发现白名单外改动 (${#OUT_OF_SCOPE[@]}):"
  printf '  - %s\n' "${OUT_OF_SCOPE[@]}"
  echo
  echo "白名单: $WHITELIST_FILE"
  echo "可通过 --whitelist 指定其他白名单文件。"
  exit 1
fi

echo "OK: 改动均在白名单内（${#CHANGED[@]} 项）。"

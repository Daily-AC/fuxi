#!/usr/bin/env bash
# 论文引用闭合扫描：列出 main.md 里引用但 refs.bib 没定义的 key，
# 以及 refs.bib 定义但正文没引用的 key。
set -euo pipefail
cd "$(dirname "$0")/../deliverables/thesis-v2"

cited=$(grep -hoE '@[a-zA-Z0-9_]+' chapters/*.md 2>/dev/null | sort -u || true)
defined=$(grep -oE '^@[a-z]+\{[a-zA-Z0-9_]+,' refs.bib | sed -E 's/^@[a-z]+\{([a-zA-Z0-9_]+),/@\1/' | sort -u)

echo "=== 引用但未定义 (cite without entry) ==="
comm -23 <(echo "$cited") <(echo "$defined") || true
echo ""
echo "=== 定义但未引用 (entry without cite) ==="
comm -13 <(echo "$cited") <(echo "$defined") || true

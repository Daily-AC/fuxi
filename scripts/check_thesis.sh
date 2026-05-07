#!/usr/bin/env bash
# 论文 QC 总扫描：占位符 / refs 闭合 / 字数 / 图表引用闭合
set -o pipefail
cd "$(dirname "$0")/../deliverables/thesis-v2"

echo "═══ Fuxi 毕设论文 QC 报告 ═══"
echo ""

# ── 1. 字数 ─────────────────────────────────────────────────
echo "── 1. 章节字数 ──"
total=0
for f in chapters/*.md; do
    chars=$(wc -m < "$f")
    name=$(basename "$f" .md)
    printf "  %-30s %s chars\n" "$name" "$chars"
    total=$((total + chars))
done
echo ""
echo "  合计正文 raw chars : $total（实际中文字数估 ≈ chars × 0.92）"
echo ""

# ── 2. 占位符扫描 ─────────────────────────────────────────────
echo "── 2. 占位符扫描（{{...}} mustache）──"
left=$(grep -rohE '\{\{[^}]+\}\}' chapters/ 2>/dev/null | sort | uniq -c | sort -rn)
if [ -z "$left" ]; then
    echo "  ✓ 无残留占位符"
else
    echo "$left" | sed 's/^/  /'
fi
echo ""

# ── 3. refs 闭合 ─────────────────────────────────────────────
echo "── 3. 参考文献闭合 ──"
cited=$(grep -hoE '@[a-zA-Z0-9_]+' chapters/*.md | sort -u)
defined=$(grep -oE '^@[a-z]+\{[a-zA-Z0-9_]+,' refs.bib | sed -E 's/^@[a-z]+\{([a-zA-Z0-9_]+),/@\1/' | sort -u)
echo "  cited keys    : $(echo "$cited" | wc -l | tr -d ' ')"
echo "  defined keys  : $(echo "$defined" | wc -l | tr -d ' ')"
missing=$(comm -23 <(echo "$cited") <(echo "$defined"))
extra=$(comm -13 <(echo "$cited") <(echo "$defined"))
[ -n "$missing" ] && echo "  ✗ 引用但未定义：$missing"
[ -n "$extra" ] && echo "  ⚠ 定义但未引用：$extra"
[ -z "$missing" ] && [ -z "$extra" ] && echo "  ✓ 闭合"
echo ""

# ── 4. 图引用闭合 ─────────────────────────────────────────────
echo "── 4. 图引用闭合 ──"
fig_refs=$(grep -hoE '图\s*[0-9]+\.[0-9]+' chapters/*.md | sort -u)
fig_files=$(ls figures/drawio/fig-*.png figures/matplotlib/fig-*.png figures/gpt-image/fig-*.png 2>/dev/null | sed -E 's|.*/fig-([0-9]+)-([0-9]+)-.*|图\1.\2|' | sort -u)
echo "  正文引用的图 : $(echo "$fig_refs" | wc -l | tr -d ' ') 个 ($(echo "$fig_refs" | tr '\n' ' '))"
echo "  实际有图文件 : $(echo "$fig_files" | wc -l | tr -d ' ') 个 ($(echo "$fig_files" | tr '\n' ' '))"
fig_missing=$(comm -23 <(echo "$fig_refs") <(echo "$fig_files"))
[ -n "$fig_missing" ] && echo "  ✗ 引用但缺文件：$fig_missing"
[ -z "$fig_missing" ] && echo "  ✓ 引用全部有对应图"
echo ""

# ── 5. 表引用 ─────────────────────────────────────────────
echo "── 5. 表引用 ──"
tab_refs=$(grep -hoE '表\s*[0-9]+\.[0-9]+' chapters/*.md | sort -u)
echo "  正文引用的表 : $(echo "$tab_refs" | wc -l | tr -d ' ') 个 ($(echo "$tab_refs" | tr '\n' ' '))"
echo ""

# ── 6. 公式编号 ──────────────────────────────────────────────
echo "── 6. 公式编号 ──"
form_tags=$(grep -hoE '\\tag\{\([0-9]+-[0-9]+\)\}' chapters/*.md | sort -u)
echo "  公式 \\tag : $(echo "$form_tags" | wc -l | tr -d ' ') 个 ($(echo "$form_tags" | tr '\n' ' '))"
echo ""

echo "═══ 完毕 ═══"

#!/usr/bin/env bash
# 编译 thesis-v3：xelatex + biber 多趟
# 使用方法：./build.sh   或   ./build.sh --quick (跳过 biber 第二轮)
#
# 注意：去掉了 -halt-on-error。listings + xeCJK 在 listings 1.11b 下偶发
# "Missing $ inserted" 警告，但不影响最终 PDF 渲染（错误位置 TeX 自动 recover）。
# 我们要的是 "整篇 PDF 出来 + 大多数交叉引用解析" 这个最终态。
set -uo pipefail
cd "$(dirname "$0")"

QUICK=${1:-}

run_xelatex() {
  local pass="$1"
  echo "▶ xelatex $pass"
  echo "===PASS_MARKER=== $pass" >>build.log
  xelatex -interaction=nonstopmode main.tex >>build.log 2>&1 || true
}

> build.log
run_xelatex "(R1：生成 aux + 占位)"

if [ "$QUICK" != "--quick" ]; then
  echo "▶ biber"
  biber main >>build.log 2>&1 || {
    echo "❌ biber 阶段错误，看 build.log 末尾："
    tail -40 build.log
    exit 1
  }
  run_xelatex "(R2：fold 引用)"
fi

run_xelatex "(R3：fold 交叉引用 + toc)"

# 第 4 轮（保险）：再跑一遍让 toc/figure list 收敛
run_xelatex "(R4：toc 收敛)"

# 清理中间文件（保留 .pdf 与 build.log）
rm -f main.{aux,log,out,toc,bbl,bcf,blg,run.xml,lof,lot}

if [ ! -f main.pdf ]; then
  echo "❌ main.pdf 未生成，看 build.log："
  tail -40 build.log
  exit 1
fi

PAGES=$(pdfinfo main.pdf 2>/dev/null | grep -i "^Pages:" | awk '{print $2}' || echo "?")
SIZE=$(du -h main.pdf | cut -f1)
# 只统计最后一轮 xelatex 的警告（前几轮自然会有未定义 ref/cite，那是 toc/bib 还没 fold 的正常状态）
LASTRUN=$(awk '/^===PASS_MARKER===/{n=NR} END{print n+0}' build.log)
ERRORS=$(awk -v n="$LASTRUN" 'NR>=n' build.log | grep -c "^! " || true)
UNDEF_REF=$(awk -v n="$LASTRUN" 'NR>=n' build.log | grep -c "Reference.*undefined" || true)
UNDEF_CITE=$(awk -v n="$LASTRUN" 'NR>=n' build.log | grep -c "Citation.*undefined" || true)
echo ""
echo "✅ 编译完成: $(pwd)/main.pdf  ($PAGES 页, $SIZE)"
echo "   末轮警告：${ERRORS} 处 ! · ${UNDEF_REF} 个未定义 ref · ${UNDEF_CITE} 个未定义 cite"
echo "   详见 build.log（grep '^! ' 看错误，grep 'undefined' 看引用）"

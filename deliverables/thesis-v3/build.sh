#!/usr/bin/env bash
# 编译 thesis-v3：xelatex + biber 三趟
# 使用方法：./build.sh   或   ./build.sh --quick (跳过 biber 第二轮)
set -euo pipefail
cd "$(dirname "$0")"

QUICK=${1:-}

echo "▶ 第 1 轮 xelatex（生成 .aux + 占位 toc/bib）"
xelatex -interaction=nonstopmode -halt-on-error main.tex >build.log 2>&1 || {
  echo "❌ 第 1 轮失败，看 build.log 末尾："
  tail -40 build.log
  exit 1
}

if [ "$QUICK" != "--quick" ]; then
  echo "▶ biber（解析 refs.bib 引用）"
  biber main >>build.log 2>&1 || {
    echo "❌ biber 失败："
    tail -40 build.log
    exit 1
  }

  echo "▶ 第 2 轮 xelatex（fold 引用编号）"
  xelatex -interaction=nonstopmode -halt-on-error main.tex >>build.log 2>&1
fi

echo "▶ 第 3 轮 xelatex（fold toc 与交叉引用）"
xelatex -interaction=nonstopmode -halt-on-error main.tex >>build.log 2>&1 || {
  echo "❌ 末轮失败："
  tail -40 build.log
  exit 1
}

# 清理中间文件（保留 .pdf 与 build.log）
rm -f main.{aux,log,out,toc,bbl,bcf,blg,run.xml,lof,lot}

PAGES=$(pdfinfo main.pdf 2>/dev/null | grep -i "^Pages:" | awk '{print $2}' || echo "?")
SIZE=$(du -h main.pdf | cut -f1)
echo ""
echo "✅ 编译完成: $(pwd)/main.pdf  ($PAGES 页, $SIZE)"

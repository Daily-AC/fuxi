#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

# 拼接 chapters/*.md 按字典序（00-frontmatter → 99-acknowledgements）。
# main.md 提供 yaml metadata（title/author/date），不参与正文拼接。
cat main.md > _full.md
echo "" >> _full.md
cat chapters/*.md >> _full.md

# pandoc → docx；不传 reference.docx，让学校格式由用户对照 format-checklist.md 手调。
# --citeproc 处理 [@key] 引用，渲染成 [1][2] 风格 + 文末参考文献列表。
pandoc _full.md \
  --from markdown+yaml_metadata_block+raw_tex \
  --to docx \
  --bibliography refs.bib \
  --citeproc \
  --csl=gb-t-7714.csl \
  --metadata link-citations=true \
  --toc \
  --toc-depth=2 \
  -o "基于AI_Agent的高性能分布式通讯系统_v2.docx"

rm _full.md
echo "已导出：$(pwd)/基于AI_Agent的高性能分布式通讯系统_v2.docx"

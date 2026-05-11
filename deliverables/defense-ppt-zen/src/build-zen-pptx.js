// 把 13 张 zen_sXX.png 合成一个 pptx, 每张全 bleed, 16:9
// 不加任何 chrome (无标题栏 / 无页码 / 无装饰), 保持 deck 大道至简的视觉

const path = require("path");
const fs = require("fs");
const pptxgen = require("/Users/e0_7/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/pptxgenjs");

const DECK_DIR = "/Users/e0_7/fuxi/deliverables/defense-ppt-zen";
const PNG_DIR = path.join(DECK_DIR, "slides/png");
const OUT_PATH = path.join(DECK_DIR, "fuxi-defense-zen-2026-05-09.pptx");

const pptx = new pptxgen();
pptx.layout = "LAYOUT_WIDE";
pptx.author = "张以琳";
pptx.subject = "基于 AI Agent 的高性能分布式通讯系统";
pptx.title = "伏羲毕业答辩 (zen)";
pptx.company = "Fuxi";
pptx.lang = "zh-CN";

// 16:9 13.333 × 7.5 inch (LAYOUT_WIDE)
const W = 13.333;
const H = 7.5;

const files = fs
  .readdirSync(PNG_DIR)
  .filter((f) => /^zen_s\d{2}\.png$/.test(f))
  .sort();

console.log(`[zen-pptx] 共 ${files.length} 张, 输出 ${OUT_PATH}`);

for (const f of files) {
  const slide = pptx.addSlide();
  // 用 #FCFAF6 (deck 暖白纸底色) 兜底, 如果图片渲染有微小空白不破风格
  slide.background = { color: "FCFAF6" };
  slide.addImage({
    path: path.join(PNG_DIR, f),
    x: 0,
    y: 0,
    w: W,
    h: H,
    sizing: { type: "cover", x: 0, y: 0, w: W, h: H },
  });
}

pptx.writeFile({ fileName: OUT_PATH }).then((r) => {
  const sz = (fs.statSync(r).size / 1024 / 1024).toFixed(2);
  console.log(`[zen-pptx] ✓ ${r} (${sz} MB)`);
});

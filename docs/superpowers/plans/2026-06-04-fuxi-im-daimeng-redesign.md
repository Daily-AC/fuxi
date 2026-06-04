# 伏羲 IM 呆萌风重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 fuxi-im PWA 从"暖棕专业 Claude Code 风"整套重做成奶油糖果呆萌风 + 可交互玄女吉祥物，移动端优先，质感拉满反塑料反 AI。

**Architecture:** 纯前端重构（Solid + Vite PWA + CSS Modules），复用现有后端 API（A2A/EventBus/conv_store 不动）。先立"设计系统层 + 吉祥物 + 共享原语组件"地基，再换底栏/导航模型（新增「家」tab，「通知」并入家），然后逐页按 5 个原型重绘，最后 loading/空态/转场打磨 + PWA 资源 + e2e + home 实测。对外一次性上线。

**Tech Stack:** Solid.js 1.9 · Vite 6 · CSS Modules · vitest + @solidjs/testing-library · Playwright · rembg/gpt-image-2（吉祥物资源管线，已跑通）。

**Spec:** `docs/superpowers/specs/2026-06-04-fuxi-im-呆萌重构-design.md`（先读，尤其 §2 质感系统硬约束 + §6.2 页面原型表）。

**工作目录:** `crates/fuxi-im/web`（下文相对路径都相对它）。分支 `feat/fuxi-im-daimeng-redesign`（已建，spec 已 commit）。

**约定（每个 worker 必读）:**
- 测试在 `tests/unit/*.test.tsx`，用 `vitest` + `@solidjs/testing-library`，import 用 `~/` alias，断言走 `data-testid`。跑：`npm test`（vitest run）。
- 类型检查 `npm run typecheck`；lint `npm run lint`。三者 + 测试全绿才 commit。
- **质感 gate（§2 硬约束）**：每个出可视页面的 task，最后一步必须 `npm run dev` 起本地 + Playwright 截图，**人眼**确认"像精致 app 不像 AI 糖水"。截图存 `/tmp/im-shots/<task>.png` 并在 commit message 注明已自检。不达标重做，不准跳过。
- **禁 emoji**（记忆 `feedback_no_emoji_ui_too`）：图标全 SVG。
- TDD：先写失败测试 → 跑红 → 最小实现 → 跑绿 → commit。CSS/视觉无法单测的部分，用 testid 结构测 + 截图 gate 兜。

---

## 文件结构总览（先看清边界）

**新增：**
- `src/styles/texture.css` — 质感 utility 层（noise / mesh-gradient / glass / soft-shadow class）。全局 import。
- `src/components/Mascot/Mascot.tsx` + `.module.css` — 吉祥物渲染组件（帧切换 + CSS 动效）。
- `src/components/Mascot/mascotMachine.ts` — 纯函数状态机（事件 → state），可单测，无 DOM。
- `src/components/Mascot/MascotController.tsx` — context/provider，把 app 事件映射成 state，供多处共用。
- `src/components/Mascot/quips.ts` — 戳一戳俏皮话本地文案池。
- `src/components/ui/` — 共享原语：`Card.tsx` `ListRow.tsx` `ScreenHeader.tsx` `StatePill.tsx` `Tile.tsx` `ToggleRow.tsx` `SectionLabel.tsx` `EmptyState.tsx` `Skeleton.tsx` `MascotLoader.tsx`（各配 `.module.css`）。
- `src/views/pages/HomePage.tsx` + `.module.css` — 新「家」客厅主屏。
- `public/mascot/xuannv-<state>.webp` — 吉祥物帧资源（由 `design-assets/mascot-v1/transparent/` 压缩转换而来）+ `public/mascot/manifest.json`。
- `public/textures/noise.png` — 噪点 tile。

**重写（token 化 + 原语化 + 原型化）：**
- `src/tokens.ts` + `src/styles/global.css` — 奶油糖果 token（双出口同步）。
- `src/components/BottomTabBar.tsx` + `.module.css` — 4 tab（家/聊天/任务/更多）+ glass。
- `src/components/ApiProvider.tsx` — tab 模型（0=家 1=聊天 2=任务 3=更多；通知并入家）。
- `src/App.tsx` — MainShell 接 HomePage + tab 索引调整。
- 全部 `src/views/pages/*.tsx`+`.module.css` 与 `src/components/messages/*`、`Composer`/`Mention*`/`TopicSidebar`/`NavigationStack`/`MoreSubShell`/`Toast` — 按原型重绘上色。
- `vite.config.ts` PWA manifest 主题色 + `public/icons/*` 重做。

---

# Phase 1 · 设计系统层 + 吉祥物 + 共享原语（地基）

## Task 1: 奶油糖果 token —— `tokens.ts`

**Files:**
- Modify: `src/tokens.ts`
- Test: `tests/unit/tokens.test.ts`（新建）

- [ ] **Step 1: 写失败测试**

```ts
// tests/unit/tokens.test.ts
import { describe, expect, it } from "vitest";
import { tokens, colorForRole } from "~/tokens";

describe("奶油糖果 tokens", () => {
  it("背景是暖奶白不是旧暗棕", () => {
    expect(tokens.bg.toUpperCase()).toBe("#FFF8F0");
    expect(tokens.bg).not.toBe("#1F1E1B");
  });
  it("主 accent 是蜜桃", () => {
    expect(tokens.accent.toUpperCase()).toBe("#FFB877");
  });
  it("玄女角色色是 lavender", () => {
    expect(colorForRole("xuannv").toUpperCase()).toBe("#C9B6E8");
    expect(colorForRole("玄女")).toBe(colorForRole("xuannv"));
  });
  it("卡片圆角更胖（≥20）", () => {
    expect(tokens.radius.card).toBeGreaterThanOrEqual(20);
  });
});
```

- [ ] **Step 2: 跑红** — `npm test -- tokens` → FAIL（值还是旧暗棕）。

- [ ] **Step 3: 重写 `tokens.ts`** 按 spec §3：

```ts
export const tokens = {
  bg: "#FFF8F0",
  surface: "#FFFFFF",
  surfaceSoft: "#FFFDFA",
  surfaceElevated: "#FFFFFF",
  border: "#F0E2D2",
  borderStrong: "#E6D5C0",

  textPrimary: "#5A4A3A",
  textSecondary: "#9A8C7A",
  textMuted: "#C3B4A2",

  accent: "#FFB877",
  accentDeep: "#E8915A",   // 文字/描边用，在 accent 底上对比足够
  accentSubtle: "#FFF0DC",
  onAccent: "#FFFFFF",

  mint: "#8FD9B6",
  mintSoft: "#A8E6CF",
  lavender: "#C9B6E8",
  pink: "#FF9EB5",

  xuannv: "#C9B6E8",
  luban: "#E8915A",
  pusong: "#6FB893",

  userBubble: "#FFFFFF",
  userBubbleText: "#5A4A3A",

  success: "#6FB893",
  warning: "#E8A23A",
  danger: "#E8857A",

  fontSans: '"Yuanti SC", -apple-system, BlinkMacSystemFont, "PingFang SC", "Noto Sans CJK SC", system-ui, sans-serif',
  fontRound: '"Yuanti SC", "PingFang SC", system-ui, sans-serif',
  fontMono: '"JetBrains Mono", "SF Mono", Menlo, Consolas, monospace',
  size: { meta: 11, aux: 13, body: 15, heading: 16, title: 19 },
  weight: { normal: 400, medium: 500, semibold: 700 },
  radius: { card: 20, bubble: 20, sheet: 28, pill: 999, chip: 14 },

  touch: 44,
  headerHeight: 54,
  composerHeight: 64,
} as const;

export function colorForRole(role: string | null | undefined): string {
  switch (role) {
    case "xuannv": case "玄女": return tokens.xuannv;
    case "luban": case "鲁班": return tokens.luban;
    case "pusong": case "蒲松": return tokens.pusong;
    default: return tokens.textSecondary;
  }
}
```

- [ ] **Step 4: 跑绿** — `npm test -- tokens` → PASS。`npm run typecheck` → 若别处引用了删掉的字段（如 `accentDim`/`surfaceElevated` 命名变化）会报错，记录待 Task 修（先不动别处，保留旧字段别名以防大面积红：在 tokens 里补 `accentDim: "#FFF0DC"` 等兼容别名，commit message 注明"过渡别名，Phase 3 清"）。

- [ ] **Step 5: Commit** — `git add src/tokens.ts tests/unit/tokens.test.ts && git commit -m "feat(im): 奶油糖果设计 token（tokens.ts）"`

## Task 2: 奶油糖果 CSS 变量 —— `global.css :root`

**Files:**
- Modify: `src/styles/global.css`（`:root` 块 + 顶部硬规则注释）

- [ ] **Step 1: 改注释**（旧文件顶部写着 `no shadow / no gradient / no glassmorphism`——本次明确反转）。把该注释块替换为：

```css
/* fuxi-im 呆萌风 · 视觉规则源头：spec 2026-06-04 §2 质感系统
 * 硬规则：必须有 柔影 / 网状渐变 / 暖光高光 / 克制毛玻璃 / 噪点纸纹（反塑料反 AI）
 * 禁 emoji（图标走 SVG）。等宽仅限 code/agent-id/tool 输出。
 */
```

- [ ] **Step 2: 重写 `:root` 变量** 与 `tokens.ts` 一一对齐（同名 kebab-case）：`--bg:#FFF8F0; --surface:#fff; --surface-soft:#FFFDFA; --border:#F0E2D2; --text-primary:#5A4A3A; --text-secondary:#9A8C7A; --text-muted:#C3B4A2; --accent:#FFB877; --accent-deep:#E8915A; --accent-subtle:#FFF0DC; --on-accent:#fff; --mint:#8FD9B6; --lavender:#C9B6E8; --pink:#FF9EB5; --xuannv:#C9B6E8; --luban:#E8915A; --pusong:#6FB893; --success:#6FB893; --warning:#E8A23A; --danger:#E8857A;` + radius（`--radius-card:20px; --radius-bubble:20px; --radius-sheet:28px; --radius-pill:999px; --radius-chip:14px;`）+ 字体 `--font-sans`/`--font-round`。保留旧的 `--space-*` `--transition-*` `--touch` `--header-h`。新增软影变量见 Task 3。

- [ ] **Step 3: 验证** — `npm run dev`，开浏览器看不报错、背景变奶白。`npm run lint` 绿。

- [ ] **Step 4: Commit** — `git add src/styles/global.css && git commit -m "feat(im): global.css 奶油糖果变量 + 反转质感规则注释"`

## Task 3: 质感 utility 层 —— `texture.css` + 噪点 tile

**Files:**
- Create: `src/styles/texture.css`
- Create: `public/textures/noise.png`（128×128 透明细噪点 tile，用脚本生成）
- Modify: `src/index.tsx`（import texture.css）

- [ ] **Step 1: 生成噪点 tile** — 跑一次性脚本：

```bash
/tmp/imgvenv/bin/python - <<'PY'
from PIL import Image
import random
random.seed(7)
w=h=128
im=Image.new("RGBA",(w,h),(0,0,0,0)); px=im.load()
for y in range(h):
  for x in range(w):
    a=random.randint(0,16)  # 极淡
    px[x,y]=(120,90,60,a)
im.save("public/textures/noise.png")
print("noise tile written")
PY
```

- [ ] **Step 2: 写 `texture.css`**（class 工具，供所有组件用）：

```css
/* 质感 utility · spec §2。class 叠加到任意容器。 */
:root {
  --shadow-soft1: 0 2px 6px rgba(120,80,50,.08);
  --shadow-soft2: 0 14px 34px rgba(120,80,50,.14);
  --shadow-card: var(--shadow-soft1), 0 8px 20px rgba(120,80,50,.08);
  --inset-hi: inset 0 1px 0 rgba(255,255,255,.7);
  --glow-mascot: 0 10px 30px rgba(255,170,110,.30);
  --spring: cubic-bezier(.34,1.56,.64,1);
  --soft: cubic-bezier(.4,0,.2,1);
}
.u-mesh { background:
  radial-gradient(120% 90% at 12% 0%, #FFF6EC 0%, transparent 55%),
  radial-gradient(120% 90% at 100% 8%, #FBEFE0 0%, transparent 50%),
  radial-gradient(120% 120% at 50% 110%, #EFF4E9 0%, transparent 55%),
  #FFF8F0; }
.u-noise::after { content:""; position:absolute; inset:0; pointer-events:none;
  background:url(/textures/noise.png) repeat; opacity:.5; mix-blend-mode:multiply; border-radius:inherit; }
.u-card { background:var(--surface); border-radius:var(--radius-card);
  box-shadow:var(--shadow-card), var(--inset-hi); position:relative; }
.u-glass { background:rgba(255,247,236,.78); backdrop-filter:blur(14px) saturate(1.3);
  -webkit-backdrop-filter:blur(14px) saturate(1.3); }
@media (prefers-reduced-motion: reduce){ *{ animation:none!important; transition:none!important; } }
```

- [ ] **Step 3: import** — `src/index.tsx` 顶部加 `import "./styles/texture.css";`（在 global.css 之后）。

- [ ] **Step 4: 验证 + 截图 gate** — `npm run dev`，临时给某容器加 `class="u-card u-noise"` 看柔影 + 噪点生效；Playwright 截图人眼确认有质感不平板。撤掉临时 class。

- [ ] **Step 5: Commit** — `git add src/styles/texture.css public/textures/noise.png src/index.tsx && git commit -m "feat(im): 质感 utility 层（mesh/noise/card/glass/软影）"`

## Task 4: 圆体字体接入

**Files:**
- Modify: `src/styles/global.css`（body font-family → var(--font-sans)；标题类 → var(--font-round)）

- [ ] **Step 1:** 确认 `--font-sans`/`--font-round` 已含 `"Yuanti SC"`（macOS/iOS 自带圆体，安卓 fallback 系统圆体/PingFang）。body 设 `font-family: var(--font-sans)`。新增 `.u-title{ font-family:var(--font-round); font-weight:700; letter-spacing:.01em; }`。
- [ ] **Step 2: 验证** — dev 起，中文正文/标题字重层级清晰。截图 gate。
- [ ] **Step 3: Commit** — `git commit -am "feat(im): 圆体字体 + 标题排印"`

## Task 5: 吉祥物资源进 `public/mascot/`（webp + manifest）

**Files:**
- Create: `public/mascot/xuannv-<state>.webp`（8 个）+ `public/mascot/manifest.json`
- 源：`crates/fuxi-im/web/design-assets/mascot-v1/transparent/xuannv-<state>.png`

- [ ] **Step 1: 转 webp（带 alpha，q88，≤768px 高）** 一次性脚本：

```bash
cd crates/fuxi-im/web
/tmp/imgvenv/bin/python - <<'PY'
from PIL import Image
import pathlib, json
src=pathlib.Path("design-assets/mascot-v1/transparent")
dst=pathlib.Path("public/mascot"); dst.mkdir(parents=True, exist_ok=True)
states=["idle","blink","happy","think","talk","wave","sleep","surprise"]
man={}
for s in states:
  im=Image.open(src/f"xuannv-{s}.png").convert("RGBA")
  bb=im.getbbox()
  if bb: im=im.crop(bb)
  w,h=im.size; nh=min(h,768); nw=int(w*nh/h)
  im=im.resize((nw,nh), Image.LANCZOS)
  im.save(dst/f"xuannv-{s}.webp","WEBP",quality=88,method=6)
  man[s]={"src":f"/mascot/xuannv-{s}.webp","w":nw,"h":nh}
(dst/"manifest.json").write_text(json.dumps(man,ensure_ascii=False,indent=2))
print("webp + manifest written", {k:f"{v['w']}x{v['h']}" for k,v in man.items()})
PY
```

- [ ] **Step 2: 看图 gate** — 用 Read 工具看 2–3 张 webp（idle/happy/sleep），确认透明干净、无毛边、角色一致。不达标回 spec §4.1 管线重生。
- [ ] **Step 3: Commit** — `git add public/mascot && git commit -m "feat(im): 玄女吉祥物 8 帧 webp 资源 + manifest"`（webp 小，可进 git）

## Task 6: 吉祥物状态机（纯函数，可单测）

**Files:**
- Create: `src/components/Mascot/mascotMachine.ts`
- Test: `tests/unit/mascotMachine.test.ts`

- [ ] **Step 1: 写失败测试**

```ts
import { describe, expect, it } from "vitest";
import { reduceMascot, type MascotState, type MascotEvent } from "~/components/Mascot/mascotMachine";

describe("mascotMachine", () => {
  const idle: MascotState = { kind: "idle" };
  it("流式开始 → talk", () => {
    expect(reduceMascot(idle, { type: "stream-start" }).kind).toBe("talk");
  });
  it("门客在跑 → think", () => {
    expect(reduceMascot(idle, { type: "work-running" }).kind).toBe("think");
  });
  it("任务完成 → happy", () => {
    expect(reduceMascot(idle, { type: "task-done" }).kind).toBe("happy");
  });
  it("报错 → surprise", () => {
    expect(reduceMascot(idle, { type: "error" }).kind).toBe("surprise");
  });
  it("戳一下 → poke", () => {
    expect(reduceMascot(idle, { type: "poke" }).kind).toBe("poke");
  });
  it("happy/surprise/poke 是瞬时态，settle 后回 idle", () => {
    expect(reduceMascot({ kind: "happy" }, { type: "settle" }).kind).toBe("idle");
    expect(reduceMascot({ kind: "poke" }, { type: "settle" }).kind).toBe("idle");
  });
  it("idle 计时到点 → sleep；任何输入唤醒回 idle", () => {
    expect(reduceMascot(idle, { type: "idle-timeout" }).kind).toBe("sleep");
    expect(reduceMascot({ kind: "sleep" }, { type: "poke" }).kind).toBe("poke");
  });
  it("talk 期间 error 优先级更高 → surprise", () => {
    expect(reduceMascot({ kind: "talk" }, { type: "error" }).kind).toBe("surprise");
  });
});
```

- [ ] **Step 2: 跑红** — `npm test -- mascotMachine` → FAIL。

- [ ] **Step 3: 实现** `mascotMachine.ts`：

```ts
export type MascotStateKind = "idle"|"blink"|"talk"|"think"|"happy"|"wave"|"surprise"|"sleep"|"poke";
export interface MascotState { kind: MascotStateKind }
export type MascotEvent =
  | { type: "stream-start" } | { type: "stream-end" }
  | { type: "work-running" } | { type: "work-idle" }
  | { type: "task-done" } | { type: "error" }
  | { type: "greet" } | { type: "poke" }
  | { type: "idle-timeout" } | { type: "settle" };

// settle = 瞬时态（happy/surprise/poke/wave）播放结束的归位信号
const TRANSIENT: MascotStateKind[] = ["happy","surprise","poke","wave"];

export function reduceMascot(s: MascotState, e: MascotEvent): MascotState {
  switch (e.type) {
    case "error": return { kind: "surprise" };       // 最高优先
    case "poke": return { kind: "poke" };
    case "task-done": return { kind: "happy" };
    case "greet": return { kind: "wave" };
    case "stream-start": return { kind: "talk" };
    case "work-running": return s.kind === "talk" ? s : { kind: "think" };
    case "stream-end":
    case "work-idle": return { kind: "idle" };
    case "idle-timeout": return s.kind === "idle" ? { kind: "sleep" } : s;
    case "settle": return TRANSIENT.includes(s.kind) ? { kind: "idle" } : s;
    default: return s;
  }
}
```

- [ ] **Step 4: 跑绿** — `npm test -- mascotMachine` → PASS。
- [ ] **Step 5: Commit** — `git add src/components/Mascot/mascotMachine.ts tests/unit/mascotMachine.test.ts && git commit -m "feat(im): 吉祥物状态机（纯函数 reducer）"`

## Task 7: Mascot 渲染组件（帧切换 + 动效 + 戳）

**Files:**
- Create: `src/components/Mascot/Mascot.tsx` + `Mascot.module.css` + `quips.ts`
- Test: `tests/unit/Mascot.test.tsx`

- [ ] **Step 1: 写失败测试**（结构 + testid + 戳回调）

```tsx
import { describe, expect, it, vi } from "vitest";
import { render, fireEvent } from "@solidjs/testing-library";
import { Mascot } from "~/components/Mascot/Mascot";

describe("Mascot", () => {
  it("按 state 渲染对应帧 img（src 含 state 名）", () => {
    const { getByTestId, unmount } = render(() => <Mascot state="happy" size={120} />);
    const img = getByTestId("mascot-img") as HTMLImageElement;
    expect(img.getAttribute("src")).toContain("xuannv-happy");
    unmount();
  });
  it("可戳：点击触发 onPoke", () => {
    const onPoke = vi.fn();
    const { getByTestId, unmount } = render(() => <Mascot state="idle" size={120} onPoke={onPoke} />);
    fireEvent.click(getByTestId("mascot"));
    expect(onPoke).toHaveBeenCalledOnce();
    unmount();
  });
});
```

- [ ] **Step 2: 跑红。**
- [ ] **Step 3: 实现 `Mascot.tsx`**：props `{ state: MascotStateKind; size: number; onPoke?: ()=>void; draggable?: boolean }`。内部 `frameSrc(state)` 映射到 `/mascot/xuannv-<state>.webp`（idle 与 blink 用于呼吸+眨眼，CSS 类 `breathe`，JS `setInterval` 随机切 blink 150ms）。`onPoke` 时加 `.wobble` class（`animationend` 移除）。`<img data-testid="mascot-img">` 包在 `data-testid="mascot"` 容器。`prefers-reduced-motion` 时不挂计时器。`.module.css` 写 breathe/wobble/float keyframes（曲线用 `var(--spring)`，参考 spec §4.2 + 视觉伴侣 demo 的 wobble）。`quips.ts` 导出 `QUIPS: string[]`（呆萌短语 10+ 条，如「在呢～」「戳坏了要赔的哦」）。
- [ ] **Step 4: 跑绿 + 截图 gate**（dev 里临时挂 `<Mascot state="idle" size={160}/>` 看呼吸眨眼 + 戳 wobble，人眼确认像桌宠不僵硬）。
- [ ] **Step 5: Commit** — `git commit -m "feat(im): Mascot 渲染组件（帧切换+呼吸眨眼+戳 wobble）"`

## Task 8: MascotController（事件 → state，全局共用）

**Files:**
- Create: `src/components/Mascot/MascotController.tsx`
- Test: `tests/unit/MascotController.test.tsx`

- [ ] **Step 1: 写失败测试** — provider 暴露 `mascotState()` signal 与 `dispatch(event)`/`poke()`；dispatch `task-done` 后 `mascotState().kind==="happy"`，约 1.8s（用 `vi.useFakeTimers`）后 settle 回 `idle`；`poke()` 后状态 poke 且从 `quips` 取一句 `quip()` 非空。
- [ ] **Step 2: 跑红。**
- [ ] **Step 3: 实现** — `createContext` + `MascotProvider`：内部 `createSignal<MascotState>`，`dispatch=(e)=>set(reduceMascot(get(),e))`，瞬时态 `dispatch` 后用 `setTimeout` 发 `settle`（happy 1.8s/poke .65s/surprise 1.2s/wave 1s）。idle 计时器 30s → `idle-timeout`。`poke()` = `dispatch({type:'poke'})` + 随机 `quip`。`useMascot()` hook。
- [ ] **Step 4: 跑绿。**
- [ ] **Step 5: 挂载** — `App.tsx` 在 `ApiProvider` 内包一层 `MascotProvider`（先只 provide，不接事件源；事件源在各页 task 接）。`npm run typecheck` 绿。
- [ ] **Step 6: Commit** — `git commit -m "feat(im): MascotController（事件映射+瞬时态归位+俏皮话）"`

## Task 9: 共享原语组件（一）—— Card / ScreenHeader / SectionLabel / StatePill

**Files:**
- Create: `src/components/ui/{Card,ScreenHeader,SectionLabel,StatePill}.tsx` + 各 `.module.css`
- Test: `tests/unit/ui-primitives.test.tsx`

- [ ] **Step 1: 写失败测试** — 每个组件渲染出对应 testid + 关键内容：`Card`（`data-testid="ui-card"`，应用 `u-card` 质感 class）；`ScreenHeader`（title + 可选 back/right slot，back 点击触发回调）；`SectionLabel`（大写小标签文本）；`StatePill`（按 `tone` 给不同背景 class：running/done/queued/warn）。
- [ ] **Step 2: 跑红。**
- [ ] **Step 3: 实现** 四个原语，全部用 §2 质感（Card 走 `u-card`；StatePill 圆 pill + 柔色；ScreenHeader 标题用 `u-title`，back 是 SVG `‹` + 文案，min-height 44）。
- [ ] **Step 4: 跑绿 + 截图 gate。**
- [ ] **Step 5: Commit** — `git commit -m "feat(im): UI 原语 Card/ScreenHeader/SectionLabel/StatePill"`

## Task 10: 共享原语组件（二）—— ListRow / Tile / ToggleRow

**Files:**
- Create: `src/components/ui/{ListRow,Tile,ToggleRow}.tsx` + `.module.css`
- Test: `tests/unit/ui-primitives2.test.tsx`

- [ ] **Step 1: 写失败测试** — `ListRow`（左彩色 SVG 图标槽 + 标题/副标 + 右 slot(pill/chevron)，整行可点触发 onClick，min-height 44）；`Tile`（图标+名+描述，宫格用，可点）；`ToggleRow`（标题 + 圆头开关，受控 `checked`/`onChange`，点击翻转）。
- [ ] **Step 2: 跑红 → Step 3 实现（质感：ListRow/Tile 走 `u-card`，图标槽圆角渐变底；ToggleRow 开关 spring 过渡）→ Step 4 跑绿 + 截图 gate → Step 5 Commit** `git commit -m "feat(im): UI 原语 ListRow/Tile/ToggleRow"`

## Task 11: Loading/空态原语 —— MascotLoader / Skeleton / EmptyState / TypingDots

**Files:**
- Create: `src/components/ui/{MascotLoader,Skeleton,EmptyState,TypingDots}.tsx` + `.module.css`
- Test: `tests/unit/ui-loading.test.tsx`

- [ ] **Step 1: 写失败测试** — `MascotLoader`（渲染 think 帧 mascot + 转圈星点容器 testid `mascot-loader`，可带文案）；`Skeleton`（占位块，`shimmer` class）；`EmptyState`（mascot + 标题 + 副文案 + 可选 action，testid `empty-state`）；`TypingDots`（三跳点 testid `typing-dots`）。
- [ ] **Step 2-5:** 跑红 → 实现（MascotLoader 用 `<Mascot state="think">` + CSS 轨道星点，spec §5；Skeleton shimmer 暖奶白扫光；EmptyState 用 idle/sleep mascot + 呆萌文案）→ 跑绿 + 截图 gate → Commit `git commit -m "feat(im): loading/空态原语（MascotLoader/Skeleton/EmptyState/TypingDots）"`

---

# Phase 2 · 壳层 / 导航模型 + 家 + 聊天

## Task 12: BottomTabBar → 4 tab（家/聊天/任务/更多）+ glass

**Files:**
- Modify: `src/components/BottomTabBar.tsx` + `.module.css`
- Modify: `tests/unit/BottomTabBar.test.tsx`（已存在，更新断言）

- [ ] **Step 1: 改测试** — tab key 改为 `["home","xuannv","tasks","more"]`，label `["家","聊天","任务","更多"]`；每个 tab 渲染 SVG 图标（testid `tab-<key>-icon`）；选中态 `aria-selected`；badge 仍支持。
- [ ] **Step 2: 跑红** — `npm test -- BottomTabBar` → FAIL。
- [ ] **Step 3: 实现** — `TabSpec.key` 类型扩为 `"home"|"xuannv"|"tasks"|"more"`；每 tab 渲染对应 SVG（home/chat/task/grid，复用视觉伴侣里那套 path）；`.module.css` 底栏加 `u-glass` + 顶部细描边；选中 peach（`--accent-deep`），未选 `--text-muted`；图标 20px、`stroke-linecap:round`。
- [ ] **Step 4: 跑绿 + 截图 gate（毛玻璃 + 圆胖图标）。**
- [ ] **Step 5: Commit** — `git commit -m "feat(im): 底栏 4 tab（家/聊天/任务/更多）+ 毛玻璃"`

## Task 13: 导航模型 —— ApiProvider tab 索引调整（通知并入家）

**Files:**
- Modify: `src/components/ApiProvider.tsx`
- Modify: `tests/unit/ApiProvider-nav.test.tsx`（已存在）

- [ ] **Step 1: 读现状** — 现 `TabIndex 0|1|2|3 = 玄女/任务/通知/更多`，`navTo(kind)` 注释里 project→tab3 等映射。新模型 `0=家 1=聊天(原玄女) 2=任务 3=更多`，通知不再是一级 tab（进家）。
- [ ] **Step 2: 改测试** — `navTo("project")` 仍落 tab3+moreSub；新增：`goHome()` → tab0；原本切到"通知 tab"的入口改为 `navPush({kind:"notifications"})` 或 home 内入口（按实现选一种，测试断言对应 state）。更新所有引用旧 index 语义的断言。
- [ ] **Step 3: 跑红 → Step 4 实现**（调整 TabIndex 语义注释 + 任何硬编码 index 的映射；新增 `notifications` 作为 navRoute 或 moreSub 入口，二选一并在注释写明）→ **Step 5 跑绿** `npm test -- ApiProvider` → PASS。
- [ ] **Step 6: Commit** — `git commit -m "refactor(im): 导航模型 4 tab（家/聊天/任务/更多），通知并入家"`

## Task 14: App.tsx MainShell 接 HomePage + tab 索引

**Files:**
- Modify: `src/App.tsx`
- Test: 复用 `tests/unit/ApiProvider-nav.test.tsx` + 新增 `tests/unit/App-shell.test.tsx`（断言 tab0 渲染 HomePage testid、tab1 渲染会话）

- [ ] **Step 1: 写失败测试** — 登录态下默认 tab0 → `getByTestId("page-home")` 存在；切 tab1 → 会话 stream 存在。
- [ ] **Step 2: 跑红**（HomePage 还没有）→ 先建占位 `HomePage`（`<div data-testid="page-home"/>`）让结构通，真实装在 Task 15。
- [ ] **Step 3: 改 `App.tsx`** — `MainShell` 的 `<Switch>`：`Match activeTab()===0 → <HomePage/>`；`===1 → <XuannvPage/>`（原会话）；`===2 → 任务`；`===3 → 更多`。`BASE_TABS` 改 4 项新 key/label。
- [ ] **Step 4: 跑绿（占位）+ typecheck。**
- [ ] **Step 5: Commit** — `git commit -m "refactor(im): MainShell 接家 tab + 索引迁移（HomePage 占位）"`

## Task 15: HomePage（客厅主屏）实装

**Files:**
- Create/Modify: `src/views/pages/HomePage.tsx` + `.module.css`
- Test: `tests/unit/HomePage.test.tsx`

- [ ] **Step 1: 写失败测试** — 给定 mock api（`fetchTasksOverview` 返回 N running、`fetchNotifications` 返回 M 未读），HomePage 渲染：mascot hero（testid `home-mascot`）、问候语含名字、状态条文本含 `N`/`M`、4 个快捷入口卡（找玄女聊/任务/通知/交付）。点"找玄女聊" → 调 `setActiveTab(1)`。点 hero → 调 `useMascot().poke`（spy）。
- [ ] **Step 2: 跑红。**
- [ ] **Step 3: 实现** — 用原语：`u-mesh u-noise` 背景；`<Mascot>` hero（接 MascotController state，进页 dispatch `greet`）；问候按时段（早/午/晚 + "以琳"）；状态条来自 `createResource(fetchTasksOverview/fetchNotifications)`；快捷入口 4 张 `Tile`/`Card`；点击走 `setActiveTab`/`navPush`。空/loading 用 `Skeleton`。
- [ ] **Step 4: 跑绿 + 截图 gate（客厅整体质感复检，§2）。**
- [ ] **Step 5: Commit** — `git commit -m "feat(im): 家·客厅主屏（mascot hero+问候+状态+快捷入口）"`

## Task 16: 聊天页 Conversation + composer 重绘 + mini mascot

**Files:**
- Modify: `src/views/Conversation.tsx` + `.module.css`，`src/views/pages/XuannvPage.tsx`，`src/components/Composer*.tsx`+css、`MentionComposer`/`MentionAutocomplete`/`MentionChip` css
- Test: 复用 `tests/unit/Conversation.test.tsx`（更新空态文案断言如改动）

- [ ] **Step 1:** 若空态文案/结构改了先改测试，否则保持。接 MascotController：流式中 `dispatch('stream-start')`、结束 `stream-end`。
- [ ] **Step 2: 跑红（若改了测试）→ Step 3 重绘** — 消息流背景 `u-mesh`；composer 走 `u-card` 圆胖输入 + 蜜桃发送按钮（spring 按压）；聊天页右下挂可拖 `<Mascot size={56} draggable>`（mini）；流式时 mini 切 talk + `TypingDots`。
- [ ] **Step 4: 跑绿 + 截图 gate。**
- [ ] **Step 5: Commit** — `git commit -m "feat(im): 聊天页奶油糖果重绘 + composer + 悬浮 mini 吉祥物"`

## Task 17: 消息族组件重绘（bubbles + tool cards + rows）

**Files:**
- Modify: `src/components/messages/*`（`UserBubble`/`XuannvBubble`/`WorkerBubble`/`ToolCallCard`/`ToolGroupCard`/`ThinkingRow`/`StreamingText`/`FileMessage`/`InlineFileCard`/`AttachmentChip`/`StatusMarkerRow`/`SystemMessageRow`）的 `.module.css`（+ 必要结构）
- Test: 复用 `tests/unit/messages*.test.ts`、`AttachmentChip.test.tsx` 等（结构不变则只换样式，测试应仍绿）

- [ ] **Step 1:** 先跑现有消息测试基线绿。
- [ ] **Step 2: 逐组件换样式** — 用户气泡 `--user-bubble`(白)+柔影；玄女气泡浅紫 lavender 调；门客气泡浅色 + role 色描边；tool card `u-card` + 圆角等宽内容区；ThinkingRow 用 `TypingDots`/think 微动。保持 testid 不变。
- [ ] **Step 3: 跑绿（结构没动测试自然过）+ 截图 gate（聊天整屏多类型消息混排）。**
- [ ] **Step 4: Commit** — `git commit -m "feat(im): 消息族（气泡/工具卡/状态行）奶油糖果重绘"`

---

# Phase 3 · 逐页按原型重绘

> 每页通用配方（DRY）：① 若结构/文案改→先改对应 `tests/unit/*` 断言（多数只换样式则跳过）② 把页面 JSX 重构成由 Phase 1 原语（ScreenHeader/Card/ListRow/Tile/ToggleRow/StatePill/SectionLabel/EmptyState/Skeleton）组合 ③ `.module.css` 换 token + 质感 ④ 跑 `npm test`/`typecheck` 绿 ⑤ **截图 gate**（§2 人眼）⑥ commit。每页一个 task。

## Task 18: 任务列表 TasksPage（原型 A）
**Files:** `src/views/pages/TasksPage.tsx`+css；Test 复用/新增 `tests/unit/TasksPage.test.tsx`
- [ ] 按配方：列表用 `ListRow`（左 role 色图标 + 任务名/门客 + 右 `StatePill` 状态）；空态 `EmptyState`（mascot + "还没有任务～"）；loading `Skeleton`。截图 gate。Commit `feat(im): 任务列表 A 型重绘`。

## Task 19: 任务线程 TaskThreadPage（原型 B·线程）
**Files:** `src/views/pages/TaskThreadPage.tsx`+css
- [ ] `ScreenHeader`(back+任务名+成员菜单)；顶部任务横幅 `Card`（进度/门客/StatePill）；消息流复用 Phase 2 消息族样式；底部"介入"输入（复用 composer 样式）。成员菜单 overlay 换 `u-glass`+圆角。截图 gate。Commit `feat(im): 任务线程 B 型重绘`。

## Task 20: 通知 NotificationsPage（原型 A）+ 家入口联动
**Files:** `src/views/pages/NotificationsPage.tsx`+css
- [ ] `ListRow` 列表（未读高亮 + 时间）；已读置灰；空态 EmptyState。确认从家点"通知"能进（Task 13 入口）。截图 gate。Commit。

## Task 21: 更多 hub MorePage（原型 C）
**Files:** `src/views/pages/MorePage.tsx`+css；Test 复用 `tests/unit/MorePage.test.tsx`
- [ ] 2 列 `Tile` 宫格（节点/项目/工作者/交付/记忆/角色/更漏/设置，各配彩色 SVG 图标槽）。testid 保持 `more-tile-*`。截图 gate。Commit。

## Task 22: 节点 NodesPage（A + 添加 modal）
**Files:** `src/views/pages/NodesPage.tsx`+css
- [ ] `ScreenHeader`(标题+「添加」) + `ListRow`(状态点 在线/离线 + 负载 + StatePill)；添加 modal 换 `u-card`/`u-glass` scrim。截图 gate。Commit。

## Task 23: 项目 ProjectsPage + ProjectDetailPage（A + B）
**Files:** `src/views/pages/ProjectsPage.tsx`+css、`ProjectDetailPage.tsx`+css
- [ ] 列表 A（项目卡 + 空态"暂无项目"用 EmptyState）；详情 B（摘要卡 + 分组内容）。截图 gate。Commit。

## Task 24: 交付 DeliverablesPage + DeliverableDetailPage（A + B）
**Files:** 两文件+css；Test 复用 `DeliverablesPage.test.tsx`/`DeliverableDetailPage.test.tsx`
- [ ] 列表 A（交付卡 + 空态"门客把活做完会在这里出现"）；详情 B（摘要卡 + 时间线 + 底部"下载"主按钮）。保持 testid。截图 gate。Commit。

## Task 25: 工作者 WorkerPage（B·线程）
**Files:** `src/views/pages/WorkerPage.tsx`+css；Test 复用 `WorkerPage.test.tsx`
- [ ] `ScreenHeader`(role) + 任务横幅 banner `Card` + thread（复用消息族）+ composer。保持 testid。截图 gate。Commit。

## Task 26: 记忆 MemoryPage（分层）
**Files:** `src/views/pages/MemoryPage.tsx`+css
- [ ] `SectionLabel` 分层（身份卡=紫调 Card / 事实策府=Card 列表）；空态 EmptyState。截图 gate。Commit。

## Task 27: 角色 RolesPage（能力卡）
**Files:** `src/views/pages/RolesPage.tsx`+css
- [ ] role `Card`（头像 role 色 + 名 + tier StatePill + 能力 chip 组）。截图 gate。Commit。

## Task 28: 更漏 CronPage（A）
**Files:** `src/views/pages/CronPage.tsx`+css
- [ ] `ListRow`(intent/kind/summary/meta，失败态 danger 色)；空态 EmptyState。截图 gate。Commit。

## Task 29: 设置 SettingsPage（D·表单）
**Files:** `src/views/pages/SettingsPage.tsx`+css
- [ ] `SectionLabel` 分组 + `ToggleRow`（推送/门客完成提醒/夜间免打扰/桌宠动效/戳一戳俏皮话）+ 值行（版本）。"桌宠动效"开关接 reduced-motion override（关→静态）。截图 gate。Commit。

## Task 30: 登录 LoginView
**Files:** `src/components/LoginView.tsx`+css
- [ ] `u-mesh` 背景 + mascot wave 欢迎 + 圆胖输入 + 蜜桃主按钮。截图 gate。Commit。

## Task 31: 公共骨架 NavigationStack / MoreSubShell / TopicSidebar / Toast / modal
**Files:** 对应 `.tsx`/`.module.css`
- [ ] NavigationStack 转场（soft fade+slide，spring，方向感，spec §5）；MoreSubShell 返回头用 ScreenHeader 风；TopicSidebar drawer 换奶油 + `u-glass`；Toast 圆胖 + 柔影 + role 色；归档/确认 modal 重新上色（保持已有 IM modal 行为）。截图 gate（逐个）。Commit `feat(im): 公共骨架（转场/抽屉/toast/modal）重绘`。

---

# Phase 4 · Loading/空态/转场打磨 + PWA 资源 + e2e + 部署

## Task 32: 全局事件 → MascotController 接线复检
**Files:** 跨页（家/聊天/任务/通知）
- [ ] 通读：流式→talk、门客在跑→think、任务完成/交付到达→happy、报错(toast error)→surprise、进家→greet/wave 均已 dispatch。补缺的接线。新增 `tests/unit/mascot-wiring.test.tsx` 抽测关键 2-3 条（如 toast error → controller 收到 `error`）。跑绿。Commit `feat(im): 吉祥物全局事件接线复检`。

## Task 33: PWA icon / splash / manifest 重做
**Files:** `public/icons/*`、`vite.config.ts`(PWA manifest theme_color/background_color)、`index.html`
- [ ] 用 mascot 头像生成 192/512/512-maskable + `icon.svg`（可由 idle 帧裁头 + 圆底脚本，或单独 gpt-image-2 头像）；manifest `theme_color:#FFB877` `background_color:#FFF8F0`。看图 gate。Commit `feat(im): PWA 图标/启动/主题色奶油化`。

## Task 34: e2e（Playwright）回归
**Files:** `tests/e2e/*.spec.ts`
- [ ] 更新/新增：4-tab 导航（家↔聊天↔任务↔更多）、家→通知、戳吉祥物有 wobble/state 变化、空态出现、loading 出现。跑 `npm run e2e`（先 `npm run e2e:install`）。修红。Commit `test(im): e2e 覆盖新导航+吉祥物`。

## Task 35: 全量门禁 + 旧 token 别名清理
**Files:** 全仓
- [ ] 删 Task 1 留的过渡别名，grep 残留旧字段引用补齐；`npm run lint && npm run typecheck && npm test && npm run e2e` 全绿；`npm run build` 成功。Commit `chore(im): 清理过渡 token 别名 + 全量门禁绿`。

## Task 36: 全局质感终检（§2 人眼）+ 安卓壳
**Files:** `apps/fuxi-im-android`（sync only）
- [ ] 逐屏 Playwright 截图（移动视口 390×844）过一遍 §2 清单（柔影/噪点/网状渐变/暖光/毛玻璃/无 emoji/无塑料色块），不达标的页回对应 task 修。`apps/fuxi-im-android` 重新 `npm run build` + `npx cap sync`（无原生改动）。Commit `chore(im): 全局质感终检 + 安卓壳同步`。

## Task 37: home 部署 + 手机实测
- [ ] 按 CLAUDE.md 部署流程：commit+push 分支 → merge 到 main(`--no-ff`) → push origin → rsync web/build 产物 + 重启 `fuxi-im.service`（web 资源部署，按 `reference_home_deploy`）→ 手机开 PWA 实测核心路径（家→聊天发消息→看任务→戳吉祥物→收通知）。记录问题。

---

## Self-Review（已对 spec 核过）
- **spec §2 质感**：贯穿——Task 3 建 utility，每个可视 task 有截图 gate，Task 36 终检。✓
- **§3 token**：Task 1（ts）+ Task 2（css）。✓
- **§4 吉祥物**：资源 Task 5、状态机 Task 6、组件 Task 7、控制器 Task 8、全局接线 Task 32。✓
- **§5 loading/空态/转场**：原语 Task 11、转场 Task 31、接入各页。✓
- **§6 IA/原型**：底栏 Task 12、导航模型 Task 13-14、家 Task 15、聊天 Task 16-17、A/B/C/D 各页 Task 18-31。✓ 全部 spec §6.2 表内页面都有对应 task。
- **§7 技术/§8 测试**：TDD 贯穿、e2e Task 34、home 实测 Task 37。✓
- **类型一致**：`reduceMascot`/`MascotState`/`MascotStateKind`/`useMascot().poke`/`dispatch` 在 Task 6/7/8 定义并在 15/16/32 复用，命名一致。✓
- **无占位**：逻辑 task（1/6/7/8）给了完整代码；纯 CSS 重绘 task 给了文件路径+原语组合+测试+截图 gate（视觉质量按 §2 由 executor 实现并人眼验收，非编造 CSS）。

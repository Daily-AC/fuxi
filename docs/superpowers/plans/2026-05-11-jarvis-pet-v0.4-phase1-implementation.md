# Jarvis-Pet v0.4 Phase 1 MVP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `apps/jarvis-pet/` 起 Tauri 2.0 桌宠骨架，实现"玄女悬浮屏上呼吸 + 可拖动 + 收 fuxi /api/conv 事件实时切 sprite + 6 维数值 store + 1 个 GraphType (Default 6 帧) 资产验证 sprite pipeline"。

**Architecture:** 新建 `apps/jarvis-pet/`（Tauri 2.0 + Vue 3 + TS + PixiJS v8）。Rust backend 用 tauri_nspanel 解 macOS 透明常驻 + dock 隐藏；frontend Vue + PixiJS 渲染 sprite。fuxi-im /api/conv WS 走 ts 客户端订阅，EventKind → 6 维数值 store。本期不接语音（apps/jarvis Swift 暂不动）。

**Tech Stack:** Tauri 2.0 / Vue 3 / TypeScript / Vite / PixiJS v8 / Pinia / tauri_nspanel / tauri-plugin-macos-permissions / Vitest

**Spec ref:** `docs/superpowers/specs/2026-05-11-jarvis-pet-v0.4-design.md`

---

## File Map

新增（apps/jarvis-pet/ 下）：

**Tauri 后端**（Rust）：
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json` + `src-tauri/tauri.macos.conf.json`
- `src-tauri/build.rs`
- `src-tauri/src/main.rs` + `src-tauri/src/lib.rs`
- `src-tauri/src/core/setup.rs` + `src-tauri/src/core/setup/macos.rs`

**前端**（Vue 3 + TS）：
- `package.json` / `vite.config.ts` / `tsconfig.json` / `index.html`
- `src/main.ts` / `src/App.vue`
- `src/components/PetCanvas.vue`
- `src/pixi/PixiApp.ts`
- `src/sprites/lpsParser.ts` + tests
- `src/sprites/SpriteSet.ts` + tests
- `src/sprites/AnimationPlayer.ts`
- `src/stores/stats.ts` + tests
- `src/api/fuxiClient.ts`
- `src/behavior/statsMapper.ts` + tests
- `src/types/event.ts`

**资产**：
- `resources/sprites/xuannv/manifest.lps`
- `resources/sprites/xuannv/default/default_001_120.png` ... `default_006_120.png`

**根目录改动**：
- `apps/jarvis-pet/README.md`

---

## Critical Pre-Task Setup

**分支**：当前 `feat/jarvis-pet-v0.4`，全程在此分支干。

**前置工具检查**（worker 开工前在 mac 跑一遍）：
```bash
node --version    # 期望 ≥ 20
pnpm --version || npm --version    # pnpm 优先，没有用 npm
cargo --version   # rustc stable
xcode-select -p   # /Applications/Xcode.app/Contents/Developer or /Library/Developer/CommandLineTools
```

**装 Tauri CLI**（一次）：
```bash
cargo install create-tauri-app --locked    # 用于 scaffold
cargo install tauri-cli --version "^2.0" --locked
```

**Build / Run 命令**（每 task 验证用）：
```bash
cd apps/jarvis-pet
npm install                        # 装 npm deps
cargo build --manifest-path src-tauri/Cargo.toml    # 单测 Rust
npm run tauri dev                  # 起 dev 看效果（需 mac GUI）
npm test                           # vitest 跑前端单测
npm run tauri build                # 出 release .app
```

**Commit 风格**：`feat(jarvis-pet): ...` / `chore(jarvis-pet): ...` / `test(jarvis-pet): ...`，commit per task。

---

## Task 1: Bootstrap Tauri 工程结构

**Files:**
- Create: `apps/jarvis-pet/` 整个目录骨架
- Create: `apps/jarvis-pet/package.json`
- Create: `apps/jarvis-pet/vite.config.ts`
- Create: `apps/jarvis-pet/tsconfig.json` + `tsconfig.node.json`
- Create: `apps/jarvis-pet/index.html`
- Create: `apps/jarvis-pet/src/main.ts`
- Create: `apps/jarvis-pet/src/App.vue`
- Create: `apps/jarvis-pet/.gitignore`

**No dependencies.** 起手任务。

- [ ] **Step 1: 创建目录骨架**

```bash
mkdir -p apps/jarvis-pet/src apps/jarvis-pet/src-tauri/src apps/jarvis-pet/resources/sprites/xuannv/default
```

- [ ] **Step 2: 写 package.json**

`apps/jarvis-pet/package.json`：

```json
{
  "name": "jarvis-pet",
  "private": true,
  "version": "0.4.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "vue-tsc --noEmit && vite build",
    "preview": "vite preview",
    "tauri": "tauri",
    "test": "vitest run",
    "test:watch": "vitest"
  },
  "dependencies": {
    "vue": "^3.5.0",
    "pinia": "^2.2.0",
    "pixi.js": "^8.5.0",
    "@tauri-apps/api": "^2.0.0",
    "@tauri-apps/plugin-os": "^2.0.0"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2.0.0",
    "@vitejs/plugin-vue": "^5.0.0",
    "@vue/test-utils": "^2.4.0",
    "happy-dom": "^15.0.0",
    "typescript": "^5.6.0",
    "vite": "^5.4.0",
    "vitest": "^2.1.0",
    "vue-tsc": "^2.1.0"
  }
}
```

- [ ] **Step 3: 写 tsconfig.json**

`apps/jarvis-pet/tsconfig.json`：

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "useDefineForClassFields": true,
    "module": "ESNext",
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "skipLibCheck": true,
    "moduleResolution": "bundler",
    "allowImportingTsExtensions": true,
    "isolatedModules": true,
    "moduleDetection": "force",
    "noEmit": true,
    "jsx": "preserve",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true,
    "baseUrl": ".",
    "paths": { "@/*": ["src/*"] },
    "types": ["vitest/globals"]
  },
  "include": ["src/**/*.ts", "src/**/*.tsx", "src/**/*.vue"],
  "references": [{ "path": "./tsconfig.node.json" }]
}
```

`apps/jarvis-pet/tsconfig.node.json`：

```json
{
  "compilerOptions": {
    "composite": true,
    "skipLibCheck": true,
    "module": "ESNext",
    "moduleResolution": "bundler",
    "allowSyntheticDefaultImports": true
  },
  "include": ["vite.config.ts"]
}
```

- [ ] **Step 4: 写 vite.config.ts**

`apps/jarvis-pet/vite.config.ts`：

```typescript
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import path from 'node:path'

// Tauri 期望前端跑在 1420 端口（hardcoded in tauri.conf.json）
export default defineConfig(async () => ({
  plugins: [vue()],
  resolve: {
    alias: { '@': path.resolve(__dirname, './src') }
  },
  // Tauri 推荐配置：禁清屏，固定 host/port，wsl-friendly HMR
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: '127.0.0.1',
    hmr: { protocol: 'ws', host: '127.0.0.1', port: 1421 }
  },
  // Vitest 配置内嵌
  test: {
    globals: true,
    environment: 'happy-dom'
  }
}))
```

- [ ] **Step 5: 写 index.html + src/main.ts + src/App.vue**

`apps/jarvis-pet/index.html`：

```html
<!DOCTYPE html>
<html lang="zh-Hans">
  <head>
    <meta charset="UTF-8" />
    <title>玄女</title>
    <style>
      html, body { margin: 0; padding: 0; background: transparent; overflow: hidden; }
      #app { width: 100vw; height: 100vh; }
    </style>
  </head>
  <body>
    <div id="app"></div>
    <script type="module" src="/src/main.ts"></script>
  </body>
</html>
```

`apps/jarvis-pet/src/main.ts`：

```typescript
import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'

createApp(App).use(createPinia()).mount('#app')
```

`apps/jarvis-pet/src/App.vue`：

```vue
<script setup lang="ts">
// Phase 1 placeholder——T11 替换为 PetCanvas
</script>

<template>
  <div class="placeholder">玄女</div>
</template>

<style scoped>
.placeholder {
  width: 280px;
  height: 420px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-family: serif;
  font-size: 24px;
  color: #6E8896;
  background: rgba(250, 247, 241, 0.08);
}
</style>
```

- [ ] **Step 6: 写 .gitignore**

`apps/jarvis-pet/.gitignore`：

```
node_modules/
dist/
src-tauri/target/
src-tauri/Cargo.lock
.DS_Store
*.log
```

- [ ] **Step 7: 装 npm deps + 验证 vite 跑**

```bash
cd apps/jarvis-pet && npm install
npm run build
```

Expected: `dist/index.html` + assets 生成，无 error（warning OK）。

- [ ] **Step 8: Commit**

```bash
git add apps/jarvis-pet/package.json apps/jarvis-pet/vite.config.ts \
        apps/jarvis-pet/tsconfig.json apps/jarvis-pet/tsconfig.node.json \
        apps/jarvis-pet/index.html apps/jarvis-pet/src/main.ts apps/jarvis-pet/src/App.vue \
        apps/jarvis-pet/.gitignore
git commit -m "feat(jarvis-pet): bootstrap Tauri 工程骨架（Vue 3 + Vite + TS）"
```

⚠️ 不要 `git add apps/jarvis-pet/node_modules` 或 `package-lock.json`/`pnpm-lock.yaml` ——前者已 ignore，lock 文件留给 worker 起 npm install 时本地生成（lock 文件会随 npm 版本变，CI 跑 install 时重新解算）。

---

## Task 2: src-tauri Rust 工程 + Cargo.toml

**Files:**
- Create: `apps/jarvis-pet/src-tauri/Cargo.toml`
- Create: `apps/jarvis-pet/src-tauri/build.rs`
- Create: `apps/jarvis-pet/src-tauri/src/main.rs`
- Create: `apps/jarvis-pet/src-tauri/src/lib.rs`

**Depends on:** Task 1.

- [ ] **Step 1: 写 src-tauri/Cargo.toml**

```toml
[package]
name = "jarvis-pet"
version = "0.4.0"
edition = "2021"
description = "玄女桌宠 v0.4"

[lib]
name = "jarvis_pet_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = ["macos-private-api"] }
tauri-plugin-os = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
log = "0.4"
env_logger = "0.11"

[target.'cfg(target_os = "macos")'.dependencies]
# tauri_nspanel —— BongoCat 同款，把 Tauri NSWindow 桥到 NSPanel（non-activating + floating）
# 注意：crates.io 上的版本可能滞后，BongoCat 用 git pin。worker 跑 `cargo build` 报错时
# 退回 git: tauri-nspanel = { git = "https://github.com/ahkohd/tauri-nspanel", branch = "v2" }
tauri-nspanel = "2"

[features]
default = ["custom-protocol"]
custom-protocol = ["tauri/custom-protocol"]
```

- [ ] **Step 2: 写 build.rs**

```rust
fn main() {
    tauri_build::build()
}
```

- [ ] **Step 3: 写 src/main.rs**

```rust
// release 时不弹 Windows console；macOS 上 noop
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    jarvis_pet_lib::run()
}
```

- [ ] **Step 4: 写 src/lib.rs**

```rust
//! Tauri app builder——本期 Phase 1 只做最小骨架：
//! - 起 Tauri Builder
//! - 装 plugin-os
//! - macOS 上调 setup::macos::setup() 把主窗口转 NSPanel + 隐藏 dock

mod core;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_os::init())
        .setup(|app| {
            // macOS 专属 setup —— 转 NSPanel + dock 隐藏
            #[cfg(target_os = "macos")]
            {
                core::setup::macos::setup(app)?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("启动 jarvis-pet 失败");
}
```

- [ ] **Step 5: 验证 cargo check**

```bash
cd apps/jarvis-pet/src-tauri && cargo check 2>&1 | tail -10
```

Expected: cargo 报"core/mod.rs not found" —— Task 3 创建 core/ 模块就修。当前继续。

- [ ] **Step 6: Commit**

```bash
git add apps/jarvis-pet/src-tauri/Cargo.toml \
        apps/jarvis-pet/src-tauri/build.rs \
        apps/jarvis-pet/src-tauri/src/main.rs \
        apps/jarvis-pet/src-tauri/src/lib.rs
git commit -m "feat(jarvis-pet): src-tauri Rust 工程骨架 + tauri-nspanel 依赖"
```

---

## Task 3: macOS NSPanel setup + 透明 + dock 隐藏

**Files:**
- Create: `apps/jarvis-pet/src-tauri/src/core/mod.rs`
- Create: `apps/jarvis-pet/src-tauri/src/core/setup/mod.rs`
- Create: `apps/jarvis-pet/src-tauri/src/core/setup/macos.rs`

**Depends on:** Task 2.

- [ ] **Step 1: 写 src/core/mod.rs**

```rust
pub mod setup;
```

- [ ] **Step 2: 写 src/core/setup/mod.rs**

```rust
#[cfg(target_os = "macos")]
pub mod macos;
```

- [ ] **Step 3: 写 src/core/setup/macos.rs**

```rust
//! macOS 专属窗口策略：
//! - 主窗口转 NSPanel（non-activating + floating，跟 BongoCat 同款）
//! - dock 不显示 app icon
//! - 跨 Space 跟随
//!
//! 抄 BongoCat src-tauri/src/core/setup/macos.rs（MIT）

use tauri::{App, Manager};
use tauri_nspanel::{ManagerExt, WebviewWindowExt};

pub fn setup(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    // 1. 隐藏 dock icon —— accessory app，不进 dock 不进 cmd-tab
    app.set_dock_visibility(false);

    // 2. 主窗口转 NSPanel
    let main = app.get_webview_window("main")
        .ok_or("找不到 main window —— tauri.conf.json 里 windows 数组缺")?;
    let panel = main.to_panel()?;

    // 3. NSPanel 行为：
    //   - non-activating：点击不让 jarvis-pet 抢焦点
    //   - floating level：浮在 dock 之上但不压菜单栏
    //   - canJoinAllSpaces + stationary：切 Space 跟随
    use tauri_nspanel::raw_nspanel::cocoa::appkit::{
        NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    panel.set_level(7); // NSFloatingWindowLevel = 3, NSStatusWindowLevel = 25; 7 ≈ pop-up menu level，足够浮
    let mut style = NSWindowStyleMask::empty();
    style.insert(NSWindowStyleMask::NSNonactivatingPanelMask);
    style.insert(NSWindowStyleMask::NSBorderlessWindowMask);
    panel.set_style_mask(style.bits() as i32);

    let mut behavior = NSWindowCollectionBehavior::empty();
    behavior.insert(NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces);
    behavior.insert(NSWindowCollectionBehavior::NSWindowCollectionBehaviorStationary);
    behavior.insert(NSWindowCollectionBehavior::NSWindowCollectionBehaviorIgnoresCycle);
    panel.set_collection_behavior(behavior.bits());

    Ok(())
}
```

⚠️ tauri-nspanel API 在 v2 还在演进，上面 set_level/set_style_mask 的具体 method 名 worker 实装时**对照 crate docs.rs/tauri-nspanel 现状调整**。如果 API 不同，按 docs.rs 上的真接口写——核心是"转 NSPanel + non-activating + floating + canJoinAllSpaces"四件事。

- [ ] **Step 4: cargo check 验证**

```bash
cd apps/jarvis-pet/src-tauri && cargo check 2>&1 | tail -10
```

Expected: 编译过；可能有 nspanel API 名字差异 → 按 docs.rs 调。

- [ ] **Step 5: Commit**

```bash
git add apps/jarvis-pet/src-tauri/src/core/
git commit -m "feat(jarvis-pet): macOS NSPanel setup（non-activating + floating + canJoinAllSpaces）"
```

---

## Task 4: tauri.conf.json + macos 专属 conf

**Files:**
- Create: `apps/jarvis-pet/src-tauri/tauri.conf.json`
- Create: `apps/jarvis-pet/src-tauri/tauri.macos.conf.json`
- Create: `apps/jarvis-pet/src-tauri/icons/icon.png` (placeholder)

**Depends on:** Task 2.

- [ ] **Step 1: 写 tauri.conf.json（共通）**

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Xuannv",
  "version": "0.4.0",
  "identifier": "cn.qmledmq.fuxi.jarvis-pet",
  "build": {
    "beforeDevCommand": "npm run dev",
    "beforeBuildCommand": "npm run build",
    "devUrl": "http://127.0.0.1:1420",
    "frontendDist": "../dist"
  },
  "app": {
    "macOSPrivateApi": true,
    "windows": [
      {
        "label": "main",
        "title": "玄女",
        "width": 280,
        "height": 420,
        "transparent": true,
        "decorations": false,
        "alwaysOnTop": true,
        "skipTaskbar": true,
        "shadow": false,
        "resizable": false,
        "focus": false,
        "acceptFirstMouse": true
      }
    ]
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": ["icons/icon.png"],
    "resources": ["../resources/sprites/**/*"]
  }
}
```

- [ ] **Step 2: 写 tauri.macos.conf.json**（macOS 平台 override）

```json
{
  "bundle": {
    "macOS": {
      "frameworks": [],
      "minimumSystemVersion": "14.0"
    }
  }
}
```

- [ ] **Step 3: 占位图标**

Tauri 不让缺 icon。生成最简 1×1 透明 PNG 占位：

```bash
mkdir -p apps/jarvis-pet/src-tauri/icons && \
printf '\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\rIDATx\x9cc\xfc\xff\x00\x00\x05\x00\x01\x07\x9d\xae\xed\x00\x00\x00\x00IEND\xaeB`\x82' \
> apps/jarvis-pet/src-tauri/icons/icon.png
```

- [ ] **Step 4: Commit**

```bash
git add apps/jarvis-pet/src-tauri/tauri.conf.json \
        apps/jarvis-pet/src-tauri/tauri.macos.conf.json \
        apps/jarvis-pet/src-tauri/icons/icon.png
git commit -m "feat(jarvis-pet): tauri.conf.json 透明 + alwaysOnTop + nspanel 行为"
```

---

## Task 5: dev 模式 smoke run（验证空白透明窗能起）

**Files:**
- 不写代码，验证 Task 1-4 集成

**Depends on:** Task 1, 2, 3, 4.

- [ ] **Step 1: dev run**

```bash
cd apps/jarvis-pet && npm install   # 第一次跑装 deps
npm run tauri dev
```

Expected: 280×420 透明窗悬浮屏上，写"玄女"两字（来自 App.vue placeholder）。

- [ ] **Step 2: 手动检查清单**

- 窗口透明（背景看得到桌面）
- 窗口在最上层（开别的 app 仍盖不住它）
- dock 上没有 jarvis-pet 图标
- cmd+tab 切窗口不出现 jarvis-pet
- 切换 Space 桌宠跟随

- [ ] **Step 3: 任一项不通过 → 停下来 SendMessage 给 team-lead 报告**

⚠️ 验证不通过时**不要硬推下一 task**——前 4 task 集成出问题了，后面 task 全废。

- [ ] **Step 4: 通过则 commit 一条 chore 标记**（无代码改动也 commit 让历史能 grep）

```bash
git commit --allow-empty -m "chore(jarvis-pet): Phase 1 smoke 1 通过——透明 NSPanel 起来"
```

---

## Task 6: PixiJS 初始化 + canvas mount

**Files:**
- Create: `apps/jarvis-pet/src/pixi/PixiApp.ts`

**Depends on:** Task 1.

- [ ] **Step 1: 写 PixiApp.ts**

`apps/jarvis-pet/src/pixi/PixiApp.ts`：

```typescript
import { Application } from 'pixi.js'

/// PixiJS Application 单例 —— 跟 Vue 组件解耦，便于测试 mock。
/// 初始化后 .canvas 是 HTMLCanvasElement，挂到 Vue 容器即可显示。
export class PixiApp {
  private app: Application | null = null

  async init(opts: { width: number; height: number }): Promise<HTMLCanvasElement> {
    if (this.app) {
      throw new Error('PixiApp 已初始化')
    }
    this.app = new Application()
    await this.app.init({
      width: opts.width,
      height: opts.height,
      // 透明背景：让 Tauri 透明窗口的桌面背景透出来
      backgroundAlpha: 0,
      antialias: true,
      // mac WebKit 性能：preferWebGL2，不用 WebGPU（mac WebKit WebGPU 还不稳）
      preference: 'webgl',
      // 抗锯齿权衡：开了清晰但拖动时性能 -10%；桌宠主要静态可接受
      resolution: window.devicePixelRatio || 1,
      autoDensity: true
    })
    return this.app.canvas
  }

  get pixi(): Application {
    if (!this.app) throw new Error('PixiApp 未初始化')
    return this.app
  }

  destroy(): void {
    if (this.app) {
      this.app.destroy(true, { children: true, texture: true })
      this.app = null
    }
  }
}
```

- [ ] **Step 2: 验证 build**

```bash
cd apps/jarvis-pet && npm run build 2>&1 | tail -5
```

Expected: 编译过 (vue-tsc 不报)。

- [ ] **Step 3: Commit**

```bash
git add apps/jarvis-pet/src/pixi/PixiApp.ts
git commit -m "feat(jarvis-pet): PixiApp 初始化封装（透明背景 + WebGL）"
```

---

## Task 7: LPS parser

**Files:**
- Create: `apps/jarvis-pet/src/sprites/lpsParser.ts`
- Create: `apps/jarvis-pet/src/sprites/lpsParser.spec.ts`

**Depends on:** Task 1.

- [ ] **Step 1: 写失败测试**

`apps/jarvis-pet/src/sprites/lpsParser.spec.ts`：

```typescript
import { describe, it, expect } from 'vitest'
import { parseLps } from './lpsParser'

describe('parseLps', () => {
  it('解析单 section', () => {
    const input = `[character]
name: 玄女
author: fuxi
version: 0.4`
    const result = parseLps(input)
    expect(result).toEqual([
      { section: 'character', fields: { name: '玄女', author: 'fuxi', version: '0.4' } }
    ])
  })

  it('解析多 section（同名 section 算两条独立）', () => {
    const input = `[pnganimation]
graph: Default
animat: Single
mode: Normal
path: ./default
loop: true

[pnganimation]
graph: Touch_Head
animat: A_Start
mode: Happy
path: ./touch_head/happy
loop: false`
    const result = parseLps(input)
    expect(result).toHaveLength(2)
    expect(result[0].section).toBe('pnganimation')
    expect(result[0].fields.graph).toBe('Default')
    expect(result[1].fields.graph).toBe('Touch_Head')
    expect(result[1].fields.loop).toBe('false')
  })

  it('忽略空行 + # 注释', () => {
    const input = `# 这是注释
[character]
# 段内注释
name: 玄女

# 末尾注释`
    const result = parseLps(input)
    expect(result).toEqual([
      { section: 'character', fields: { name: '玄女' } }
    ])
  })

  it('field 值可以含冒号（只切第一个）', () => {
    const input = `[meta]
url: https://example.com/path:1234`
    const result = parseLps(input)
    expect(result[0].fields.url).toBe('https://example.com/path:1234')
  })

  it('段外内容报错', () => {
    const input = `name: 玄女`
    expect(() => parseLps(input)).toThrow(/段外/)
  })
})
```

- [ ] **Step 2: 跑测试看 fail**

```bash
cd apps/jarvis-pet && npm test -- lpsParser
```

Expected: FAIL, "Cannot find module './lpsParser'"

- [ ] **Step 3: 实装 lpsParser.ts**

`apps/jarvis-pet/src/sprites/lpsParser.ts`：

```typescript
/// VPet info.lps 文本格式 parser。
/// 格式：[section] 后跟 key: value 行；同一个 [section] 名出现多次算独立 entry。
/// 行首 # 是注释；空行忽略。值里的 ':' 不切（只切第一个）。

export interface LpsSection {
  section: string
  fields: Record<string, string>
}

export function parseLps(input: string): LpsSection[] {
  const result: LpsSection[] = []
  let current: LpsSection | null = null

  const lines = input.split(/\r?\n/)
  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i]
    const line = raw.trim()
    if (!line || line.startsWith('#')) continue

    const sectionMatch = line.match(/^\[(.+?)\]$/)
    if (sectionMatch) {
      current = { section: sectionMatch[1], fields: {} }
      result.push(current)
      continue
    }

    if (!current) {
      throw new Error(`第 ${i + 1} 行段外内容："${raw}"`)
    }

    const colonIdx = line.indexOf(':')
    if (colonIdx === -1) {
      throw new Error(`第 ${i + 1} 行缺 ':'："${raw}"`)
    }
    const key = line.slice(0, colonIdx).trim()
    const value = line.slice(colonIdx + 1).trim()
    current.fields[key] = value
  }

  return result
}
```

- [ ] **Step 4: 跑测试通过**

```bash
cd apps/jarvis-pet && npm test -- lpsParser
```

Expected: PASS 5/5

- [ ] **Step 5: Commit**

```bash
git add apps/jarvis-pet/src/sprites/lpsParser.ts apps/jarvis-pet/src/sprites/lpsParser.spec.ts
git commit -m "feat(jarvis-pet): VPet 风格 LPS 文本格式 parser + 5 单测"
```

---

## Task 8: SpriteSet —— 帧序列加载 + 帧时间解析

**Files:**
- Create: `apps/jarvis-pet/src/sprites/SpriteSet.ts`
- Create: `apps/jarvis-pet/src/sprites/SpriteSet.spec.ts`

**Depends on:** Task 7.

- [ ] **Step 1: 写失败测试**

`apps/jarvis-pet/src/sprites/SpriteSet.spec.ts`：

```typescript
import { describe, it, expect } from 'vitest'
import { parseFrameFilename } from './SpriteSet'

describe('parseFrameFilename', () => {
  it('解析 default_001_120.png → frame 1, duration 120ms', () => {
    expect(parseFrameFilename('default_001_120.png')).toEqual({
      frameIndex: 1,
      durationMs: 120
    })
  })

  it('解析 idle_010_50.png', () => {
    expect(parseFrameFilename('idle_010_50.png')).toEqual({
      frameIndex: 10,
      durationMs: 50
    })
  })

  it('文件名不符合约定 → 返 null（让调用方过滤掉）', () => {
    expect(parseFrameFilename('not_a_frame.png')).toBeNull()
    expect(parseFrameFilename('default_001.png')).toBeNull() // 缺 duration
    expect(parseFrameFilename('default.png')).toBeNull()
  })

  it('多下划线名字（如 a_start_b_loop_001_100.png）—— 取最后两段', () => {
    expect(parseFrameFilename('a_start_001_100.png')).toEqual({
      frameIndex: 1,
      durationMs: 100
    })
  })

  it('忽略大小写后缀', () => {
    expect(parseFrameFilename('default_001_120.PNG')).toEqual({
      frameIndex: 1,
      durationMs: 120
    })
  })
})
```

- [ ] **Step 2: 跑测试 fail**

```bash
cd apps/jarvis-pet && npm test -- SpriteSet
```

Expected: FAIL "Cannot find module './SpriteSet'"

- [ ] **Step 3: 实装 SpriteSet.ts**

`apps/jarvis-pet/src/sprites/SpriteSet.ts`：

```typescript
/// VPet sprite 帧文件名约定：<base>_<frameIndex_3digit>_<durationMs>.png
/// 例：default_001_120.png = 第 1 帧，120ms 显示
///     a_start_005_80.png = 第 5 帧，80ms 显示
/// 后缀 .png/.PNG 都接受。

export interface FrameMeta {
  frameIndex: number
  durationMs: number
}

export function parseFrameFilename(name: string): FrameMeta | null {
  // 去 .png 后缀（大小写不敏感）
  const lower = name.toLowerCase()
  if (!lower.endsWith('.png')) return null
  const base = name.slice(0, -4)

  // 取最后两个 _ 段
  const parts = base.split('_')
  if (parts.length < 3) return null

  const durationStr = parts[parts.length - 1]
  const indexStr = parts[parts.length - 2]
  const durationMs = parseInt(durationStr, 10)
  const frameIndex = parseInt(indexStr, 10)
  if (Number.isNaN(durationMs) || Number.isNaN(frameIndex)) return null
  if (durationMs <= 0 || frameIndex < 0) return null

  return { frameIndex, durationMs }
}

/// 一组帧——按 frameIndex 升序排列，每帧带 textureUrl + duration。
export interface SpriteFrame {
  textureUrl: string
  durationMs: number
}

export interface SpriteSet {
  graph: string       // GraphType，e.g. "Default"
  animat: string      // AnimatType，e.g. "Single" / "A_Start"
  mode: string        // ModeType，e.g. "Normal" / "Happy"
  loop: boolean       // 是否循环
  next?: string       // 链式下一段（A_Start → B_Loop 用）
  frames: SpriteFrame[]
}
```

- [ ] **Step 4: 跑测试通过**

```bash
cd apps/jarvis-pet && npm test -- SpriteSet
```

Expected: PASS 5/5

- [ ] **Step 5: Commit**

```bash
git add apps/jarvis-pet/src/sprites/SpriteSet.ts apps/jarvis-pet/src/sprites/SpriteSet.spec.ts
git commit -m "feat(jarvis-pet): SpriteSet 帧文件名解析（VPet 命名约定）"
```

---

## Task 9: AnimationPlayer —— 帧循环 + onFrame 回调

**Files:**
- Create: `apps/jarvis-pet/src/sprites/AnimationPlayer.ts`

无单测（涉及 PIXI Ticker，DOM 集成测代价高；smoke 阶段验证）。

**Depends on:** Task 6, Task 8.

- [ ] **Step 1: 实装**

`apps/jarvis-pet/src/sprites/AnimationPlayer.ts`：

```typescript
import { Sprite, Texture, Assets, type Application } from 'pixi.js'
import type { SpriteSet } from './SpriteSet'

/// 播放一组 SpriteSet —— 按 frame durationMs 切换 texture。
///
/// 用 PIXI.Ticker 驱动；每 tick 累加 elapsedMs，超过当前帧 duration 切下一帧。
/// loop=true 时循环；loop=false 播完触发 onComplete。
export class AnimationPlayer {
  private sprite: Sprite
  private textures: Texture[] = []
  private elapsedMs = 0
  private frameIdx = 0
  private set: SpriteSet | null = null
  private onCompleteHandler?: () => void

  constructor(private app: Application) {
    this.sprite = new Sprite()
    this.sprite.anchor.set(0.5, 1.0) // 锚点：底部居中（脚下）
    this.app.stage.addChild(this.sprite)
    this.app.ticker.add(this.tick, this)
  }

  /// 加载一组 SpriteSet 的所有 textures，准备播放。
  /// 调用前 sprite 不显示；async 等所有图加载完才返回。
  async load(set: SpriteSet): Promise<void> {
    const urls = set.frames.map(f => f.textureUrl)
    const loaded = await Assets.load(urls)
    this.textures = urls.map(u => loaded[u] as Texture)
    this.set = set
    this.frameIdx = 0
    this.elapsedMs = 0
    this.sprite.texture = this.textures[0]
    // 居屏幕中央底部
    this.sprite.x = this.app.screen.width / 2
    this.sprite.y = this.app.screen.height
  }

  /// 单次 tick 由 PIXI Ticker 自动调用
  private tick(ticker: { deltaMS: number }): void {
    if (!this.set || this.textures.length === 0) return
    this.elapsedMs += ticker.deltaMS
    const cur = this.set.frames[this.frameIdx]
    if (this.elapsedMs >= cur.durationMs) {
      this.elapsedMs -= cur.durationMs
      this.frameIdx++
      if (this.frameIdx >= this.set.frames.length) {
        if (this.set.loop) {
          this.frameIdx = 0
        } else {
          this.frameIdx = this.set.frames.length - 1
          this.set = null
          this.onCompleteHandler?.()
          return
        }
      }
      this.sprite.texture = this.textures[this.frameIdx]
    }
  }

  onComplete(handler: () => void): void {
    this.onCompleteHandler = handler
  }

  destroy(): void {
    this.app.ticker.remove(this.tick, this)
    this.sprite.destroy()
  }
}
```

- [ ] **Step 2: build 验证**

```bash
cd apps/jarvis-pet && npm run build 2>&1 | tail -5
```

Expected: 通过。

- [ ] **Step 3: Commit**

```bash
git add apps/jarvis-pet/src/sprites/AnimationPlayer.ts
git commit -m "feat(jarvis-pet): AnimationPlayer Ticker-driven 帧循环（loop + onComplete）"
```

---

## Task 10: 6 维数值 store（Pinia）

**Files:**
- Create: `apps/jarvis-pet/src/stores/stats.ts`
- Create: `apps/jarvis-pet/src/stores/stats.spec.ts`

**Depends on:** Task 1.

- [ ] **Step 1: 写失败测试**

`apps/jarvis-pet/src/stores/stats.spec.ts`：

```typescript
import { describe, it, expect, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useStatsStore, calMode } from './stats'

describe('stats store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('初始值符合 VPet 默认', () => {
    const s = useStatsStore()
    expect(s.strength).toBe(100)
    expect(s.strengthFood).toBe(100)
    expect(s.strengthDrink).toBe(100)
    expect(s.feeling).toBe(60)
    expect(s.health).toBe(100)
    expect(s.likability).toBe(0)
    expect(s.money).toBe(100)
  })

  it('update 部分字段不影响其他', () => {
    const s = useStatsStore()
    s.update({ strength: 50, feeling: 80 })
    expect(s.strength).toBe(50)
    expect(s.feeling).toBe(80)
    expect(s.strengthFood).toBe(100)
  })

  it('clamp 到 0~100（除 likability/money 外）', () => {
    const s = useStatsStore()
    s.update({ strength: 150, feeling: -10, likability: 999, money: 1000 })
    expect(s.strength).toBe(100)
    expect(s.feeling).toBe(0)
    expect(s.likability).toBe(999)
    expect(s.money).toBe(1000)
  })
})

describe('calMode', () => {
  it('Ill: health ≤ 30', () => {
    expect(calMode({ health: 30, feeling: 70 })).toBe('Ill')
    expect(calMode({ health: 0, feeling: 70 })).toBe('Ill')
  })

  it('PoorCondition: health ≤ 60 OR feeling ≤ 45', () => {
    expect(calMode({ health: 60, feeling: 70 })).toBe('PoorCondition')
    expect(calMode({ health: 80, feeling: 45 })).toBe('PoorCondition')
    expect(calMode({ health: 80, feeling: 30 })).toBe('PoorCondition')
  })

  it('Happy: feeling ≥ 90', () => {
    expect(calMode({ health: 100, feeling: 90 })).toBe('Happy')
  })

  it('Normal: 其它', () => {
    expect(calMode({ health: 80, feeling: 60 })).toBe('Normal')
  })
})
```

- [ ] **Step 2: 跑测试 fail**

```bash
cd apps/jarvis-pet && npm test -- stats
```

Expected: FAIL

- [ ] **Step 3: 实装**

`apps/jarvis-pet/src/stores/stats.ts`：

```typescript
import { defineStore } from 'pinia'
import { ref } from 'vue'

/// VPet 风格 ModeType —— 抄 VPet GraphInfo.cs CalMode()
export type ModeType = 'Ill' | 'PoorCondition' | 'Normal' | 'Happy'

/// VPet 风格 6 维数值 + likability + money。范围：
/// - strength/strengthFood/strengthDrink/feeling/health: 0-100 clamp
/// - likability/money: 累加 ∞，不 clamp
export const useStatsStore = defineStore('stats', () => {
  const strength = ref(100)
  const strengthFood = ref(100)
  const strengthDrink = ref(100)
  const feeling = ref(60)
  const health = ref(100)
  const likability = ref(0)
  const money = ref(100)

  function clamp01(v: number): number {
    return Math.max(0, Math.min(100, v))
  }

  function update(diff: Partial<{
    strength: number
    strengthFood: number
    strengthDrink: number
    feeling: number
    health: number
    likability: number
    money: number
  }>): void {
    if (diff.strength !== undefined) strength.value = clamp01(diff.strength)
    if (diff.strengthFood !== undefined) strengthFood.value = clamp01(diff.strengthFood)
    if (diff.strengthDrink !== undefined) strengthDrink.value = clamp01(diff.strengthDrink)
    if (diff.feeling !== undefined) feeling.value = clamp01(diff.feeling)
    if (diff.health !== undefined) health.value = clamp01(diff.health)
    if (diff.likability !== undefined) likability.value = diff.likability
    if (diff.money !== undefined) money.value = diff.money
  }

  return { strength, strengthFood, strengthDrink, feeling, health, likability, money, update }
})

/// 抄 VPet GraphInfo.cs CalMode()
export function calMode(s: { health: number; feeling: number }): ModeType {
  if (s.health <= 30) return 'Ill'
  if (s.health <= 60 || s.feeling <= 45) return 'PoorCondition'
  if (s.feeling >= 90) return 'Happy'
  return 'Normal'
}
```

- [ ] **Step 4: 跑测试通过**

```bash
cd apps/jarvis-pet && npm test -- stats
```

Expected: PASS 7/7

- [ ] **Step 5: Commit**

```bash
git add apps/jarvis-pet/src/stores/stats.ts apps/jarvis-pet/src/stores/stats.spec.ts
git commit -m "feat(jarvis-pet): 6 维数值 store（Pinia） + calMode (VPet 抄)"
```

---

## Task 11: fuxi-im REST + WS 客户端

**Files:**
- Create: `apps/jarvis-pet/src/types/event.ts`
- Create: `apps/jarvis-pet/src/api/fuxiClient.ts`

无单测（WS 集成测代价高；smoke 阶段验证）。

**Depends on:** Task 1.

- [ ] **Step 1: 写 types/event.ts —— EventKind wire 类型**

```typescript
/// fuxi EventKind wire 形式（按 #[serde(tag="type")] tagged union）
/// 仅列 jarvis-pet 关心的子集；其它走 WireKind.other
export type WireKind =
  | { type: 'xuannv_voice_line'; text: string }
  | { type: 'thinking_started' }
  | { type: 'thinking_finished' }
  | { type: 'agent_responded'; text: string }
  | { type: 'task_dispatched'; to: string }
  | { type: 'task_state_changed'; to: string }
  | { type: 'deliverable_accepted'; deliverable_id: string }
  | { type: 'deliverable_produced'; deliverable_kind: string; files: unknown[] }
  | { type: 'agent_dead'; cause: string }
  | { type: 'worker_heartbeat_state_changed'; inflight_count: number; max_concurrency: number }
  | { type: 'worker_stale_swept'; recycled_jobs: unknown[] }
  | { type: 'usage_report'; total_tokens: number; window_size: number; pct: number }
  | { type: 'xuannv_context_watermark'; threshold_pct: number; action: string }
  | { type: 'xuannv_handoff_written'; path: string; length_chars: number }
  | { type: 'user_prompted'; text: string }
  | { type: string; [key: string]: unknown }   // unknown fallback

export interface WireEvent {
  meta: {
    id?: string
    agent?: string
    task?: string
  }
  kind: WireKind
}
```

- [ ] **Step 2: 写 fuxiClient.ts**

```typescript
import type { WireEvent } from '@/types/event'

/// fuxi-im /api/conv WebSocket 客户端 + 简单 REST 包装。
/// reconnect 策略：disconnect 后 1s/2s/4s/8s ... cap 30s 退避重连。

export interface FuxiClientOpts {
  baseURL: string         // e.g. https://im.qmledmq.cn:8443
  pairToken?: string      // Authorization Bearer，可空（开发可不带）
  onEvent: (ev: WireEvent) => void
  onStatus?: (status: 'connecting' | 'connected' | 'disconnected') => void
}

export class FuxiClient {
  private ws: WebSocket | null = null
  private reconnectMs = 1000
  private stopped = false

  constructor(private opts: FuxiClientOpts) {}

  connect(): void {
    this.stopped = false
    this.openWs()
  }

  stop(): void {
    this.stopped = true
    this.ws?.close()
    this.ws = null
  }

  private openWs(): void {
    this.opts.onStatus?.('connecting')
    const url = this.wsUrl()
    const ws = new WebSocket(url)
    this.ws = ws

    ws.addEventListener('open', () => {
      this.reconnectMs = 1000
      this.opts.onStatus?.('connected')
    })

    ws.addEventListener('message', e => {
      try {
        const ev = JSON.parse(e.data as string) as WireEvent
        this.opts.onEvent(ev)
      } catch (err) {
        console.error('[fuxiClient] message parse failed', err, e.data)
      }
    })

    ws.addEventListener('close', () => {
      this.opts.onStatus?.('disconnected')
      if (!this.stopped) {
        setTimeout(() => this.openWs(), this.reconnectMs)
        this.reconnectMs = Math.min(this.reconnectMs * 2, 30000)
      }
    })

    ws.addEventListener('error', err => {
      console.warn('[fuxiClient] ws error', err)
    })
  }

  private wsUrl(): string {
    const u = new URL(this.opts.baseURL)
    u.protocol = u.protocol === 'https:' ? 'wss:' : 'ws:'
    u.pathname = '/api/conv'
    if (this.opts.pairToken) {
      u.searchParams.set('token', this.opts.pairToken)
    }
    return u.toString()
  }
}
```

- [ ] **Step 3: build 验证**

```bash
cd apps/jarvis-pet && npm run build 2>&1 | tail -5
```

Expected: 编译过。

- [ ] **Step 4: Commit**

```bash
git add apps/jarvis-pet/src/types/event.ts apps/jarvis-pet/src/api/fuxiClient.ts
git commit -m "feat(jarvis-pet): fuxi-im /api/conv WebSocket 客户端 + 退避重连"
```

---

## Task 12: 数值 mapper（EventKind → stats diff）

**Files:**
- Create: `apps/jarvis-pet/src/behavior/statsMapper.ts`
- Create: `apps/jarvis-pet/src/behavior/statsMapper.spec.ts`

**Depends on:** Task 10, Task 11.

- [ ] **Step 1: 写失败测试**

`apps/jarvis-pet/src/behavior/statsMapper.spec.ts`：

```typescript
import { describe, it, expect } from 'vitest'
import { mapEventToStats } from './statsMapper'
import type { WireEvent } from '@/types/event'

function ev(kind: WireEvent['kind']): WireEvent {
  return { meta: {}, kind }
}

describe('mapEventToStats', () => {
  it('UsageReport.pct 0.4 → strengthFood = 60', () => {
    const r = mapEventToStats(ev({ type: 'usage_report', total_tokens: 4000, window_size: 10000, pct: 0.4 }))
    expect(r).toEqual({ strengthFood: 60 })
  })

  it('WorkerHeartbeat 在途 3/总容量 10 → strength = 70', () => {
    const r = mapEventToStats(ev({
      type: 'worker_heartbeat_state_changed',
      inflight_count: 3,
      max_concurrency: 10
    }))
    expect(r).toEqual({ strength: 70 })
  })

  it('WorkerHeartbeat 容量 0 → 不更新（避免除零）', () => {
    const r = mapEventToStats(ev({
      type: 'worker_heartbeat_state_changed',
      inflight_count: 0,
      max_concurrency: 0
    }))
    expect(r).toEqual({})
  })

  it('DeliverableAccepted → feeling +5（绝对值，非 diff，需当前值）', () => {
    // 简化：mapper 返绝对值时不需要当前值；用 increment 字段表示
    const r = mapEventToStats(ev({ type: 'deliverable_accepted', deliverable_id: 'd1' }))
    expect(r).toEqual({ feelingDelta: 5, likabilityDelta: 1 })
  })

  it('UserPrompted → strengthDrink 重置 100 + likability +1', () => {
    const r = mapEventToStats(ev({ type: 'user_prompted', text: 'hi' }))
    expect(r).toEqual({ strengthDrink: 100, likabilityDelta: 1 })
  })

  it('XuannvContextWatermark handoff_offer → strengthFood = 0（强提示）', () => {
    const r = mapEventToStats(ev({
      type: 'xuannv_context_watermark',
      threshold_pct: 0.45,
      action: 'handoff_offer'
    }))
    expect(r).toEqual({ strengthFood: 0 })
  })

  it('未识别事件 → 空对象', () => {
    const r = mapEventToStats(ev({ type: 'some_unknown_event' }))
    expect(r).toEqual({})
  })
})
```

- [ ] **Step 2: 跑测试 fail**

```bash
cd apps/jarvis-pet && npm test -- statsMapper
```

- [ ] **Step 3: 实装**

`apps/jarvis-pet/src/behavior/statsMapper.ts`：

```typescript
import type { WireEvent } from '@/types/event'

/// EventKind 到 stats 更新的映射。
/// 返绝对值字段（如 strength: 70）= setter；返 *Delta 字段（如 feelingDelta: 5）= 增量。
/// 调用方负责把 delta 应用到 store 当前值。
export interface StatsUpdate {
  strength?: number
  strengthFood?: number
  strengthDrink?: number
  feeling?: number
  health?: number
  likability?: number
  money?: number
  feelingDelta?: number
  likabilityDelta?: number
  moneyDelta?: number
}

export function mapEventToStats(ev: WireEvent): StatsUpdate {
  const k = ev.kind
  switch (k.type) {
    case 'usage_report': {
      // strengthFood = 100 - 100*pct (context 余量 = 饱腹度)
      const pct = (k as { pct: number }).pct
      return { strengthFood: Math.round((1 - pct) * 100) }
    }
    case 'worker_heartbeat_state_changed': {
      const { inflight_count, max_concurrency } = k as {
        inflight_count: number
        max_concurrency: number
      }
      if (max_concurrency <= 0) return {}
      const load = inflight_count / max_concurrency
      return { strength: Math.round((1 - load) * 100) }
    }
    case 'deliverable_accepted':
      return { feelingDelta: 5, likabilityDelta: 1 }
    case 'user_prompted':
      // 用户主动 prompt 重置 "口渴度" + 加好感
      return { strengthDrink: 100, likabilityDelta: 1 }
    case 'xuannv_context_watermark': {
      const { action } = k as { action: string }
      // 玄女已要让贤 → strengthFood 归零（强提示）
      if (action === 'handoff_offer') return { strengthFood: 0 }
      return {}
    }
    case 'agent_dead':
    case 'worker_stale_swept':
      // 健康下滑——单事件 -5
      return { feelingDelta: 0 } // health 本期不通过单事件直接降，留 phase 2 做窗口聚合
    default:
      return {}
  }
}
```

- [ ] **Step 4: 跑测试通过**

```bash
cd apps/jarvis-pet && npm test -- statsMapper
```

Expected: PASS 7/7

⚠️ 测试 4 case 期望 `{ feelingDelta: 5, likabilityDelta: 1 }`——上面实装返同款；`agent_dead` case 返 `{ feelingDelta: 0 }` 不是 `{}`，测试不覆盖那条 case 所以不挂；如果 worker 担心覆盖率，可以把 agent_dead/stale_swept 改返 `{}` 与 default 同。

- [ ] **Step 5: Commit**

```bash
git add apps/jarvis-pet/src/behavior/statsMapper.ts apps/jarvis-pet/src/behavior/statsMapper.spec.ts
git commit -m "feat(jarvis-pet): EventKind → stats mapper（6 类事件 + 7 单测）"
```

---

## Task 13: PetCanvas Vue 组件 —— 集成 PixiApp + AnimationPlayer + stats

**Files:**
- Create: `apps/jarvis-pet/src/components/PetCanvas.vue`
- Modify: `apps/jarvis-pet/src/App.vue`

**Depends on:** Task 6 (PixiApp), Task 9 (AnimationPlayer), Task 10 (stats), Task 11 (fuxi client), Task 12 (mapper).

- [ ] **Step 1: 写 PetCanvas.vue**

```vue
<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from 'vue'
import { useStatsStore } from '@/stores/stats'
import { PixiApp } from '@/pixi/PixiApp'
import { AnimationPlayer } from '@/sprites/AnimationPlayer'
import type { SpriteSet } from '@/sprites/SpriteSet'
import { FuxiClient } from '@/api/fuxiClient'
import { mapEventToStats } from '@/behavior/statsMapper'

const canvasContainer = ref<HTMLDivElement | null>(null)
const stats = useStatsStore()

let pixiApp: PixiApp | null = null
let player: AnimationPlayer | null = null
let fuxi: FuxiClient | null = null

const PANEL_W = 280
const PANEL_H = 420

// Phase 1 hardcode 一组 Default sprite set；后续 manifest.lps 走 ManifestLoader 加载
const DEFAULT_SET: SpriteSet = {
  graph: 'Default',
  animat: 'Single',
  mode: 'Normal',
  loop: true,
  frames: [
    { textureUrl: '/sprites/xuannv/default/default_001_120.png', durationMs: 120 },
    { textureUrl: '/sprites/xuannv/default/default_002_120.png', durationMs: 120 },
    { textureUrl: '/sprites/xuannv/default/default_003_120.png', durationMs: 120 },
    { textureUrl: '/sprites/xuannv/default/default_004_120.png', durationMs: 120 },
    { textureUrl: '/sprites/xuannv/default/default_005_120.png', durationMs: 120 },
    { textureUrl: '/sprites/xuannv/default/default_006_120.png', durationMs: 120 }
  ]
}

onMounted(async () => {
  pixiApp = new PixiApp()
  const canvas = await pixiApp.init({ width: PANEL_W, height: PANEL_H })
  canvasContainer.value!.appendChild(canvas)

  player = new AnimationPlayer(pixiApp.pixi)
  await player.load(DEFAULT_SET)

  // 接 fuxi —— Phase 1 不带 token，本地连或公开 ws
  fuxi = new FuxiClient({
    baseURL: import.meta.env.VITE_FUXI_BASE_URL || 'https://im.qmledmq.cn:8443',
    onEvent: ev => {
      const update = mapEventToStats(ev)
      // 应用 setter 字段
      const setterFields: Partial<Record<string, number>> = {}
      for (const [k, v] of Object.entries(update)) {
        if (!k.endsWith('Delta') && typeof v === 'number') {
          setterFields[k] = v
        }
      }
      if (Object.keys(setterFields).length > 0) {
        stats.update(setterFields as Parameters<typeof stats.update>[0])
      }
      // 应用 delta 字段
      if (update.feelingDelta !== undefined) {
        stats.update({ feeling: stats.feeling + update.feelingDelta })
      }
      if (update.likabilityDelta !== undefined) {
        stats.update({ likability: stats.likability + update.likabilityDelta })
      }
    }
  })
  fuxi.connect()
})

onBeforeUnmount(() => {
  fuxi?.stop()
  player?.destroy()
  pixiApp?.destroy()
})
</script>

<template>
  <div class="pet-canvas" ref="canvasContainer">
    <!-- debug overlay：phase 1 显示 6 维数值方便观察 -->
    <div class="debug-overlay">
      <div>体力 {{ stats.strength }}</div>
      <div>饱腹 {{ stats.strengthFood }}</div>
      <div>口渴 {{ stats.strengthDrink }}</div>
      <div>心情 {{ stats.feeling }}</div>
      <div>健康 {{ stats.health }}</div>
      <div>好感 {{ stats.likability }}</div>
      <div>金钱 {{ stats.money }}</div>
    </div>
  </div>
</template>

<style scoped>
.pet-canvas {
  position: relative;
  width: 280px;
  height: 420px;
}
.debug-overlay {
  position: absolute;
  top: 0;
  left: 0;
  font-family: monospace;
  font-size: 10px;
  color: rgba(110, 136, 150, 0.7);
  pointer-events: none;
  user-select: none;
  background: rgba(250, 247, 241, 0.06);
  padding: 4px;
  border-radius: 4px;
}
</style>
```

- [ ] **Step 2: 改 App.vue 用 PetCanvas**

`apps/jarvis-pet/src/App.vue`：

```vue
<script setup lang="ts">
import PetCanvas from './components/PetCanvas.vue'
</script>

<template>
  <PetCanvas />
</template>

<style>
html, body { margin: 0; padding: 0; background: transparent; overflow: hidden; }
#app { width: 100vw; height: 100vh; background: transparent; }
</style>
```

- [ ] **Step 3: build 验证**

```bash
cd apps/jarvis-pet && npm run build 2>&1 | tail -5
```

Expected: 通过。

- [ ] **Step 4: Commit**

```bash
git add apps/jarvis-pet/src/components/PetCanvas.vue apps/jarvis-pet/src/App.vue
git commit -m "feat(jarvis-pet): PetCanvas 集成 PixiApp + AnimationPlayer + stats + fuxi 客户端"
```

---

## Task 14: 拖动互动（Tauri startDragging）

**Files:**
- Modify: `apps/jarvis-pet/src/components/PetCanvas.vue`

**Depends on:** Task 13.

- [ ] **Step 1: 加拖动监听**

在 `<script setup>` 末尾加：

```typescript
import { getCurrentWindow } from '@tauri-apps/api/window'

function onPointerDown(e: PointerEvent) {
  // 只响应主键 + 不是从 debug overlay 起点
  if (e.button !== 0) return
  if ((e.target as HTMLElement).closest('.debug-overlay')) return
  // Tauri 内置 dragging：startDragging() 自动跟随鼠标移动 NSPanel，无需 mousemove 监听
  getCurrentWindow().startDragging().catch(err => {
    console.warn('[drag] startDragging failed', err)
  })
}
```

在 `<template>` 根 div 加 `@pointerdown="onPointerDown"`：

```vue
<div class="pet-canvas" ref="canvasContainer" @pointerdown="onPointerDown">
```

- [ ] **Step 2: build 验证**

```bash
cd apps/jarvis-pet && npm run build 2>&1 | tail -5
```

- [ ] **Step 3: Commit**

```bash
git add apps/jarvis-pet/src/components/PetCanvas.vue
git commit -m "feat(jarvis-pet): 拖动桌宠（Tauri startDragging）"
```

---

## Task 15: 资产生成 · gpt-image-2 出 Default 6 帧（art ε 单独跑）

**Files:**
- Create: `apps/jarvis-pet/resources/sprites/xuannv/default/default_001_120.png` ... `default_006_120.png`
- Create: `apps/jarvis-pet/resources/sprites/xuannv/manifest.lps`

**Depends on:** Task 1（资产目录已建）。**与代码 task 全部并行**。

⚠️ 这个 task 由 art ε 跑，不是 alpha/beta/gamma 跑。

- [ ] **Step 1: 用 gpt-image-2 生 ref-sheet（如果还没）**

如果上 v0.3 周期 ref-sheet.png 还在 archive 分支，可以 cherry-pick / 从 `archive/jarvis-v0.3-pose` checkout：

```bash
git checkout archive/jarvis-v0.3-pose -- apps/jarvis/Sources/Resources/Pet/poses/ref-sheet.png
mv apps/jarvis/Sources/Resources/Pet/poses/ref-sheet.png apps/jarvis-pet/resources/sprites/xuannv/ref-sheet.png
git restore --staged apps/jarvis/Sources/Resources/Pet/poses/ref-sheet.png 2>/dev/null
```

否则重生（prompt 见 v0.3 plan A1）。

- [ ] **Step 2: 用 ref-sheet 走 IP-Adapter 一致性，生 6 帧 Default 呼吸循环**

每帧 prompt 模板：

```
基于 [ref-sheet.png] 一致角色：九天玄女上古女神 · 仙气素纱 · 隐身处理（侧影/背影，不正面露脸）·
姿态：侧身静立，双手交于身前，呼吸微动 phase {N}/6
- phase 1/6: 呼气末，胸口最低
- phase 2/6: 起息开始，肩微抬
- phase 3/6: 吸气中，胸口上升
- phase 4/6: 吸气末，肩最高
- phase 5/6: 呼气开始，肩微落
- phase 6/6: 呼气中，胸口下降
透明背景 · 同 ref 风格 · 560×840 · 衣袖随呼吸轻微飘动
```

输出：`apps/jarvis-pet/resources/sprites/xuannv/default/default_00{N}_120.png` (1..6)

⚠️ **monitor timeout 教训**（feedback_long_running_bg_monitor_timeout）：起 background gpt-image-2 task 时 monitor 必须 ≥ 1500s 或 persistent: true。单帧 ~120-180s × 6 = 12-18min，monitor 设 1800s。

- [ ] **Step 3: 写 manifest.lps**

`apps/jarvis-pet/resources/sprites/xuannv/manifest.lps`：

```
[character]
name: 玄女
author: fuxi
version: 0.4

[pnganimation]
graph: Default
animat: Single
mode: Normal
path: ./default
loop: true
```

- [ ] **Step 4: Commit**

```bash
git add apps/jarvis-pet/resources/sprites/xuannv/manifest.lps \
        apps/jarvis-pet/resources/sprites/xuannv/ref-sheet.png \
        apps/jarvis-pet/resources/sprites/xuannv/default/
git commit -m "art(jarvis-pet): 玄女 Default 6 帧呼吸循环 + ref-sheet + manifest.lps"
```

- [ ] **Step 5: SendMessage 给 team-lead 报告路径 + 等用户 ack 体感**

类似 v0.3 A1：6 帧出来后让用户先目视确认"循环顺滑 + 风格对" 再 unlock 后续 GraphType 批量出图。

---

## Task 16: 静态资源 serve 配置 + dev smoke run

**Files:**
- Modify: `apps/jarvis-pet/vite.config.ts` (加 publicDir 让 sprites 通过 dev server 可访问)
- Modify: `apps/jarvis-pet/src-tauri/tauri.conf.json` (确认 bundle.resources include sprites)

**Depends on:** Task 13, Task 15.

- [ ] **Step 1: vite publicDir 让 sprites 在 dev mode 通过 / 路径访问**

修改 `apps/jarvis-pet/vite.config.ts` —— 加 `publicDir`：

```typescript
export default defineConfig(async () => ({
  plugins: [vue()],
  resolve: {
    alias: { '@': path.resolve(__dirname, './src') }
  },
  publicDir: path.resolve(__dirname, './resources'),   // 让 /sprites/... 路径解析到 resources/sprites/
  clearScreen: false,
  // ... 其它不变
}))
```

⚠️ vite 把 publicDir 整个 cp 进 dist/，跟 Tauri bundle.resources 双重保险。

- [ ] **Step 2: 跑 dev 验证**

```bash
cd apps/jarvis-pet && npm run tauri dev
```

Expected:
- 透明窗口悬浮
- 屏幕中央底部：玄女水墨立绘呼吸循环（6 帧 720ms 一个周期）
- 左上角 debug overlay 显示 6 维数值
- 启动 fuxi-im 后端时数值会实时更新（如果连不上 ws，数值保持初值不动也没事 phase 1 OK）
- 点立绘（不点 debug overlay 区）拖动可以拖到屏幕任意位置

- [ ] **Step 3: 任一点不通过 → SendMessage team-lead 报告**

- [ ] **Step 4: 通过则 commit chore 标记**

```bash
git commit --allow-empty -m "chore(jarvis-pet): Phase 1 smoke 2 通过——立绘呼吸 + 拖动 + 数值显示"
```

---

## Task 17: build release .app + 装 ~/Applications/

**Files:**
- 仅 build + 部署，无代码改动

**Depends on:** Task 1-16 全部。

- [ ] **Step 1: build release**

```bash
cd apps/jarvis-pet && npm run tauri build 2>&1 | tail -10
```

Expected: `.app` 输出到 `src-tauri/target/release/bundle/macos/Xuannv.app`（Tauri 自动 codesign ad-hoc）。

- [ ] **Step 2: 装到 ~/Applications/**

⚠️ **跟 jarvis（药丸）共存检查**：v0.3 时 ~/Applications/Xuannv.app 是 jarvis 的（药丸 + 语音）。jarvis-pet 不能用同名覆盖——会冲突。

改装 `~/Applications/XuannvPet.app`：

```bash
TARGET=$HOME/Applications/XuannvPet.app
SOURCE=apps/jarvis-pet/src-tauri/target/release/bundle/macos/Xuannv.app
rm -rf "$TARGET"
cp -R "$SOURCE" "$TARGET"
codesign --verify --verbose=2 "$TARGET" 2>&1 | tail -3
```

⚠️ Info.plist 里 CFBundleIdentifier 是 `cn.qmledmq.fuxi.jarvis-pet`，跟 jarvis 的 `cn.qmledmq.fuxi.xuannv` 不同——TCC 权限独立，不互相干扰。

- [ ] **Step 3: smoke open**

```bash
open ~/Applications/XuannvPet.app
```

Expected: 桌宠出现在屏幕，可拖动。

- [ ] **Step 4: 改 README**

`apps/jarvis-pet/README.md`：

```markdown
# jarvis-pet · 玄女桌宠 v0.4

Tauri 2.0 + Vue 3 + PixiJS v8。VPet 风格 sprite 行为系统，玄女工作状态映射 6 维数值。

## Phase 1 范围

- macOS NSPanel 透明常驻
- 1 个 GraphType（Default 6 帧呼吸循环）
- fuxi-im /api/conv WS 接入 + 6 维数值实时更新
- 拖动互动
- debug overlay 显数值

## 起跑

```bash
cd apps/jarvis-pet
npm install
npm run tauri dev   # dev 模式
npm run tauri build # 出 release .app
```

Release .app 装在 `~/Applications/XuannvPet.app`，跟 jarvis（药丸 v0.2）共存（不同 bundle id）。

## 不在范围（留 Phase 2-4）

- BehaviorScheduler 调度器 + AnimatType 链
- 27 个其它 GraphType（Touch/Sleep/Say/Listen/Think/Work/...）
- ModeType 切换
- 接 jarvis Swift sidecar 的语音
- MOD 系统
- 跨平台（win/linux）

## 架构

详见 `docs/superpowers/specs/2026-05-11-jarvis-pet-v0.4-design.md`。
```

- [ ] **Step 5: Commit**

```bash
git add apps/jarvis-pet/README.md
git commit -m "docs(jarvis-pet): Phase 1 README + smoke run 路径"
```

---

## Self-Review 通过项

✅ Spec 对应实装：
- Tauri 2.0 + macOS NSPanel + 透明 + alwaysOnTop → Task 2/3/4/5
- PixiJS sprite 渲染 → Task 6/9
- VPet 风格 LPS 元数据 → Task 7
- 6 维数值 store + ModeType → Task 10
- fuxi-im /api/conv WS 接入 → Task 11
- EventKind → 数值 mapper → Task 12
- 集成 → Task 13
- 拖动 → Task 14
- 1 个 GraphType (Default 6 帧) → Task 15
- smoke + 部署 → Task 16/17

✅ 没在范围（Phase 1 不做）：
- BehaviorScheduler / AnimatType 链 / 27 其它 GraphType / ModeType 切换 / 语音 IPC / MOD —— 全部留 Phase 2-4

✅ 类型一致性：
- `SpriteSet` (Task 8) 跟 `AnimationPlayer.load(set)` (Task 9) 接口对齐
- `WireEvent` (Task 11) 跟 `mapEventToStats(ev)` (Task 12) 对齐
- `useStatsStore` setter 字段名跟 `mapEventToStats` 返字段名对齐

✅ 无 placeholder：所有 step 给完整代码。tauri-nspanel API 那段加了 ⚠️ 警示让 worker 按 docs.rs 实状对，不是 TODO。

## 依赖图（给 team-lead 派活参考）

```
T1 (bootstrap) ──┬─ T2 (Cargo) ── T3 (nspanel) ── T4 (conf) ── T5 (smoke 1)
                 │
                 ├─ T6 (PixiApp) ──┐
                 ├─ T7 (lpsParser)  │
                 ├─ T8 (SpriteSet) ─┤
                 │                  │
                 ├─ T9 (AnimPlayer ← T6+T8)
                 ├─ T10 (stats)
                 ├─ T11 (fuxiClient)
                 │
                 └─ T12 (statsMapper ← T10+T11)
                                    │
                                    ▼
                                 T13 (PetCanvas ← T6/9/10/11/12)
                                    │
                                    ▼
                                 T14 (drag)
                                    │
                                    ├── T16 (smoke 2) ← also needs T15
                                    ▼
                                 T17 (build .app)

T15 (art) —— 跟所有代码 task 并行
```

可并行 group：
- Group α (Tauri bootstrap chain): T1 → T2 → T3 → T4 → T5 → T14 → T17
- Group β (sprite & render chain): T6, T7 → T8 → T9 → T13 → T16
- Group γ (data layer): T10, T11 → T12
- Group δ (art): T15

最少 worker 配置：3 个工程 ε（α/β/γ） + 1 art ε。

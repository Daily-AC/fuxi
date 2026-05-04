import { defineConfig } from "vitest/config";
import solid from "vite-plugin-solid";
import { VitePWA } from "vite-plugin-pwa";
import path from "node:path";

// PWA：手机"添加到主屏"必须 manifest + sw 都到位。
// outDir 写死 dist，给 fuxi-im axum 的 include_dir!() 直接吃。
// 用 vitest/config 的 defineConfig（兼容 vite 5/6 + 暴露 test 字段）。
// `plugins` 类型在 vitest 2.x 仍指向 vite@5 的 Plugin —— 跟 vite@6 的 vite-plugin-solid/pwa
// 在结构上兼容但 nominal 类型不一致。这里 cast 到 any[] 走运行时即可。
export default defineConfig({
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["tests/setup.ts"],
    include: ["tests/unit/**/*.{test,spec}.{ts,tsx}"],
    css: true,
  },
  plugins: [
    solid() as unknown as never,
    VitePWA({
      registerType: "autoUpdate",
      injectRegister: "auto",
      // sw 仅缓静态资源；事件历史走 IndexedDB，不走 sw cache。
      // bug #77：用户报修复后界面没生效——SW 缓存了旧 bundle 不更新。
      // skipWaiting + clientsClaim 让新 SW 立刻接管不等老 client 退出，
      // autoUpdate 触发自动 reload。代价：新部署后老页面短暂闪一下；用户痛感优先。
      workbox: {
        globPatterns: ["**/*.{js,css,html,svg,woff2}"],
        navigateFallback: "/index.html",
        navigateFallbackDenylist: [/^\/api\//],
        skipWaiting: true,
        clientsClaim: true,
        cleanupOutdatedCaches: true,
      },
      manifest: {
        name: "伏羲 IM",
        short_name: "fuxi",
        description: "玄女在你口袋里",
        start_url: "/",
        display: "standalone",
        orientation: "portrait",
        background_color: "#0a0a0a",
        theme_color: "#0a0a0a",
        lang: "zh-CN",
        icons: [
          { src: "/icons/icon-192.png", sizes: "192x192", type: "image/png" },
          { src: "/icons/icon-512.png", sizes: "512x512", type: "image/png" },
          {
            src: "/icons/icon-512-maskable.png",
            sizes: "512x512",
            type: "image/png",
            purpose: "maskable",
          },
        ],
      },
      devOptions: { enabled: false },
    }) as unknown as never,
  ],
  resolve: {
    alias: { "~": path.resolve(__dirname, "src") },
    // jsdom 下让 solid-js 解析到 browser/development entry，否则 createSignal 会拿到 server stub
    conditions: ["development", "browser"],
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    sourcemap: true,
    target: "es2022",
  },
  server: {
    port: 5173,
    host: "127.0.0.1",
    // dev 期间打到 fuxi-im axum 默认端口
    proxy: {
      "/api": {
        target: "http://127.0.0.1:9100",
        changeOrigin: true,
        ws: true,
      },
    },
  },
});

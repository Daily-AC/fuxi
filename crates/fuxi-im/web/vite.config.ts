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
      // bug #f391c55b：generateSW 没法注入自定义 push event handler — 后端
      // web-push fan-out 推到浏览器后无消费方，通知不弹。改 injectManifest
      // 让 src/sw.ts 接管：precacheAndRoute / cleanupOutdatedCaches /
      // skipWaiting / clientsClaim 在 sw.ts 内复刻；加 push/notificationclick handler。
      strategies: "injectManifest",
      srcDir: "src",
      filename: "sw.ts",
      injectManifest: {
        globPatterns: ["**/*.{js,css,html,svg,woff2}"],
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

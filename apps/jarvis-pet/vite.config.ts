import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import path from 'node:path'

export default defineConfig(async () => ({
  plugins: [vue()],
  // Tauri 2 release 的 frontend 走自定义 scheme（macOS `tauri://`），
  // 文档 base 不可靠。绝对路径 `/assets/...` 会被 WebKit 把首段当 host
  // → fetch 拿到 `tauri://assets/...` 失败。base:'./' 让 vite 产相对
  // 路径，PIXI Assets.load 解出当前文档相对的 URL，dev/release 双稳。
  base: './',
  resolve: {
    alias: { '@': path.resolve(__dirname, './src') }
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: '127.0.0.1',
    hmr: { protocol: 'ws', host: '127.0.0.1', port: 1421 }
  },
  test: {
    globals: true,
    environment: 'happy-dom'
  }
}))

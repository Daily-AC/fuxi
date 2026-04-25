import { defineConfig, devices } from "@playwright/test";

// 跑 e2e 时启的是 mock backend（vite preview + msw 风格 fetch override）。
// 真后端拉起前 ε 不阻塞——契约见 decision-14 §C。
export default defineConfig({
  testDir: "./tests/e2e",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: 1,
  reporter: process.env.CI ? "github" : "list",
  timeout: 30_000,
  use: {
    baseURL: "http://127.0.0.1:4173",
    trace: "retain-on-failure",
    viewport: { width: 390, height: 844 },
  },
  projects: [
    {
      name: "mobile-chromium",
      use: { ...devices["Pixel 7"] },
    },
  ],
  webServer: {
    command: "npm run build && npx vite preview --port 4173 --host 127.0.0.1 --strictPort",
    port: 4173,
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});

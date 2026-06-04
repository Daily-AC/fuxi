// 视觉 gate 截图工具：mock 后端 + 自动登入，截某 tab/页面。
// 用法: node scripts/shot.mjs <out.png> [tabTestid] [dataJsonFile]
import { chromium } from "@playwright/test";
import { readFileSync } from "node:fs";

const out = process.argv[2] || "/tmp/im-shots/shot.png";
const tab = process.argv[3] || null;          // e.g. tab-home / tab-tasks / tab-more
const moreTile = process.argv[4] || null;      // e.g. more-tile-nodes
const dataFile = process.argv[5] || null;      // JSON injected onto window before load
const data = dataFile ? JSON.parse(readFileSync(dataFile, "utf8")) : {};

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2 });

await page.addInitScript((inj) => {
  for (const [k, v] of Object.entries(inj)) window[k] = v;
  const json = (d, s = 200) => new Response(JSON.stringify(d), { status: s, headers: { "content-type": "application/json" } });
  const real = window.fetch.bind(window);
  window.fetch = async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
    if (url.startsWith("/api/tasks")) {
      if (url.includes("/events")) return json({ events: [], next_cursor: null });
      if (url.includes("root=1")) return json({ tasks: [] });               // auth probe → logged in
      return json(window.__FUXI_TASKS_OVERVIEW__ ?? { running: [], completed: [] });
    }
    if (url.startsWith("/api/notifications")) return json(window.__FUXI_NOTIF__ ?? { unread_count: 0, items: [] });
    if (url === "/api/nodes") return json(window.__FUXI_NODES__ ?? { nodes: [] });
    if (url.startsWith("/api/projects")) return json(window.__FUXI_PROJECTS__ ?? { projects: [] });
    if (url.startsWith("/api/deliverables")) return json(window.__FUXI_DELIVERABLES__ ?? { deliverables: [] });
    if (url.startsWith("/api/memory")) return json(window.__FUXI_MEMORY__ ?? { layers: [] });
    if (url.startsWith("/api/roles")) return json(window.__FUXI_ROLES__ ?? { roles: [] });
    if (url.startsWith("/api/cron")) return json(window.__FUXI_CRON__ ?? { triggers: [] });
    if (url.startsWith("/api/conv/messages")) return json({ messages: window.__FUXI_HISTORY__ ?? [], next_before: null });
    if (url.startsWith("/api/workers/")) return json({ events: [], next_cursor: null });
    if (url === "/api/push/vapid-pub") return json({ public_key: "stub" });
    if (url === "/api/push/subscribe") return json({ ok: true });
    if (url.startsWith("/api/")) return json({}, 200);
    return real(input, init);
  };
  class FakeWS extends EventTarget { constructor(u){super();this.url=u;this.readyState=0;setTimeout(()=>{this.readyState=1;this.dispatchEvent(new Event("open"));},20);} send(){} close(){this.readyState=3;} }
  window.WebSocket = FakeWS;
}, data);

await page.goto("http://127.0.0.1:4173/");
await page.waitForSelector('[data-testid="main-shell"]', { timeout: 8000 });
if (tab) { await page.getByTestId(tab).click(); await page.waitForTimeout(300); }
if (moreTile) { await page.getByTestId(moreTile).click(); await page.waitForTimeout(300); }
await page.waitForTimeout(500);
await page.screenshot({ path: out });
console.log("shot saved", out);
await browser.close();

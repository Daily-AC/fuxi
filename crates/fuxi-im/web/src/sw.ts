/// <reference lib="webworker" />
// 自定义 Service Worker — vite-plugin-pwa injectManifest mode 入口。
//
// 为什么不走 generateSW（默认）：
//   generateSW 只会生成 Workbox 静态资源 precache，全无 push event handler。
//   后端 web-push fan-out 推到客户端后浏览器无消费方→连默认通知都不弹（或弹
//   generic "site updated" 没法跳转）。bug #f391c55b 根因之一。
//
// 提供：
//   1. precacheAndRoute(self.__WB_MANIFEST) — 静态资源 offline 缓存（与原 generateSW 行为对齐）
//   2. cleanupOutdatedCaches() — 升级时清旧 SW cache（避免 bug #77 SW 缓存旧 bundle）
//   3. navigation fallback → /index.html — SPA 路由刷新不 404
//   4. push event handler — 解 PushPayload JSON + showNotification
//   5. notificationclick handler — focus 现有 tab 或开新 tab，按 payload.url 路由
//   6. skipWaiting + clients.claim — 新 SW 立刻接管（同 generateSW 旧配置）

import { cleanupOutdatedCaches, precacheAndRoute } from "workbox-precaching";
import { NavigationRoute, registerRoute } from "workbox-routing";

declare const self: ServiceWorkerGlobalScope & {
  __WB_MANIFEST: Array<{ url: string; revision: string | null }>;
};

self.skipWaiting().catch(() => {
  // 老 Safari 不支持 .catch on skipWaiting；忽略
});

self.addEventListener("activate", (event) => {
  event.waitUntil(self.clients.claim());
});

cleanupOutdatedCaches();
precacheAndRoute(self.__WB_MANIFEST);

// SPA fallback：/foo/bar 这类前端路由刷新 → /index.html，并跳过 /api/。
// **网络优先**（原来 cache-first 把旧 index.html 钉死 → 引用旧 hash bundle →
// 用户得 ctrl+shift+r 强刷）：每次导航先拿网络最新 index.html（→ 最新 hash 资源
// → 最新 bundle），失败（离线）才回退缓存。配合 sw-update.ts 的自动 reload，
// 前端 rsync 部署无需手动强刷。
const navHandler = async (): Promise<Response> => {
  try {
    const fresh = await fetch("/index.html", { cache: "no-store" });
    if (fresh && fresh.ok) return fresh;
  } catch {
    // 离线 → 落缓存
  }
  const cache = await caches.match("/index.html");
  if (cache) return cache;
  return fetch("/index.html");
};
registerRoute(
  new NavigationRoute(navHandler, {
    denylist: [/^\/api\//],
  }),
);

/** 后端 PushPayload wire 形状——见 crates/fuxi-im/src/push/notify.rs `PushPayload`。
 *  全字段 String；wire 不会缺，缺也兜默认。 */
interface PushPayload {
  title?: string;
  body?: string;
  url?: string;
  tag?: string;
}

const DEFAULT_TITLE = "玄女";
const DEFAULT_BODY = "你有新消息";
const DEFAULT_URL = "/";

self.addEventListener("push", (event) => {
  // userVisibleOnly: true（前端 ensurePushSubscription 已设）→ 必须 showNotification
  // 否则浏览器会代为弹一条 "site has updated in the background" generic 通知。
  let payload: PushPayload = {};
  if (event.data) {
    try {
      payload = event.data.json() as PushPayload;
    } catch {
      // 老后端 / 兜底：纯文本 body
      try {
        payload = { body: event.data.text() };
      } catch {
        payload = {};
      }
    }
  }
  const title = payload.title ?? DEFAULT_TITLE;
  const body = payload.body ?? DEFAULT_BODY;
  const url = payload.url ?? DEFAULT_URL;
  // tag 同名通知会替换（避免堆通知；同 task done dedup 一脉）；缺省 = "fuxi" 一类。
  const tag = payload.tag ?? "fuxi";

  event.waitUntil(
    self.registration.showNotification(title, {
      body,
      tag,
      data: { url },
      icon: "/icons/icon-192.png",
      badge: "/icons/icon-192.png",
      requireInteraction: false,
    }),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const targetUrl: string =
    (event.notification.data as { url?: string } | null)?.url ?? DEFAULT_URL;

  // 寻找已开的 fuxi-im tab，focus + navigate；找不到再开新窗口。
  // iOS PWA 单实例语义下 matchAll 通常返回 1 个 client。
  event.waitUntil(
    (async () => {
      const allClients = await self.clients.matchAll({
        type: "window",
        includeUncontrolled: true,
      });
      // 同源优先：URL.origin 相同直接复用
      for (const client of allClients) {
        try {
          const u = new URL(client.url);
          if (u.origin === self.location.origin) {
            await client.focus();
            // 用 postMessage 让前端 router 走 hash 跳转（避免触发整页 reload）
            client.postMessage({ type: "fuxi:navigate", url: targetUrl });
            return;
          }
        } catch {
          // ignore parsing errors on weird client URLs
        }
      }
      if (self.clients.openWindow) {
        await self.clients.openWindow(targetUrl);
      }
    })(),
  );
});

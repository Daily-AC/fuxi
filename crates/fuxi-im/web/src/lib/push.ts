// Web Push 注册流程。仅在 manifest 装到主屏后 iOS 16.4+ 才能拿到原生权限。
import type { ApiClient } from "~/lib/api";

function urlBase64ToUint8Array(b64: string): Uint8Array {
  const padding = "=".repeat((4 - (b64.length % 4)) % 4);
  const base64 = (b64 + padding).replace(/-/g, "+").replace(/_/g, "/");
  const raw = atob(base64);
  const out = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i += 1) out[i] = raw.charCodeAt(i);
  return out;
}

export async function ensurePushSubscription(api: ApiClient): Promise<PushSubscription | null> {
  if (!("serviceWorker" in navigator) || !("PushManager" in window)) return null;
  const reg = await navigator.serviceWorker.ready;
  const existing = await reg.pushManager.getSubscription();
  if (existing) return existing;

  const perm = await Notification.requestPermission();
  if (perm !== "granted") return null;

  const { public_key } = await api.vapidPub();
  const sub = await reg.pushManager.subscribe({
    userVisibleOnly: true,
    // Cast to BufferSource: 浏览器要求 ArrayBufferView | string，TS lib 在 es2024 后窄成 ArrayBuffer
    applicationServerKey: urlBase64ToUint8Array(public_key) as unknown as BufferSource,
  });

  const json = sub.toJSON();
  if (!json.endpoint || !json.keys?.p256dh || !json.keys?.auth) return sub;
  await api.pushSubscribe({
    endpoint: json.endpoint,
    keys: { p256dh: json.keys.p256dh, auth: json.keys.auth },
  });
  return sub;
}

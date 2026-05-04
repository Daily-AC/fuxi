// bug #77 · 服务端版本号 + 前端 cache busting。
//
// 真因：iOS Safari PWA 模式下 SW 接管慢，新部署后 user 强刷不一定能拉新 bundle。
// 方案：后端 build.rs 注入 `FUXI_BUILD_SHA` + `FUXI_BUILD_TS`，`/api/version` 返。
// PWA 启动 + 60s 轮询，发现 sha 变 → 提示 + 强 reload。
//
// reload 流程：
//   1. unregister 所有 SW（不再缓存老 bundle）
//   2. 清 cache storage
//   3. location.reload() 拉新 index.html → 新 bundle hash 自动生效

const STORAGE_KEY = "fuxi:bundle:sha";
const POLL_MS = 60_000;

interface VersionResp {
  sha: string;
  build_at: string;
}

async function fetchVersion(): Promise<VersionResp | null> {
  try {
    const resp = await fetch("/api/version", { credentials: "include" });
    if (!resp.ok) return null;
    return (await resp.json()) as VersionResp;
  } catch {
    return null;
  }
}

async function hardReload(): Promise<void> {
  // 1. unregister SW
  if ("serviceWorker" in navigator) {
    const regs = await navigator.serviceWorker.getRegistrations();
    await Promise.all(regs.map((r) => r.unregister().catch(() => false)));
  }
  // 2. clear cache storage
  if ("caches" in window) {
    const keys = await caches.keys();
    await Promise.all(keys.map((k) => caches.delete(k).catch(() => false)));
  }
  // 3. force reload
  location.reload();
}

/** 启动检查 + 周期轮询。
 *  - 首次：拿到 sha 写 localStorage（baseline）
 *  - 后续：sha 变 → toast 提示 + 自动 hardReload
 *  - 失败：silent，不阻塞 PWA 主流程 */
export function startVersionPoll(onUpdate?: (newSha: string, oldSha: string) => void): () => void {
  let stopped = false;
  let cached: string | null = null;
  try {
    cached = localStorage.getItem(STORAGE_KEY);
  } catch {
    /* localStorage 被禁也没办法 */
  }

  const tick = async (): Promise<void> => {
    if (stopped) return;
    const v = await fetchVersion();
    if (!v) return;
    if (v.sha === "unknown") return; // 后端 git 不可用，跳过比较
    if (cached === null) {
      cached = v.sha;
      try {
        localStorage.setItem(STORAGE_KEY, v.sha);
      } catch {
        /* ignore */
      }
      return;
    }
    if (cached !== v.sha) {
      const old = cached;
      cached = v.sha;
      try {
        localStorage.setItem(STORAGE_KEY, v.sha);
      } catch {
        /* ignore */
      }
      onUpdate?.(v.sha, old);
      // 给 toast 4 秒钟显示再 hard reload
      setTimeout(() => {
        void hardReload();
      }, 4_000);
    }
  };

  void tick();
  const id = window.setInterval(tick, POLL_MS);
  return () => {
    stopped = true;
    window.clearInterval(id);
  };
}

// 前端纯客户端的 Service Worker 自动更新——补 version-poll 的盲区。
//
// version-poll 走 `/api/version` 的 `FUXI_BUILD_SHA`，那是 **Rust 编译期** 注入的。
// 纯前端 `rsync dist` 部署不重编 Rust → SHA 不变 → version-poll 看不见前端 UI
// 更新，用户得手动 ctrl+shift+r 强刷。
//
// 本模块靠 SW 自身闭环：rsync 新 dist 后 `sw.js`（含 precache manifest 的资源
// hash）内容变，浏览器探测到新 `sw.js` → install → `skipWaiting`（见 sw.ts）→
// 接管触发 `controllerchange` → 这里 reload 一次拿新 bundle。配合定时 + 回前台
// 主动 `reg.update()` 探测，让前端部署在 ~60s 内或下次切回前台时自动生效，无需
// 用户任何操作。
//
// 与 version-poll 互补：前端部署走本模块（sw.js 变）；后端部署走 version-poll
// （FUXI_BUILD_SHA 变，sw.js 不变本模块探不到）。两者都 guard 防 reload 循环。

const UPDATE_INTERVAL_MS = 60_000;

/** controllerchange 时是否该 reload。抽成纯函数方便单测。
 *  - 已在刷新中 → false（防 reload 循环）
 *  - 首次安装（启动时无 controller，null→SW 的 claim）→ false（页面本就是最新，
 *    不该无谓刷新闪一下）
 *  - 既有 controller 被新 SW 顶替（更新）→ true */
export function shouldReloadOnControllerChange(
  hadControllerAtStart: boolean,
  alreadyRefreshing: boolean,
): boolean {
  if (alreadyRefreshing) return false;
  return hadControllerAtStart;
}

export function startServiceWorkerAutoUpdate(): void {
  if (typeof navigator === "undefined" || !("serviceWorker" in navigator)) return;

  const hadController = Boolean(navigator.serviceWorker.controller);
  let refreshing = false;

  navigator.serviceWorker.addEventListener("controllerchange", () => {
    if (!shouldReloadOnControllerChange(hadController, refreshing)) return;
    refreshing = true;
    window.location.reload();
  });

  const checkForUpdate = async (): Promise<void> => {
    try {
      const reg = await navigator.serviceWorker.getRegistration();
      await reg?.update();
    } catch {
      // 离线 / 不支持：silent，不阻塞主流程
    }
  };

  void checkForUpdate();
  window.setInterval(() => void checkForUpdate(), UPDATE_INTERVAL_MS);
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) void checkForUpdate();
  });
}

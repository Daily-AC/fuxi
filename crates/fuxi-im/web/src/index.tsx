/* @refresh reload */
import { render } from "solid-js/web";

import "./styles/global.css";
import "./styles/texture.css";
import { App } from "./App";
import { startVersionPoll } from "./lib/version-poll";
import { pushToast } from "./lib/toast";

const root = document.getElementById("root");
if (!root) throw new Error("#root missing");

// v2：单屏对话 + sheet 召唤副视图。无 router 必要——LoginView gate + 主 Conversation 双态足够。
render(() => <App />, root);

// bug #77 · server-side 版本号 cache busting
// 启动 + 60s 轮询 /api/version；sha 变 → toast 提示 + 4s 后强 reload（unregister
// SW + clear cache + location.reload），让用户立即拿新 bundle，不再卡老缓存。
startVersionPoll((newSha) => {
  pushToast(`检测到新版本 ${newSha}，4 秒后自动刷新…`, "info");
});

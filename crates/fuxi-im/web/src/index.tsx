/* @refresh reload */
import { render } from "solid-js/web";

import "./styles/global.css";
import { App } from "./App";

const root = document.getElementById("root");
if (!root) throw new Error("#root missing");

// v2：单屏对话 + sheet 召唤副视图。无 router 必要——LoginView gate + 主 Conversation 双态足够。
render(() => <App />, root);

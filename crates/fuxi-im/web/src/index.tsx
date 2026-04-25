/* @refresh reload */
import { render } from "solid-js/web";
import { Router, Route } from "@solidjs/router";
import { HashRouter } from "@solidjs/router";

import "./styles/global.css";
import { App } from "./App";
import { TasksView } from "./views/TasksView";
import { ConvView } from "./views/ConvView";
import { TaskView } from "./views/TaskView";

const root = document.getElementById("root");
if (!root) throw new Error("#root missing");

// HashRouter：决策 14 §F 锁死 hash router。原因：file:// 子目录部署 + 静态包打回退 fallback 简单。
render(
  () => (
    <HashRouter root={App}>
      <Route path="/" component={TasksView} />
      <Route path="/conv" component={ConvView} />
      <Route path="/task/:id" component={TaskView} />
    </HashRouter>
  ),
  root,
);

// 不要让 Router import 报错
void Router;

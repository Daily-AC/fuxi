import type { Component } from "solid-js";

// 家(Home) 首页 · daimeng 奶油糖果重构 · 占位
//
// 真正的首页 UI（玄女 mascot 问候 + 通知红点入口 + 快捷瓦片）在后续任务实装。
// 此处仅占位让 4 tab 壳子接通；unreadCount 从 MainShell 的 notif resource 透传，
// 占位先消费（hidden indicator）让 createResource 不变 dead code。
export const HomePage: Component<{ unreadCount?: number }> = (props) => (
  <div data-testid="page-home">
    <span data-testid="home-unread" hidden aria-hidden="true">
      {props.unreadCount ?? 0}
    </span>
  </div>
);

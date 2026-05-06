import { type Component, type JSX } from "solid-js";
import { useApi } from "./ApiProvider";
import styles from "./MoreSubShell.module.css";

// 「更多」hub 二级 sub-page 公共 wrapper · v1-session17 task #9
//
// 子页（NodesPage / ProjectsPage / DeliverablesPage / Memory / Roles / Cron / Settings）
// 原本各自是 tab base、自带顶栏；现在沉到 hub 二级，需要一个返回按钮把 sub
// 清回 null（hub 首页）。把 wrapper 抽公共，让子页保持原样不必各加返回。
//
// 子页若已含自己的 header（如 NodesPage 的 "节点" 标题），用 `hideTitle` 把
// shell 标题省掉避免双标题；否则交给 shell 显示，子页只渲染内容。

export interface MoreSubShellProps {
  /** 顶栏标题（与 hub tile label 对齐）。 */
  title: string;
  /** 子页是否已自带顶栏标题（NodesPage / ProjectsPage / DeliverablesPage = true）。 */
  hideTitle?: boolean;
  /** 返回按钮 testid 后缀，用于 e2e 区分（默认 sub-name）。 */
  testIdSuffix?: string;
  children: JSX.Element;
}

export const MoreSubShell: Component<MoreSubShellProps> = (props) => {
  const { setMoreSub } = useApi();
  return (
    <section
      class={styles.shell}
      data-testid={`more-sub-${props.testIdSuffix ?? "shell"}`}
    >
      <header class={styles.head}>
        <button
          type="button"
          class={styles.back}
          onClick={() => setMoreSub(null)}
          data-testid="more-sub-back"
          aria-label="返回更多"
        >
          ‹ 更多
        </button>
        {!props.hideTitle && <h1 class={styles.title}>{props.title}</h1>}
      </header>
      <div class={styles.body}>{props.children}</div>
    </section>
  );
};

import { For, Show, createResource, type Component } from "solid-js";
import { useApi } from "~/components/ApiProvider";
import type { RoleCardView } from "~/types/api";
import styles from "./RolesPage.module.css";

// 「更多 → 角色」· v1-session17 task #9
//
// 列项目根 `roles/<name>/ROLE.md` 解析出来的角色卡。仅读：写靠用户改文件 +
// 重启 fuxi-im 重新扫；运行期不动。

export const RolesPage: Component = () => {
  const { client } = useApi();
  const [data] = createResource(() => client.fetchRoles());
  return (
    <div class={styles.page} data-testid="page-roles">
      <Show when={!data.loading} fallback={<p class={styles.empty}>加载中…</p>}>
        <Show
          when={(data()?.roles.length ?? 0) > 0}
          fallback={<p class={styles.empty}>未发现 role —— roles 目录为空。</p>}
        >
          <For each={data()!.roles}>{(r) => <RoleCard role={r} />}</For>
        </Show>
      </Show>
    </div>
  );
};

const RoleCard: Component<{ role: RoleCardView }> = (props) => {
  return (
    <article class={styles.card} data-testid={`role-card-${props.role.id}`}>
      <header class={styles.head}>
        <span class={styles.name}>{props.role.name}</span>
        <Show when={props.role.tier}>
          {(tier) => <span class={styles.tier}>{tier()}</span>}
        </Show>
      </header>
      <Show when={props.role.description}>
        <p class={styles.desc}>{props.role.description}</p>
      </Show>
      <div class={styles.foot}>
        <Show when={props.role.cli}>
          {(cli) => <span class={styles.chip}>CLI: {cli()}</span>}
        </Show>
        <Show when={props.role.has_instructions}>
          <span class={styles.chip}>instructions</span>
        </Show>
        <Show when={props.role.has_examples}>
          <span class={styles.chip}>examples</span>
        </Show>
        <Show when={props.role.has_resources}>
          <span class={styles.chip}>resources</span>
        </Show>
        <Show when={props.role.allowed_tools}>
          {(tools) => <span class={styles.chip}>{tools()}</span>}
        </Show>
      </div>
    </article>
  );
};

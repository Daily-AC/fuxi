import { For, Show, createResource, type Component } from "solid-js";
import { useApi } from "~/components/ApiProvider";
import { Card, type CardTone } from "~/components/ui/Card";
import { EmptyState } from "~/components/ui/EmptyState";
import { StatePill } from "~/components/ui/StatePill";
import type { RoleCardView } from "~/types/api";
import styles from "./RolesPage.module.css";

// 「更多 → 角色」· v1-session17 task #9 · daimeng 奶油糖果重绘（能力卡）
//
// 列项目根 `roles/<name>/ROLE.md` 解析出来的角色卡。仅读：写靠用户改文件 +
// 重启 fuxi-im 重新扫；运行期不动。
//
// RESKIN：保留全部行为 + data-testid（page-roles / role-card-<id>）。
// 每角色一 Card：角色色头像点（玄女 lavender / 鲁班 peach / 蒲松 mint）+ 名 +
// tier StatePill + 描述 + 能力暖圆药丸。空态 EmptyState；页底 u-mesh。

// 角色 → tone：按 id / name 命中文化命名，未知归 plain。
const roleTone = (role: RoleCardView): CardTone => {
  const key = `${role.id} ${role.name}`.toLowerCase();
  if (key.includes("xuannv") || role.name.includes("玄女")) return "lavender";
  if (key.includes("luban") || role.name.includes("鲁班")) return "peach";
  if (key.includes("pusong") || role.name.includes("蒲松")) return "mint";
  return "plain";
};

export const RolesPage: Component = () => {
  const { client } = useApi();
  const [data] = createResource(() => client.fetchRoles());
  return (
    <div class={`u-mesh u-noise ${styles.page}`} data-testid="page-roles">
      <Show when={!data.loading} fallback={<p class={styles.muted}>加载中…</p>}>
        <Show
          when={(data()?.roles.length ?? 0) > 0}
          fallback={
            <EmptyState
              title="还没有角色～"
              hint="roles 目录为空。在项目根 roles/<name>/ROLE.md 写角色卡后重启 fuxi-im。"
              mascotState="sleep"
            />
          }
        >
          <div class={styles.list}>
            <For each={data()!.roles}>{(r) => <RoleCard role={r} />}</For>
          </div>
        </Show>
      </Show>
    </div>
  );
};

const RoleCard: Component<{ role: RoleCardView }> = (props) => {
  const initial = (): string =>
    (props.role.name || props.role.id || "?").trim().charAt(0);
  return (
    <Card tone={roleTone(props.role)} class={styles.card}>
      <span class={styles.inner} data-testid={`role-card-${props.role.id}`}>
        <header class={styles.head}>
          <span
            class={styles.avatar}
            data-tone={roleTone(props.role)}
            aria-hidden="true"
          >
            {initial()}
          </span>
          <span class={styles.nameWrap}>
            <span class={styles.name}>{props.role.name}</span>
            <Show when={props.role.tier}>
              {(tier) => <StatePill label={tier()} tone="neutral" />}
            </Show>
          </span>
        </header>
        <Show when={props.role.description}>
          <p class={styles.desc}>{props.role.description}</p>
        </Show>
        <div class={styles.chips}>
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
      </span>
    </Card>
  );
};

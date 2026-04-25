import type { Component } from "solid-js";
import { A } from "@solidjs/router";
import type { TaskCard as TaskCardType } from "~/types/events";
import { relativeTime, shortAgentId, statusLabel } from "~/lib/format";
import styles from "./TaskCard.module.css";

export const TaskCard: Component<{ task: TaskCardType }> = (props) => {
  return (
    <A
      href={`/task/${encodeURIComponent(props.task.id)}`}
      class={styles.card}
      classList={{
        [styles[`status-${props.task.status}`] ?? ""]: true,
        [styles.dim ?? ""]: props.task.status === "done",
      }}
      data-testid={`task-card-${props.task.id}`}
      data-status={props.task.status}
    >
      <div class={styles.head}>
        <span class={styles.statusDot} aria-hidden="true" />
        <span class={styles.status}>{statusLabel(props.task.status)}</span>
        <time class={styles.time}>{relativeTime(props.task.updated_at)}</time>
      </div>
      <h3 class={styles.title}>{props.task.title}</h3>
      {props.task.summary ? <p class={styles.summary}>{props.task.summary}</p> : null}
      <div class={styles.foot}>
        {props.task.agent ? (
          <span class={styles.agent}>
            <span class="agent-id">{shortAgentId(props.task.agent)}</span>
          </span>
        ) : (
          <span class={styles.agentNone}>未派</span>
        )}
        <span class={styles.id} aria-label="任务 id">
          <span class="agent-id">#{props.task.id.slice(0, 8)}</span>
        </span>
      </div>
    </A>
  );
};

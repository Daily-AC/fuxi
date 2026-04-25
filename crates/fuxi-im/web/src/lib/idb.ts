// IndexedDB 缓存层 —— 离线打开能立刻看到任务卡片快照 + 最近 100 条事件。
// 不缓 user-private 的密钥/token；那些归 cookie。
import { openDB, type DBSchema, type IDBPDatabase } from "idb";
import type { EventKind, TaskCard } from "~/types/events";

const DB_NAME = "fuxi-im";
const DB_VERSION = 1;
const MAX_EVENTS = 100;

interface FuxiSchema extends DBSchema {
  tasks: {
    key: string;
    value: TaskCard;
    indexes: { "by-updated": string };
  };
  events: {
    key: string; // synthetic: `${ts}|${id ?? ""}`
    value: EventKind & { _key: string };
    indexes: { "by-task": string };
  };
}

let dbPromise: Promise<IDBPDatabase<FuxiSchema>> | null = null;

function db(): Promise<IDBPDatabase<FuxiSchema>> {
  if (!dbPromise) {
    dbPromise = openDB<FuxiSchema>(DB_NAME, DB_VERSION, {
      upgrade(d) {
        if (!d.objectStoreNames.contains("tasks")) {
          const ts = d.createObjectStore("tasks", { keyPath: "id" });
          ts.createIndex("by-updated", "updated_at");
        }
        if (!d.objectStoreNames.contains("events")) {
          const es = d.createObjectStore("events", { keyPath: "_key" });
          es.createIndex("by-task", "task_id");
        }
      },
    });
  }
  return dbPromise;
}

export async function cacheTasks(tasks: TaskCard[]): Promise<void> {
  const d = await db();
  const tx = d.transaction("tasks", "readwrite");
  for (const t of tasks) await tx.store.put(t);
  await tx.done;
}

export async function loadCachedTasks(): Promise<TaskCard[]> {
  const d = await db();
  const all = await d.getAllFromIndex("tasks", "by-updated");
  return all.reverse();
}

function eventKey(e: EventKind): string {
  const ts = e.ts ?? "";
  const id = e.id ?? "";
  return `${ts}|${id}|${(e as { type: string }).type}`;
}

export async function cacheEvents(events: EventKind[]): Promise<void> {
  if (events.length === 0) return;
  const d = await db();
  const tx = d.transaction("events", "readwrite");
  for (const e of events) {
    const key = eventKey(e);
    await tx.store.put({ ...e, _key: key });
  }
  // 维持环形 max=100（per-store，整体；够 PWA 离线时显示一屏）
  const count = await tx.store.count();
  if (count > MAX_EVENTS) {
    const cursor = await tx.store.openCursor();
    let toDelete = count - MAX_EVENTS;
    let c = cursor;
    while (c && toDelete > 0) {
      await c.delete();
      toDelete -= 1;
      c = await c.continue();
    }
  }
  await tx.done;
}

export async function loadCachedEvents(taskId?: string): Promise<EventKind[]> {
  const d = await db();
  if (taskId) {
    const matches = await d.getAllFromIndex("events", "by-task", taskId);
    return matches.map(stripKey);
  }
  const all = await d.getAll("events");
  return all.map(stripKey);
}

function stripKey(e: EventKind & { _key: string }): EventKind {
  const { _key: _ignored, ...rest } = e;
  void _ignored;
  return rest as EventKind;
}

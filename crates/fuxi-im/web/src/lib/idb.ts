// IndexedDB 缓存层 —— 离线打开能立刻看到任务卡片快照 + 最近 N 条事件。
// 当前 v2 阶段 1/2 暂未接历史回放（spec 简化），保留接口供阶段 4-5 历史接通时复用。
// 不缓 user-private 的密钥 / token；那些归 cookie。
import { openDB, type DBSchema, type IDBPDatabase } from "idb";
import type { ServerEvent, TaskCard } from "~/types/events";

const DB_NAME = "fuxi-im";
// 注：schema 从 v1 改 v2 wire format（嵌套 meta/kind）；用户机器上的 v1 events 失效，
// upgrade 时 clear events store 重建索引。
const DB_VERSION = 2;
const MAX_EVENTS = 100;

interface FuxiSchema extends DBSchema {
  tasks: {
    key: string;
    value: TaskCard;
    indexes: { "by-updated": string };
  };
  events: {
    key: string; // synthetic: `${meta.at}|${meta.id}|${kind.type}`
    value: ServerEvent & { _key: string; _task: string | null };
    indexes: { "by-task": string };
  };
}

let dbPromise: Promise<IDBPDatabase<FuxiSchema>> | null = null;

function idbAvailable(): boolean {
  return typeof indexedDB !== "undefined";
}

function db(): Promise<IDBPDatabase<FuxiSchema>> | null {
  if (!idbAvailable()) return null;
  if (!dbPromise) {
    dbPromise = openDB<FuxiSchema>(DB_NAME, DB_VERSION, {
      upgrade(d, oldVersion) {
        if (!d.objectStoreNames.contains("tasks")) {
          const ts = d.createObjectStore("tasks", { keyPath: "id" });
          ts.createIndex("by-updated", "updated_at");
        }
        if (oldVersion < 2 && d.objectStoreNames.contains("events")) {
          // wire fix · v1 缓存的 flat 事件已经无效，整 store 重建
          d.deleteObjectStore("events");
        }
        if (!d.objectStoreNames.contains("events")) {
          const es = d.createObjectStore("events", { keyPath: "_key" });
          es.createIndex("by-task", "_task");
        }
      },
    }).catch((err) => {
      console.warn("idb open failed, falling back to memory-only", err);
      throw err;
    });
  }
  return dbPromise;
}

export async function cacheTasks(tasks: TaskCard[]): Promise<void> {
  const p = db();
  if (!p) return;
  const d = await p;
  const tx = d.transaction("tasks", "readwrite");
  for (const t of tasks) await tx.store.put(t);
  await tx.done;
}

export async function loadCachedTasks(): Promise<TaskCard[]> {
  const p = db();
  if (!p) return [];
  const d = await p;
  const all = await d.getAllFromIndex("tasks", "by-updated");
  return all.reverse();
}

function eventKey(e: ServerEvent): string {
  const at = e.meta?.at ?? "";
  const id = e.meta?.id ?? "";
  const t = e.kind?.type ?? "?";
  return `${at}|${id}|${t}`;
}

export async function cacheEvents(events: ServerEvent[]): Promise<void> {
  if (events.length === 0) return;
  const p = db();
  if (!p) return;
  const d = await p;
  const tx = d.transaction("events", "readwrite");
  for (const e of events) {
    const key = eventKey(e);
    await tx.store.put({ ...e, _key: key, _task: e.meta?.task ?? null });
  }
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

export async function loadCachedEvents(taskId?: string): Promise<ServerEvent[]> {
  const p = db();
  if (!p) return [];
  const d = await p;
  if (taskId) {
    const matches = await d.getAllFromIndex("events", "by-task", taskId);
    return matches.map(stripMeta);
  }
  const all = await d.getAll("events");
  return all.map(stripMeta);
}

function stripMeta(e: ServerEvent & { _key: string; _task: string | null }): ServerEvent {
  const { _key: _k, _task: _t, ...rest } = e;
  void _k;
  void _t;
  return rest as ServerEvent;
}

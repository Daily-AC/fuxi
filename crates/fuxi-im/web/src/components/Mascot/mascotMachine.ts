export type MascotStateKind = "idle"|"blink"|"talk"|"think"|"happy"|"wave"|"surprise"|"sleep"|"poke";
export interface MascotState { kind: MascotStateKind }
export type MascotEvent =
  | { type: "stream-start" } | { type: "stream-end" }
  | { type: "work-running" } | { type: "work-idle" }
  | { type: "task-done" } | { type: "error" }
  | { type: "greet" } | { type: "poke" }
  | { type: "idle-timeout" } | { type: "settle" };

// settle = 瞬时态（happy/surprise/poke/wave）播放结束的归位信号
const TRANSIENT: MascotStateKind[] = ["happy","surprise","poke","wave"];

export function reduceMascot(s: MascotState, e: MascotEvent): MascotState {
  switch (e.type) {
    case "error": return { kind: "surprise" };       // 最高优先
    case "poke": return { kind: "poke" };
    case "task-done": return { kind: "happy" };
    case "greet": return { kind: "wave" };
    case "stream-start": return { kind: "talk" };
    case "work-running": return s.kind === "talk" ? s : { kind: "think" };
    case "stream-end":
    case "work-idle": return { kind: "idle" };
    case "idle-timeout": return s.kind === "idle" ? { kind: "sleep" } : s;
    case "settle": return TRANSIENT.includes(s.kind) ? { kind: "idle" } : s;
    default: return s;
  }
}

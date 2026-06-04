import {
  createContext,
  useContext,
  createSignal,
  onCleanup,
  type Accessor,
  type ParentComponent,
} from "solid-js";
import {
  reduceMascot,
  type MascotEvent,
  type MascotState,
  type MascotStateKind,
} from "./mascotMachine";
import { QUIPS } from "./quips";

// 瞬时态归位时长（ms）——跟 Task 7 mascot 动画手感对齐。
// 播放完动画后自动 dispatch {type:'settle'} 回 idle。
const SETTLE_MS: Partial<Record<MascotStateKind, number>> = {
  happy: 1800,
  poke: 650,
  surprise: 1200,
  wave: 1000,
};

// idle 持续这么久（无任何事件打断）后睡着。
const IDLE_SLEEP_MS = 30000;

export interface MascotContextValue {
  mascotState: Accessor<MascotState>;
  dispatch: (e: MascotEvent) => void;
  poke: () => void;
  quip: Accessor<string | null>;
}

export const MascotContext = createContext<MascotContextValue>();

export const MascotProvider: ParentComponent = (props) => {
  const [mascotState, setMascotState] = createSignal<MascotState>({
    kind: "idle",
  });
  const [quip, setQuip] = createSignal<string | null>(null);

  // 单一 settle 定时器：每次进瞬时态前先清旧的，避免上一态的 settle 把新态误归位。
  let settleTimer: ReturnType<typeof setTimeout> | undefined;
  // idle→sleep 定时器：进 idle 时启动，离 idle 时清掉。
  let sleepTimer: ReturnType<typeof setTimeout> | undefined;

  const clearSettle = (): void => {
    if (settleTimer !== undefined) {
      clearTimeout(settleTimer);
      settleTimer = undefined;
    }
  };
  const clearSleep = (): void => {
    if (sleepTimer !== undefined) {
      clearTimeout(sleepTimer);
      sleepTimer = undefined;
    }
  };

  const dispatch = (e: MascotEvent): void => {
    const next = reduceMascot(mascotState(), e);
    setMascotState(next);

    // 瞬时态：清旧 settle，排新 settle。
    const settleMs = SETTLE_MS[next.kind];
    clearSettle();
    if (settleMs !== undefined) {
      settleTimer = setTimeout(() => {
        settleTimer = undefined;
        dispatch({ type: "settle" });
      }, settleMs);
    }

    // idle→sleep：进 idle 重置睡眠计时；离 idle（含 sleep）清掉。
    clearSleep();
    if (next.kind === "idle") {
      sleepTimer = setTimeout(() => {
        sleepTimer = undefined;
        dispatch({ type: "idle-timeout" });
      }, IDLE_SLEEP_MS);
    }
  };

  const poke = (): void => {
    const pick = QUIPS[Math.floor(Math.random() * QUIPS.length)] ?? null;
    setQuip(pick);
    dispatch({ type: "poke" });
  };

  // 初始 idle 也要起睡眠计时。
  sleepTimer = setTimeout(() => {
    sleepTimer = undefined;
    dispatch({ type: "idle-timeout" });
  }, IDLE_SLEEP_MS);

  onCleanup(() => {
    clearSettle();
    clearSleep();
  });

  const value: MascotContextValue = { mascotState, dispatch, poke, quip };
  return (
    <MascotContext.Provider value={value}>
      {props.children}
    </MascotContext.Provider>
  );
};

export function useMascot(): MascotContextValue {
  const ctx = useContext(MascotContext);
  if (!ctx) {
    throw new Error("useMascot 必须在 <MascotProvider> 内使用");
  }
  return ctx;
}

import type { WireEvent } from '@/types/event'

/// EventKind 到 stats 更新的映射。
/// 返绝对值字段（如 strength: 70）= setter；返 *Delta 字段（如 feelingDelta: 5）= 增量。
/// 调用方负责把 delta 应用到 store 当前值。
export interface StatsUpdate {
  strength?: number
  strengthFood?: number
  strengthDrink?: number
  feeling?: number
  health?: number
  likability?: number
  money?: number
  feelingDelta?: number
  likabilityDelta?: number
  moneyDelta?: number
}

export function mapEventToStats(ev: WireEvent): StatsUpdate {
  const k = ev.kind
  switch (k.type) {
    case 'usage_report': {
      // strengthFood = 100 - 100*pct (context 余量 = 饱腹度)
      const pct = (k as { pct: number }).pct
      return { strengthFood: Math.round((1 - pct) * 100) }
    }
    case 'worker_heartbeat_state_changed': {
      const { inflight_count, max_concurrency } = k as {
        inflight_count: number
        max_concurrency: number
      }
      if (max_concurrency <= 0) return {}
      const load = inflight_count / max_concurrency
      return { strength: Math.round((1 - load) * 100) }
    }
    case 'deliverable_accepted':
      return { feelingDelta: 5, likabilityDelta: 1 }
    case 'user_prompted':
      // 用户主动 prompt 重置 "口渴度" + 加好感
      return { strengthDrink: 100, likabilityDelta: 1 }
    case 'xuannv_context_watermark': {
      const { action } = k as { action: string }
      // 玄女已要让贤 → strengthFood 归零（强提示）
      if (action === 'handoff_offer') return { strengthFood: 0 }
      return {}
    }
    default:
      return {}
  }
}

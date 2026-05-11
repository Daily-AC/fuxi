import { describe, it, expect } from 'vitest'
import { mapEventToStats } from './statsMapper'
import type { WireEvent } from '@/types/event'

function ev(kind: WireEvent['kind']): WireEvent {
  return { meta: {}, kind }
}

describe('mapEventToStats', () => {
  it('UsageReport.pct 0.4 → strengthFood = 60', () => {
    const r = mapEventToStats(ev({ type: 'usage_report', total_tokens: 4000, window_size: 10000, pct: 0.4 }))
    expect(r).toEqual({ strengthFood: 60 })
  })

  it('WorkerHeartbeat 在途 3/总容量 10 → strength = 70', () => {
    const r = mapEventToStats(ev({
      type: 'worker_heartbeat_state_changed',
      inflight_count: 3,
      max_concurrency: 10
    }))
    expect(r).toEqual({ strength: 70 })
  })

  it('WorkerHeartbeat 容量 0 → 不更新（避免除零）', () => {
    const r = mapEventToStats(ev({
      type: 'worker_heartbeat_state_changed',
      inflight_count: 0,
      max_concurrency: 0
    }))
    expect(r).toEqual({})
  })

  it('DeliverableAccepted → feeling +5（绝对值，非 diff，需当前值）', () => {
    // 简化：mapper 返绝对值时不需要当前值；用 increment 字段表示
    const r = mapEventToStats(ev({ type: 'deliverable_accepted', deliverable_id: 'd1' }))
    expect(r).toEqual({ feelingDelta: 5, likabilityDelta: 1 })
  })

  it('UserPrompted → strengthDrink 重置 100 + likability +1', () => {
    const r = mapEventToStats(ev({ type: 'user_prompted', text: 'hi' }))
    expect(r).toEqual({ strengthDrink: 100, likabilityDelta: 1 })
  })

  it('XuannvContextWatermark handoff_offer → strengthFood = 0（强提示）', () => {
    const r = mapEventToStats(ev({
      type: 'xuannv_context_watermark',
      threshold_pct: 0.45,
      action: 'handoff_offer'
    }))
    expect(r).toEqual({ strengthFood: 0 })
  })

  it('未识别事件 → 空对象', () => {
    const r = mapEventToStats(ev({ type: 'some_unknown_event' }))
    expect(r).toEqual({})
  })
})

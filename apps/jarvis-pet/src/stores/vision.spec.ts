import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useVisionStore } from './vision'

describe('vision store · 禁眼状态机', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
    vi.setSystemTime(new Date('2026-05-14T10:00:00Z'))
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('默认允许：disabled=false', () => {
    const v = useVisionStore()
    expect(v.disabled).toBe(false)
    expect(v.disabledUntil).toBeNull()
  })

  it('禁眼 15 分钟：开启时 disabled=true', () => {
    const v = useVisionStore()
    v.disableFor(15 * 60_000)
    expect(v.disabled).toBe(true)
    expect(typeof v.disabledUntil).toBe('number')
  })

  it('禁眼 15 分钟：14 分钟后仍禁，15 分钟过期解禁', () => {
    const v = useVisionStore()
    v.disableFor(15 * 60_000)
    vi.advanceTimersByTime(14 * 60_000)
    expect(v.disabled).toBe(true)
    vi.advanceTimersByTime(2 * 60_000) // 共 16 分钟
    expect(v.disabled).toBe(false)
    expect(v.disabledUntil).toBeNull()  // getter 自洁过期标记
  })

  it('永久禁眼：disabledUntil="forever"，永远 disabled', () => {
    const v = useVisionStore()
    v.disableForever()
    expect(v.disabled).toBe(true)
    expect(v.disabledUntil).toBe('forever')
    // 1 小时已经远超任何 disableFor 的合法值；365 天会让 setInterval 排
    // 队 31M 次拖慢 vitest，这里 1h 验证够用
    vi.advanceTimersByTime(60 * 60_000)
    expect(v.disabled).toBe(true)
  })

  it('enable() 立即解禁', () => {
    const v = useVisionStore()
    v.disableForever()
    expect(v.disabled).toBe(true)
    v.enable()
    expect(v.disabled).toBe(false)
    expect(v.disabledUntil).toBeNull()
  })

  it('capture 状态：idle → capturing → idle', () => {
    const v = useVisionStore()
    expect(v.capturing).toBe(false)
    v.markCapturing()
    expect(v.capturing).toBe(true)
    v.markIdle()
    expect(v.capturing).toBe(false)
  })

  it('剩余秒数：disableFor(15min) 后 remainingSec ≈ 900', () => {
    const v = useVisionStore()
    v.disableFor(15 * 60_000)
    expect(v.remainingSec).toBe(900)
    vi.advanceTimersByTime(60_000)
    expect(v.remainingSec).toBe(840)
  })

  it('剩余秒数：永久 → null；未禁 → null', () => {
    const v = useVisionStore()
    expect(v.remainingSec).toBeNull()
    v.disableForever()
    expect(v.remainingSec).toBeNull()
  })
})

import { describe, it, expect, beforeEach } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { useStatsStore, calMode } from './stats'

describe('stats store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('初始值符合 VPet 默认', () => {
    const s = useStatsStore()
    expect(s.strength).toBe(100)
    expect(s.strengthFood).toBe(100)
    expect(s.strengthDrink).toBe(100)
    expect(s.feeling).toBe(60)
    expect(s.health).toBe(100)
    expect(s.likability).toBe(0)
    expect(s.money).toBe(100)
  })

  it('update 部分字段不影响其他', () => {
    const s = useStatsStore()
    s.update({ strength: 50, feeling: 80 })
    expect(s.strength).toBe(50)
    expect(s.feeling).toBe(80)
    expect(s.strengthFood).toBe(100)
  })

  it('clamp 到 0~100（除 likability/money 外）', () => {
    const s = useStatsStore()
    s.update({ strength: 150, feeling: -10, likability: 999, money: 1000 })
    expect(s.strength).toBe(100)
    expect(s.feeling).toBe(0)
    expect(s.likability).toBe(999)
    expect(s.money).toBe(1000)
  })
})

describe('calMode', () => {
  it('Ill: health ≤ 30', () => {
    expect(calMode({ health: 30, feeling: 70 })).toBe('Ill')
    expect(calMode({ health: 0, feeling: 70 })).toBe('Ill')
  })

  it('PoorCondition: health ≤ 60 OR feeling ≤ 45', () => {
    expect(calMode({ health: 60, feeling: 70 })).toBe('PoorCondition')
    expect(calMode({ health: 80, feeling: 45 })).toBe('PoorCondition')
    expect(calMode({ health: 80, feeling: 30 })).toBe('PoorCondition')
  })

  it('Happy: feeling ≥ 90', () => {
    expect(calMode({ health: 100, feeling: 90 })).toBe('Happy')
  })

  it('Normal: 其它', () => {
    expect(calMode({ health: 80, feeling: 60 })).toBe('Normal')
  })
})

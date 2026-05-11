import { defineStore } from 'pinia'
import { ref } from 'vue'

/// VPet 风格 ModeType —— 抄 VPet GraphInfo.cs CalMode()
export type ModeType = 'Ill' | 'PoorCondition' | 'Normal' | 'Happy'

/// VPet 风格 6 维数值 + likability + money。范围：
/// - strength/strengthFood/strengthDrink/feeling/health: 0-100 clamp
/// - likability/money: 累加 ∞，不 clamp
export const useStatsStore = defineStore('stats', () => {
  const strength = ref(100)
  const strengthFood = ref(100)
  const strengthDrink = ref(100)
  const feeling = ref(60)
  const health = ref(100)
  const likability = ref(0)
  const money = ref(100)

  function clamp01(v: number): number {
    return Math.max(0, Math.min(100, v))
  }

  function update(diff: Partial<{
    strength: number
    strengthFood: number
    strengthDrink: number
    feeling: number
    health: number
    likability: number
    money: number
  }>): void {
    if (diff.strength !== undefined) strength.value = clamp01(diff.strength)
    if (diff.strengthFood !== undefined) strengthFood.value = clamp01(diff.strengthFood)
    if (diff.strengthDrink !== undefined) strengthDrink.value = clamp01(diff.strengthDrink)
    if (diff.feeling !== undefined) feeling.value = clamp01(diff.feeling)
    if (diff.health !== undefined) health.value = clamp01(diff.health)
    if (diff.likability !== undefined) likability.value = diff.likability
    if (diff.money !== undefined) money.value = diff.money
  }

  return { strength, strengthFood, strengthDrink, feeling, health, likability, money, update }
})

/// 抄 VPet GraphInfo.cs CalMode()
export function calMode(s: { health: number; feeling: number }): ModeType {
  if (s.health <= 30) return 'Ill'
  if (s.health <= 60 || s.feeling <= 45) return 'PoorCondition'
  if (s.feeling >= 90) return 'Happy'
  return 'Normal'
}

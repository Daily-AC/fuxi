import { defineStore } from 'pinia'
import { computed, onScopeDispose, ref } from 'vue'

/// 玄女眼睛 v1 · 桌宠端隐私 + 状态点 store。
///
/// `disabledUntil`:
///  - `null`：允许采帧（默认）
///  - `number`：unix-ms 截止；过期后 getter `disabled` 自动归 false 并清空标记
///  - `'forever'`：永久禁眼，重启失效（不写 localStorage 是有意——重启即解禁，
///    防止用户「禁了忘了」长期黑屏）
///
/// 内部用 `now` ref + 1s setInterval 让 `disabled` / `remainingSec` 在 Vue
/// 模板里随时间自动 reactive 重算（computed 不会自动跟 `Date.now()` 走，
/// 必须有响应式依赖，否则菜单标签不会自己刷）。
export type DisabledUntil = number | 'forever' | null

export const useVisionStore = defineStore('vision', () => {
  const disabledUntilRef = ref<DisabledUntil>(null)
  const capturing = ref(false)
  const now = ref(Date.now())

  // tick：每 1s 推一次 now，让所有时间 computed 重算。setInterval 比 setTimeout
  // 链简单，scope dispose 时 clearInterval 防泄漏（vitest 的 fake timers 也认
  // setInterval，advanceTimersByTime 会推它）。
  const tickHandle = setInterval(() => { now.value = Date.now() }, 1000)
  onScopeDispose(() => clearInterval(tickHandle))

  const disabled = computed<boolean>(() => {
    const u = disabledUntilRef.value
    if (u === null) return false
    if (u === 'forever') return true
    if (now.value >= u) {
      // 过期 self-clear——避免菜单标签一直显「已禁」误导用户。
      // 直接在 computed 副作用里改 ref：触发下次 getter 自洽
      disabledUntilRef.value = null
      return false
    }
    return true
  })

  // 暴露给菜单 UI 显示用——getter 内联 expire-clear，让模板里 `disabledUntil`
  // 永远跟 `disabled` 真实同步
  const disabledUntil = computed<DisabledUntil>(() => {
    void disabled.value
    return disabledUntilRef.value
  })

  const remainingSec = computed<number | null>(() => {
    const u = disabledUntilRef.value
    if (typeof u !== 'number') return null
    const ms = u - now.value
    return ms > 0 ? Math.floor(ms / 1000) : 0
  })

  function disableFor(ms: number): void {
    disabledUntilRef.value = Date.now() + ms
    now.value = Date.now()
  }

  function disableForever(): void {
    disabledUntilRef.value = 'forever'
  }

  function enable(): void {
    disabledUntilRef.value = null
  }

  function markCapturing(): void {
    capturing.value = true
  }

  function markIdle(): void {
    capturing.value = false
  }

  return {
    disabled,
    disabledUntil,
    remainingSec,
    capturing,
    disableFor,
    disableForever,
    enable,
    markCapturing,
    markIdle,
  }
})

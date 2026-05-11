<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useStatsStore } from '@/stores/stats'
import { PixiApp } from '@/pixi/PixiApp'
import { AnimationPlayer } from '@/sprites/AnimationPlayer'
import type { SpriteSet } from '@/sprites/SpriteSet'
import { FuxiClient } from '@/api/fuxiClient'
import { mapEventToStats } from '@/behavior/statsMapper'

const canvasContainer = ref<HTMLDivElement | null>(null)
const stats = useStatsStore()

let pixiApp: PixiApp | null = null
let player: AnimationPlayer | null = null
let fuxi: FuxiClient | null = null

const PANEL_W = 280
const PANEL_H = 420

// Phase 1 hardcode 一组 Default sprite set；后续 manifest.lps 走 ManifestLoader 加载
const DEFAULT_SET: SpriteSet = {
  graph: 'Default',
  animat: 'Single',
  mode: 'Normal',
  loop: true,
  frames: [
    { textureUrl: '/sprites/xuannv/default/default_001_120.png', durationMs: 120 },
    { textureUrl: '/sprites/xuannv/default/default_002_120.png', durationMs: 120 },
    { textureUrl: '/sprites/xuannv/default/default_003_120.png', durationMs: 120 },
    { textureUrl: '/sprites/xuannv/default/default_004_120.png', durationMs: 120 },
    { textureUrl: '/sprites/xuannv/default/default_005_120.png', durationMs: 120 },
    { textureUrl: '/sprites/xuannv/default/default_006_120.png', durationMs: 120 }
  ]
}

onMounted(async () => {
  pixiApp = new PixiApp()
  const canvas = await pixiApp.init({ width: PANEL_W, height: PANEL_H })
  canvasContainer.value!.appendChild(canvas)

  player = new AnimationPlayer(pixiApp.pixi)
  await player.load(DEFAULT_SET)

  // 接 fuxi —— Phase 1 不带 token，本地连或公开 ws
  fuxi = new FuxiClient({
    baseURL: import.meta.env.VITE_FUXI_BASE_URL || 'https://im.qmledmq.cn:8443',
    onEvent: ev => {
      const update = mapEventToStats(ev)
      // 应用 setter 字段
      const setterFields: Partial<Record<string, number>> = {}
      for (const [k, v] of Object.entries(update)) {
        if (!k.endsWith('Delta') && typeof v === 'number') {
          setterFields[k] = v
        }
      }
      if (Object.keys(setterFields).length > 0) {
        stats.update(setterFields as Parameters<typeof stats.update>[0])
      }
      // 应用 delta 字段
      if (update.feelingDelta !== undefined) {
        stats.update({ feeling: stats.feeling + update.feelingDelta })
      }
      if (update.likabilityDelta !== undefined) {
        stats.update({ likability: stats.likability + update.likabilityDelta })
      }
    }
  })
  fuxi.connect()
})

onBeforeUnmount(() => {
  fuxi?.stop()
  player?.destroy()
  pixiApp?.destroy()
})

function onPointerDown(e: PointerEvent) {
  if (e.button !== 0) return
  if ((e.target as HTMLElement).closest('.debug-overlay')) return
  getCurrentWindow().startDragging().catch(err => {
    console.warn('[drag] startDragging failed', err)
  })
}
</script>

<template>
  <div class="pet-canvas" ref="canvasContainer" @pointerdown="onPointerDown">
    <!-- debug overlay：phase 1 显示 6 维数值方便观察 -->
    <div class="debug-overlay">
      <div>体力 {{ stats.strength }}</div>
      <div>饱腹 {{ stats.strengthFood }}</div>
      <div>口渴 {{ stats.strengthDrink }}</div>
      <div>心情 {{ stats.feeling }}</div>
      <div>健康 {{ stats.health }}</div>
      <div>好感 {{ stats.likability }}</div>
      <div>金钱 {{ stats.money }}</div>
    </div>
  </div>
</template>

<style scoped>
.pet-canvas {
  position: relative;
  width: 280px;
  height: 420px;
}
.debug-overlay {
  position: absolute;
  top: 0;
  left: 0;
  font-family: monospace;
  font-size: 10px;
  color: rgba(110, 136, 150, 0.7);
  pointer-events: none;
  user-select: none;
  background: rgba(250, 247, 241, 0.06);
  padding: 4px;
  border-radius: 4px;
}
</style>

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
const sizeDebug = ref('size: -')

let pixiApp: PixiApp | null = null
let player: AnimationPlayer | null = null
let fuxi: FuxiClient | null = null

const PANEL_W = 200
const PANEL_H = 360

// Phase 1 hardcode 一组 Default sprite set；后续 manifest.lps 走 ManifestLoader 加载
// 资源来自 VPet 0000_core mod 的 Default/Nomal/1（萝莉斯默认 idle 循环）
// 帧时长按 VPet 原文件名 `_<idx>_<dur>.png` 解出（250/125 不齐——VPet 原作美术节奏）
const DEFAULT_SET: SpriteSet = {
  graph: 'Default',
  animat: 'Single',
  mode: 'Nomal',
  loop: true,
  frames: [
    { textureUrl: '/sprites/loris/default/nomal/1/_000_250.png', durationMs: 250 },
    { textureUrl: '/sprites/loris/default/nomal/1/_001_125.png', durationMs: 125 },
    { textureUrl: '/sprites/loris/default/nomal/1/_002_125.png', durationMs: 125 },
    { textureUrl: '/sprites/loris/default/nomal/1/_003_375.png', durationMs: 375 },
    { textureUrl: '/sprites/loris/default/nomal/1/_004_125.png', durationMs: 125 },
    { textureUrl: '/sprites/loris/default/nomal/1/_005_250.png', durationMs: 250 },
    { textureUrl: '/sprites/loris/default/nomal/1/_006_125.png', durationMs: 125 },
    { textureUrl: '/sprites/loris/default/nomal/1/_007_125.png', durationMs: 125 }
  ]
}

onMounted(async () => {
  pixiApp = new PixiApp()
  const canvas = await pixiApp.init({ width: PANEL_W, height: PANEL_H })
  canvasContainer.value!.appendChild(canvas)

  player = new AnimationPlayer(pixiApp.pixi)
  await player.load(DEFAULT_SET)
  // size debug：写到 overlay 给 headless agent 通过截图回传 PIXI 实际尺寸
  const px = pixiApp.pixi
  sizeDebug.value = `screen ${px.screen.width}x${px.screen.height} canvas ${px.canvas.width}x${px.canvas.height} client ${px.canvas.clientWidth}x${px.canvas.clientHeight} dpr ${window.devicePixelRatio}`

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
      <div>{{ sizeDebug }}</div>
    </div>
  </div>
</template>

<style scoped>
.pet-canvas {
  position: relative;
  width: 200px;
  height: 360px;
  background: transparent !important;
}
.pet-canvas :deep(canvas) {
  background: transparent !important;
  display: block;
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

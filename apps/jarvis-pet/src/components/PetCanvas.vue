<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useStatsStore } from '@/stores/stats'
import { PixiApp } from '@/pixi/PixiApp'
import { AnimationPlayer } from '@/sprites/AnimationPlayer'
import type { SpriteSet } from '@/sprites/SpriteSet'
import { FuxiClient } from '@/api/fuxiClient'
import { mapEventToStats } from '@/behavior/statsMapper'

// 8 帧通过 Vite ?url 显式导入：dev 下 vite 直接服务真实文件，build 时 hash 进 dist/assets。
// 走 publicDir + /sprites 绝对路径在 Tauri release 下会撞「tauri://sprites/...」host 误读
// （WebKit URL parser 见 leading-slash 找不到 base host 时把第一段当 host）—— ?url 由
// vite 产模块级 URL，避免绝对路径解析坑。
import f000 from '../../resources/sprites/loris/default/nomal/1/_000_250.png?url'
import f001 from '../../resources/sprites/loris/default/nomal/1/_001_125.png?url'
import f002 from '../../resources/sprites/loris/default/nomal/1/_002_125.png?url'
import f003 from '../../resources/sprites/loris/default/nomal/1/_003_375.png?url'
import f004 from '../../resources/sprites/loris/default/nomal/1/_004_125.png?url'
import f005 from '../../resources/sprites/loris/default/nomal/1/_005_250.png?url'
import f006 from '../../resources/sprites/loris/default/nomal/1/_006_125.png?url'
import f007 from '../../resources/sprites/loris/default/nomal/1/_007_125.png?url'

const canvasContainer = ref<HTMLDivElement | null>(null)
const stats = useStatsStore()
const sizeDebug = ref('size: -')
const showMenu = ref(false)
const toast = ref('')

let toastTimer: number | null = null
function flashToast(msg: string, ms = 1500) {
  toast.value = msg
  if (toastTimer != null) clearTimeout(toastTimer)
  toastTimer = window.setTimeout(() => { toast.value = '' }, ms)
}

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
    { textureUrl: f000, durationMs: 250 },
    { textureUrl: f001, durationMs: 125 },
    { textureUrl: f002, durationMs: 125 },
    { textureUrl: f003, durationMs: 375 },
    { textureUrl: f004, durationMs: 125 },
    { textureUrl: f005, durationMs: 250 },
    { textureUrl: f006, durationMs: 125 },
    { textureUrl: f007, durationMs: 125 }
  ]
}

onMounted(async () => {
  pixiApp = new PixiApp()
  const canvas = await pixiApp.init({ width: PANEL_W, height: PANEL_H })
  canvasContainer.value!.appendChild(canvas)

  player = new AnimationPlayer(pixiApp.pixi)
  await player.load(DEFAULT_SET)
  const px = pixiApp.pixi
  sizeDebug.value = `${px.screen.width}x${px.screen.height} dpr ${window.devicePixelRatio}`

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
  // 右键 toggle 菜单
  if (e.button === 2) {
    e.preventDefault()
    showMenu.value = !showMenu.value
    return
  }
  if (e.button !== 0) return
  // 菜单内点击不拖（菜单 mousedown.stop 已挡，这里二保险）
  if (e.target instanceof HTMLElement && e.target.closest('.context-menu')) return
  // 左键点本体：先收菜单，再请求拖动；起手 flash 用于确认事件到达 + 失败可见
  showMenu.value = false
  flashToast('drag…', 600)
  getCurrentWindow().startDragging().catch(err => {
    flashToast(`drag err: ${String(err).slice(0, 60)}`, 3000)
  })
}

async function onQuit() {
  try {
    await getCurrentWindow().close()
  } catch (e) {
    flashToast(`close err: ${String(e).slice(0, 60)}`)
  }
}
</script>

<template>
  <div class="pet-canvas" ref="canvasContainer"
       @pointerdown="onPointerDown"
       @contextmenu.prevent>
    <!-- 数值菜单：默认隐藏，右键 toggle；mousedown.stop 防止落到外层触发拖动 -->
    <div v-if="showMenu" class="context-menu" @mousedown.stop>
      <div class="row">体力 {{ stats.strength }}</div>
      <div class="row">饱腹 {{ stats.strengthFood }}</div>
      <div class="row">口渴 {{ stats.strengthDrink }}</div>
      <div class="row">心情 {{ stats.feeling }}</div>
      <div class="row">健康 {{ stats.health }}</div>
      <div class="row">好感 {{ stats.likability }}</div>
      <div class="row">金钱 {{ stats.money }}</div>
      <div class="row size">{{ sizeDebug }}</div>
      <div class="sep"></div>
      <div class="row clickable" @click="onQuit">退出</div>
    </div>
    <!-- 短暂提示：drag 异常 / 操作反馈用 -->
    <div v-if="toast" class="toast">{{ toast }}</div>
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
.context-menu {
  position: absolute;
  top: 8px;
  right: 8px;
  font-family: -apple-system, sans-serif;
  font-size: 11px;
  color: rgba(40, 40, 40, 0.85);
  user-select: none;
  background: rgba(255, 255, 255, 0.92);
  backdrop-filter: blur(6px);
  border: 1px solid rgba(0, 0, 0, 0.08);
  padding: 6px 10px;
  border-radius: 6px;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.12);
  min-width: 90px;
}
.row {
  padding: 2px 0;
  line-height: 1.4;
}
.row.size {
  font-size: 9px;
  color: rgba(120, 120, 120, 0.7);
}
.row.clickable {
  cursor: pointer;
  color: rgb(180, 60, 60);
}
.row.clickable:hover {
  background: rgba(180, 60, 60, 0.08);
  border-radius: 3px;
}
.sep {
  height: 1px;
  background: rgba(0, 0, 0, 0.08);
  margin: 4px 0;
}
.toast {
  position: absolute;
  bottom: 8px;
  left: 8px;
  right: 8px;
  font-family: monospace;
  font-size: 9px;
  color: rgba(180, 60, 60, 0.9);
  background: rgba(255, 255, 255, 0.9);
  padding: 4px 6px;
  border-radius: 4px;
  pointer-events: none;
}
</style>

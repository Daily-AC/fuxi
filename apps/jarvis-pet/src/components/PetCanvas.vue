<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useStatsStore } from '@/stores/stats'
import { PixiApp } from '@/pixi/PixiApp'
import { AnimationPlayer } from '@/sprites/AnimationPlayer'
import type { SpriteSet } from '@/sprites/SpriteSet'
import { FuxiClient } from '@/api/fuxiClient'
import { mapEventToStats } from '@/behavior/statsMapper'
import { MicRecorder } from '@/voice/micRecorder'
import { AsrClient } from '@/voice/asrClient'
import { sendIntervene } from '@/api/intervene'

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
const bubble = ref('')         // 玄女回话气泡，>0s 显示，淡出
const voiceState = ref<'idle' | 'recording' | 'transcribing' | 'sending'>('idle')

const BASE_URL = import.meta.env.VITE_FUXI_BASE_URL || 'https://im.qmledmq.cn:8443'
const TOKEN_LS_KEY = 'jarvis-pet.pairToken'
const pairToken = ref<string>(localStorage.getItem(TOKEN_LS_KEY) || '')
const showTokenInput = ref(false)
const tokenDraft = ref('')

let toastTimer: number | null = null
function flashToast(msg: string, ms = 1500) {
  toast.value = msg
  if (toastTimer != null) clearTimeout(toastTimer)
  toastTimer = window.setTimeout(() => { toast.value = '' }, ms)
}

let bubbleTimer: number | null = null
function showBubble(text: string, ms = 6000) {
  bubble.value = text
  if (bubbleTimer != null) clearTimeout(bubbleTimer)
  bubbleTimer = window.setTimeout(() => { bubble.value = '' }, ms)
}

let pixiApp: PixiApp | null = null
let player: AnimationPlayer | null = null
let fuxi: FuxiClient | null = null
let mic: MicRecorder | null = null
let asr: AsrClient | null = null

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

  // 接 fuxi —— pairToken 从 localStorage 读，没的话 WS 仍连（开发期降级，
  // 生产用户在菜单设 token 后 reconnect 拿到鉴权）
  fuxi = new FuxiClient({
    baseURL: BASE_URL,
    pairToken: pairToken.value || undefined,
    onEvent: ev => {
      // 玄女说话：弹气泡（Phase 2.D MVP；Phase 2.E 加 TTS + Say 动画）
      if (ev.kind.type === 'xuannv_voice_line' && typeof ev.kind.text === 'string') {
        showBubble(ev.kind.text)
      }
      const update = mapEventToStats(ev)
      const setterFields: Partial<Record<string, number>> = {}
      for (const [k, v] of Object.entries(update)) {
        if (!k.endsWith('Delta') && typeof v === 'number') {
          setterFields[k] = v
        }
      }
      if (Object.keys(setterFields).length > 0) {
        stats.update(setterFields as Parameters<typeof stats.update>[0])
      }
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
  mic?.stop()
  asr?.abort()
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

function onSetToken() {
  // Tauri 2 webview 不支持 window.prompt（静默 noop），用内嵌模态
  tokenDraft.value = pairToken.value
  showTokenInput.value = true
  showMenu.value = false
}

function onSaveToken() {
  const trimmed = tokenDraft.value.trim()
  if (!trimmed) {
    localStorage.removeItem(TOKEN_LS_KEY)
    pairToken.value = ''
    flashToast('token 已清')
  } else {
    localStorage.setItem(TOKEN_LS_KEY, trimmed)
    pairToken.value = trimmed
    flashToast('token 已存，重启生效', 2500)
  }
  showTokenInput.value = false
  tokenDraft.value = ''
}

function onCancelToken() {
  showTokenInput.value = false
  tokenDraft.value = ''
}

async function onTalkToggle() {
  if (!pairToken.value) {
    flashToast('先设 token', 2500)
    return
  }
  if (voiceState.value === 'idle') {
    // 起录音 + ASR WS
    try {
      voiceState.value = 'recording'
      showMenu.value = false
      asr = new AsrClient({ baseURL: BASE_URL, token: pairToken.value })
      await asr.connect()
      mic = new MicRecorder()
      await mic.start(chunk => asr?.sendPcm(chunk))
      flashToast('🎤 录音中（再次点击送出）', 4000)
    } catch (e) {
      voiceState.value = 'idle'
      mic?.stop()
      asr?.abort()
      mic = null
      asr = null
      flashToast(`录音起失败: ${String(e).slice(0, 50)}`, 3000)
    }
  } else if (voiceState.value === 'recording') {
    // 停录音 → finish ASR → intervene
    try {
      voiceState.value = 'transcribing'
      flashToast('转写中...')
      mic?.stop()
      mic = null
      const result = await asr!.finish()
      asr = null
      const text = result.text.trim()
      if (!text) {
        voiceState.value = 'idle'
        flashToast('没听到', 1500)
        return
      }
      flashToast(`你：${text}`, 3500)
      voiceState.value = 'sending'
      await sendIntervene({ baseURL: BASE_URL, token: pairToken.value, text })
      voiceState.value = 'idle'
    } catch (e) {
      voiceState.value = 'idle'
      flashToast(`失败：${String(e).slice(0, 60)}`, 3500)
    }
  }
}
</script>

<template>
  <div class="pet-canvas" ref="canvasContainer"
       @pointerdown="onPointerDown"
       @contextmenu.prevent>
    <!-- 数值菜单：默认隐藏，右键 toggle；mousedown.stop 防止落到外层触发拖动 -->
    <div v-if="showMenu" class="context-menu" @mousedown.stop>
      <div class="row clickable talk" @click="onTalkToggle">
        {{ voiceState === 'recording' ? '🎤 录音中…点击送出' :
           voiceState === 'transcribing' ? '⏳ 转写中…' :
           voiceState === 'sending' ? '⏳ 发送中…' :
           '🎤 跟玄女说一句' }}
      </div>
      <div class="row clickable" @click="onSetToken">
        🔑 {{ pairToken ? '换 token' : '设 token' }}
      </div>
      <div class="sep"></div>
      <div class="row">体力 {{ stats.strength }}</div>
      <div class="row">饱腹 {{ stats.strengthFood }}</div>
      <div class="row">口渴 {{ stats.strengthDrink }}</div>
      <div class="row">心情 {{ stats.feeling }}</div>
      <div class="row">健康 {{ stats.health }}</div>
      <div class="row">好感 {{ stats.likability }}</div>
      <div class="row">金钱 {{ stats.money }}</div>
      <div class="row size">{{ sizeDebug }}</div>
      <div class="sep"></div>
      <div class="row clickable quit" @click="onQuit">退出</div>
    </div>
    <!-- Token 设置模态：Tauri 2 webview 没原生 prompt，自己画浮层 -->
    <div v-if="showTokenInput" class="token-modal" @mousedown.stop>
      <div class="token-title">粘贴 HMAC token</div>
      <textarea
        class="token-input"
        v-model="tokenDraft"
        rows="6"
        placeholder="eyJ... (30天有效，home: python3 ~/.fuxi/im-mint-token.py)"
        spellcheck="false"
      />
      <div class="token-actions">
        <button class="btn cancel" @click="onCancelToken">取消</button>
        <button class="btn save" @click="onSaveToken">保存</button>
      </div>
    </div>
    <!-- 玄女回话气泡：xuannv_voice_line 事件触发，自动 fade 6s -->
    <div v-if="bubble" class="bubble">{{ bubble }}</div>
    <!-- 短暂提示：drag 异常 / 操作反馈 -->
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
  color: rgba(40, 40, 40, 0.9);
}
.row.clickable:hover {
  background: rgba(0, 0, 0, 0.05);
  border-radius: 3px;
}
.row.clickable.talk {
  color: rgb(40, 100, 180);
  font-weight: 500;
}
.row.clickable.talk:hover {
  background: rgba(40, 100, 180, 0.08);
}
.row.clickable.quit {
  color: rgb(180, 60, 60);
}
.row.clickable.quit:hover {
  background: rgba(180, 60, 60, 0.08);
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
.bubble {
  position: absolute;
  top: 8px;
  left: 8px;
  right: 8px;
  font-family: -apple-system, sans-serif;
  font-size: 12px;
  line-height: 1.45;
  color: rgba(40, 40, 40, 0.92);
  background: rgba(255, 255, 245, 0.95);
  backdrop-filter: blur(6px);
  border: 1px solid rgba(0, 0, 0, 0.08);
  padding: 8px 10px;
  border-radius: 10px 10px 10px 2px;
  box-shadow: 0 3px 10px rgba(0, 0, 0, 0.12);
  pointer-events: none;
  animation: bubble-in 220ms ease-out;
}
@keyframes bubble-in {
  from { opacity: 0; transform: translateY(-4px) scale(0.94); }
  to   { opacity: 1; transform: translateY(0) scale(1); }
}
.token-modal {
  position: absolute;
  top: 8px;
  left: 8px;
  right: 8px;
  background: rgba(255, 255, 255, 0.97);
  backdrop-filter: blur(8px);
  border: 1px solid rgba(0, 0, 0, 0.1);
  border-radius: 8px;
  padding: 10px;
  box-shadow: 0 4px 14px rgba(0, 0, 0, 0.18);
  font-family: -apple-system, sans-serif;
}
.token-title {
  font-size: 11px;
  color: rgba(40, 40, 40, 0.85);
  margin-bottom: 6px;
}
.token-input {
  width: 100%;
  box-sizing: border-box;
  font-family: ui-monospace, monospace;
  font-size: 9px;
  line-height: 1.3;
  border: 1px solid rgba(0, 0, 0, 0.15);
  border-radius: 4px;
  padding: 6px;
  resize: none;
  word-break: break-all;
  background: rgba(248, 248, 248, 0.9);
}
.token-input:focus {
  outline: none;
  border-color: rgba(40, 100, 180, 0.6);
}
.token-actions {
  display: flex;
  gap: 6px;
  justify-content: flex-end;
  margin-top: 8px;
}
.btn {
  font-family: -apple-system, sans-serif;
  font-size: 11px;
  padding: 4px 12px;
  border-radius: 4px;
  border: 1px solid rgba(0, 0, 0, 0.12);
  cursor: pointer;
}
.btn.cancel {
  background: rgba(0, 0, 0, 0.03);
  color: rgba(40, 40, 40, 0.7);
}
.btn.save {
  background: rgb(40, 100, 180);
  color: white;
  border-color: rgb(40, 100, 180);
}
.btn:hover {
  filter: brightness(1.05);
}
</style>

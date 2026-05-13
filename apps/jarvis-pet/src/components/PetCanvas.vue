<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from 'vue'
import { getCurrentWindow, LogicalSize } from '@tauri-apps/api/window'
import { useStatsStore } from '@/stores/stats'
import { useVisionStore } from '@/stores/vision'
import { PixiApp } from '@/pixi/PixiApp'
import { AnimationPlayer } from '@/sprites/AnimationPlayer'
import type { SpriteSet } from '@/sprites/SpriteSet'
import { FuxiClient } from '@/api/fuxiClient'
import { mapEventToStats } from '@/behavior/statsMapper'
import { MicRecorder } from '@/voice/micRecorder'
import { AsrClient } from '@/voice/asrClient'
import { sendIntervene } from '@/api/intervene'
import { playTts } from '@/voice/tts'
import { WakeClient } from '@/voice/wakeClient'
import { EnergyVad } from '@/voice/vad'
import { captureForSeconds, pcmToWav, uploadEnroll, uploadVerify, SAMPLE_TEXT } from '@/voice/voiceprint'
import { captureWebcamFrame, captureScreenFrame, PermissionDeniedError, NoDeviceError } from '@/voice/visionCapture'
import { uploadVisionFrame } from '@/api/visionFrame'

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

// 摸头 B_Nomal 11 帧（VPet vup/Touch_Head/B_Nomal fork，去中文文件名）
import h000 from '../../resources/sprites/loris/touch_head/nomal/b/touch_head_000_125.png?url'
import h001 from '../../resources/sprites/loris/touch_head/nomal/b/touch_head_001_125.png?url'
import h002 from '../../resources/sprites/loris/touch_head/nomal/b/touch_head_002_125.png?url'
import h003 from '../../resources/sprites/loris/touch_head/nomal/b/touch_head_003_125.png?url'
import h004 from '../../resources/sprites/loris/touch_head/nomal/b/touch_head_004_125.png?url'
import h005 from '../../resources/sprites/loris/touch_head/nomal/b/touch_head_005_250.png?url'
import h006 from '../../resources/sprites/loris/touch_head/nomal/b/touch_head_006_125.png?url'
import h007 from '../../resources/sprites/loris/touch_head/nomal/b/touch_head_007_125.png?url'
import h008 from '../../resources/sprites/loris/touch_head/nomal/b/touch_head_008_125.png?url'
import h009 from '../../resources/sprites/loris/touch_head/nomal/b/touch_head_009_125.png?url'
import h010 from '../../resources/sprites/loris/touch_head/nomal/b/touch_head_010_125.png?url'

// Say/Serious/B 4 帧嘴动 loop（VPet 原 0000_core/pet/vup/Say/Serious/B fork）
// TTS 播放期间循环这 4 帧，播完 onEnd 切回 Default
import s000 from '../../resources/sprites/loris/say/serious/b/say_000_125.png?url'
import s001 from '../../resources/sprites/loris/say/serious/b/say_001_125.png?url'
import s002 from '../../resources/sprites/loris/say/serious/b/say_002_125.png?url'
import s003 from '../../resources/sprites/loris/say/serious/b/say_003_125.png?url'

// Phase 3 情绪 idle 帧 —— Default/Happy/1 共 13 帧（高兴 + 惊喜走这套）
import hp000 from '../../resources/sprites/loris/default/happy/1/_000_125.png?url'
import hp001 from '../../resources/sprites/loris/default/happy/1/_001_125.png?url'
import hp002 from '../../resources/sprites/loris/default/happy/1/_002_125.png?url'
import hp003 from '../../resources/sprites/loris/default/happy/1/_003_125.png?url'
import hp004 from '../../resources/sprites/loris/default/happy/1/_004_125.png?url'
import hp005 from '../../resources/sprites/loris/default/happy/1/_005_250.png?url'
import hp006 from '../../resources/sprites/loris/default/happy/1/_006_125.png?url'
import hp007 from '../../resources/sprites/loris/default/happy/1/_007_125.png?url'
import hp008 from '../../resources/sprites/loris/default/happy/1/_008_125.png?url'
import hp009 from '../../resources/sprites/loris/default/happy/1/_009_125.png?url'
import hp010 from '../../resources/sprites/loris/default/happy/1/_010_125.png?url'
import hp011 from '../../resources/sprites/loris/default/happy/1/_011_125.png?url'
import hp012 from '../../resources/sprites/loris/default/happy/1/_012_250.png?url'

// Phase 3 · Default/PoorCondition/1 共 17 帧（担心 + 难过走这套）
import pc000 from '../../resources/sprites/loris/default/poorcondition/1/_000_125.png?url'
import pc001 from '../../resources/sprites/loris/default/poorcondition/1/_001_125.png?url'
import pc002 from '../../resources/sprites/loris/default/poorcondition/1/_002_125.png?url'
import pc003 from '../../resources/sprites/loris/default/poorcondition/1/_003_125.png?url'
import pc004 from '../../resources/sprites/loris/default/poorcondition/1/_004_375.png?url'
import pc005 from '../../resources/sprites/loris/default/poorcondition/1/_005_125.png?url'
import pc006 from '../../resources/sprites/loris/default/poorcondition/1/_006_125.png?url'
import pc007 from '../../resources/sprites/loris/default/poorcondition/1/_007_125.png?url'
import pc008 from '../../resources/sprites/loris/default/poorcondition/1/_008_500.png?url'
import pc009 from '../../resources/sprites/loris/default/poorcondition/1/_009_125.png?url'
import pc010 from '../../resources/sprites/loris/default/poorcondition/1/_010_125.png?url'
import pc011 from '../../resources/sprites/loris/default/poorcondition/1/_011_125.png?url'
import pc012 from '../../resources/sprites/loris/default/poorcondition/1/_012_375.png?url'
import pc013 from '../../resources/sprites/loris/default/poorcondition/1/_013_375.png?url'
import pc014 from '../../resources/sprites/loris/default/poorcondition/1/_014_125.png?url'
import pc015 from '../../resources/sprites/loris/default/poorcondition/1/_015_125.png?url'
import pc016 from '../../resources/sprites/loris/default/poorcondition/1/_016_125.png?url'

const canvasContainer = ref<HTMLDivElement | null>(null)
const stats = useStatsStore()
const vision = useVisionStore()
const visionError = ref(false)   // 红色短暂提示位（capture / upload 失败 2s 内置 true）
const sizeDebug = ref('size: -')
const showMenu = ref(false)
const toast = ref('')
const bubble = ref('')         // 玄女回话气泡，>0s 显示，淡出
const voiceState = ref<'idle' | 'recording' | 'transcribing' | 'sending'>('idle')

// 声纹注册 modal 状态
type VpStage = 'idle' | 'guide' | 'recording' | 'uploading' | 'verifying' | 'done' | 'error'
const vpStage = ref<VpStage>('idle')
const vpElapsedSec = ref(0)
const vpRms = ref(0)
const vpResult = ref<string>('')
const VP_DURATION_SEC = 20
const VP_SAMPLE_TEXT = SAMPLE_TEXT

const BASE_URL = import.meta.env.VITE_FUXI_BASE_URL || 'https://im.qmledmq.cn:8443'
const TOKEN_LS_KEY = 'jarvis-pet.pairToken'
const WAKE_TOKEN_LS_KEY = 'jarvis-pet.wakeToken'
const WAKE_EN_LS_KEY = 'jarvis-pet.wakeEnabled'
const pairToken = ref<string>(localStorage.getItem(TOKEN_LS_KEY) || '')
const wakeToken = ref<string>(localStorage.getItem(WAKE_TOKEN_LS_KEY) || '')
const wakeEnabled = ref<boolean>(localStorage.getItem(WAKE_EN_LS_KEY) === '1')
const wakeStatus = ref<'off' | 'connecting' | 'ready' | 'disconnected'>('off')
const showTokenInput = ref(false)
const tokenDraft = ref('')
const wakeTokenDraft = ref('')

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
let wake: WakeClient | null = null
let vad: EnergyVad | null = null
let asrPcmUnsub: (() => void) | null = null
let wakePcmUnsub: (() => void) | null = null

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

/// 说话嘴动 4 帧 loop（VPet vup/Say/Serious/B）；TTS 播放期间持续 loop，
/// onEnd 时 load 回 DEFAULT_SET
const SAY_SET: SpriteSet = {
  graph: 'Say',
  animat: 'B',
  mode: 'Serious',
  loop: true,
  frames: [
    { textureUrl: s000, durationMs: 125 },
    { textureUrl: s001, durationMs: 125 },
    { textureUrl: s002, durationMs: 125 },
    { textureUrl: s003, durationMs: 125 }
  ]
}

/// Phase 3 · Default/Happy/1 13 帧 loop（高兴 / 惊喜 idle）
const HAPPY_SET: SpriteSet = {
  graph: 'Default',
  animat: 'Single',
  mode: 'Happy',
  loop: true,
  frames: [
    { textureUrl: hp000, durationMs: 125 },
    { textureUrl: hp001, durationMs: 125 },
    { textureUrl: hp002, durationMs: 125 },
    { textureUrl: hp003, durationMs: 125 },
    { textureUrl: hp004, durationMs: 125 },
    { textureUrl: hp005, durationMs: 250 },
    { textureUrl: hp006, durationMs: 125 },
    { textureUrl: hp007, durationMs: 125 },
    { textureUrl: hp008, durationMs: 125 },
    { textureUrl: hp009, durationMs: 125 },
    { textureUrl: hp010, durationMs: 125 },
    { textureUrl: hp011, durationMs: 125 },
    { textureUrl: hp012, durationMs: 250 }
  ]
}

/// Phase 3 · Default/PoorCondition/1 17 帧 loop（担心 / 难过 idle）
const POOR_CONDITION_SET: SpriteSet = {
  graph: 'Default',
  animat: 'Single',
  mode: 'PoorCondition',
  loop: true,
  frames: [
    { textureUrl: pc000, durationMs: 125 },
    { textureUrl: pc001, durationMs: 125 },
    { textureUrl: pc002, durationMs: 125 },
    { textureUrl: pc003, durationMs: 125 },
    { textureUrl: pc004, durationMs: 375 },
    { textureUrl: pc005, durationMs: 125 },
    { textureUrl: pc006, durationMs: 125 },
    { textureUrl: pc007, durationMs: 125 },
    { textureUrl: pc008, durationMs: 500 },
    { textureUrl: pc009, durationMs: 125 },
    { textureUrl: pc010, durationMs: 125 },
    { textureUrl: pc011, durationMs: 125 },
    { textureUrl: pc012, durationMs: 375 },
    { textureUrl: pc013, durationMs: 375 },
    { textureUrl: pc014, durationMs: 125 },
    { textureUrl: pc015, durationMs: 125 },
    { textureUrl: pc016, durationMs: 125 }
  ]
}

/// 摸头 B_Nomal 11 帧 loop（VPet 原作）；playOnce 完后回 DEFAULT_SET
const TOUCH_HEAD_SET: SpriteSet = {
  graph: 'Touch_Head',
  animat: 'B',
  mode: 'Nomal',
  loop: false,
  frames: [
    { textureUrl: h000, durationMs: 125 },
    { textureUrl: h001, durationMs: 125 },
    { textureUrl: h002, durationMs: 125 },
    { textureUrl: h003, durationMs: 125 },
    { textureUrl: h004, durationMs: 125 },
    { textureUrl: h005, durationMs: 250 },
    { textureUrl: h006, durationMs: 125 },
    { textureUrl: h007, durationMs: 125 },
    { textureUrl: h008, durationMs: 125 },
    { textureUrl: h009, durationMs: 125 },
    { textureUrl: h010, durationMs: 125 }
  ]
}

onMounted(async () => {
  console.log('[boot] onMounted start')
  try {
    pixiApp = new PixiApp()
    const canvas = await pixiApp.init({ width: PANEL_W, height: PANEL_H })
    console.log('[boot] pixi init done')
    canvasContainer.value!.appendChild(canvas)
    player = new AnimationPlayer(pixiApp.pixi)
    await player.load(DEFAULT_SET)
    console.log('[boot] player loaded default')
    const px = pixiApp.pixi
    sizeDebug.value = `${px.screen.width}x${px.screen.height} dpr ${window.devicePixelRatio}`
  } catch (e) {
    console.error('[boot] init err:', String(e))
    throw e
  }

  // 接 fuxi —— pairToken 从 localStorage 读，没的话 WS 仍连（开发期降级，
  // 生产用户在菜单设 token 后 reconnect 拿到鉴权）
  fuxi = new FuxiClient({
    baseURL: BASE_URL,
    pairToken: pairToken.value || undefined,
    onEvent: ev => {
      // 调试：所有事件 type 闪一下，看玄女实际发了啥（say / agent_responded / etc）
      const evType = ev.kind.type
      console.log('[wsEvent]', evType, ev.kind)
      if (evType === 'xuannv_voice_line' || evType === 'agent_responded' ||
          evType === 'thinking_started' || evType === 'thinking_finished') {
        flashToast(`ws: ${evType}`, 1500)
      }
      // 玄女眼睛：召唤式抓帧请求。禁眼时静默不上传——后端 timeout 退化成 stderr，
      // 玄女按 spec 错误兜底矩阵告诉用户「眼睛被蒙了」
      if (ev.kind.type === 'vision_request' &&
          typeof (ev.kind as { request_id?: unknown }).request_id === 'string' &&
          ((ev.kind as { target?: unknown }).target === 'webcam' ||
           (ev.kind as { target?: unknown }).target === 'screen')) {
        const reqId = (ev.kind as { request_id: string }).request_id
        const target = (ev.kind as { target: 'webcam' | 'screen' }).target
        handleVisionRequest(reqId, target).catch(e => {
          console.error('[vision] handle err', e)
          visionError.value = true
          window.setTimeout(() => { visionError.value = false }, 2000)
        })
      }
      // 玄女说话：弹气泡 + 嘴动 + TTS 派蒙音（Phase 2.D+E）+ 情绪 ref/sprite 切（Phase 3）
      if (ev.kind.type === 'xuannv_voice_line' && typeof ev.kind.text === 'string') {
        const sayText = ev.kind.text
        // emotion 可能 undefined / null / string——只在是非空 string 时透传，
        // 后端缺 ref 时也会 fallback normal，所以这里宽松收
        const rawEmotion = ev.kind.emotion
        const emotion = typeof rawEmotion === 'string' && rawEmotion.length > 0
          ? rawEmotion
          : undefined
        showBubble(sayText)
        speakWithMouth(sayText, emotion).catch(e => {
          flashToast(`tts err: ${String(e).slice(0, 80)}`, 5000)
        })
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

  // 上次启用过 wake 词且 token 还在 → 自动续上（开机自动监听）
  console.log('[boot] wakeEnabled=', wakeEnabled.value, 'wakeTokenLen=', wakeToken.value.length, 'pairTokenLen=', pairToken.value.length)
  if (wakeEnabled.value && wakeToken.value) {
    enableWake().catch(e => flashToast(`wake 自启失败: ${String(e).slice(0, 50)}`, 3000))
  } else if (wakeEnabled.value && !wakeToken.value) {
    console.warn('[boot] wakeEnabled but no wakeToken — 设置里第二行没填')
  }
})

onBeforeUnmount(() => {
  wake?.stop()
  asr?.abort()
  mic?.stop()
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

let touchBusy = false
/// 双击萝莉斯 → 播 Touch_Head 动画 + 推一条 "[摸了摸头]" 给玄女让她回应
async function onTouchHead() {
  if (touchBusy || !player) return
  touchBusy = true
  try {
    showMenu.value = false
    // 动画跑一遍（11 帧 ~1.4s）+ 自动回 Default
    await player.playOnce(TOUCH_HEAD_SET, DEFAULT_SET)
    // 异步发给玄女不阻塞动画
    if (pairToken.value) {
      // [语音] 前缀让玄女走 say 走 TTS（公理 #8），桌宠才能拿到 voice_line 事件
      sendIntervene({
        baseURL: BASE_URL,
        token: pairToken.value,
        text: '[语音] （用户摸了摸你的头）'
      }).catch(e => flashToast(`摸头消息发失败: ${String(e).slice(0, 50)}`, 2500))
    }
  } finally {
    touchBusy = false
  }
}

function onEyeAllow() {
  vision.enable()
  flashToast('眼睛已允许', 1200)
}
function onEyeDisable15() {
  vision.disableFor(15 * 60_000)
  flashToast('眼睛禁 15 分钟', 1500)
}
function onEyeDisableForever() {
  // 切回 toggle：已永久禁 → 解禁；否则永久禁
  if (vision.disabledUntil === 'forever') {
    vision.enable()
    flashToast('眼睛已恢复', 1200)
  } else {
    vision.disableForever()
    flashToast('眼睛永久禁（重启失效）', 2000)
  }
}

function onSetToken() {
  // Tauri 2 webview 不支持 window.prompt（静默 noop），用内嵌模态
  tokenDraft.value = pairToken.value
  wakeTokenDraft.value = wakeToken.value
  showTokenInput.value = true
  showMenu.value = false
}

function onSaveToken() {
  const pair = tokenDraft.value.trim()
  const wk = wakeTokenDraft.value.trim()
  if (!pair) {
    localStorage.removeItem(TOKEN_LS_KEY)
    pairToken.value = ''
  } else {
    localStorage.setItem(TOKEN_LS_KEY, pair)
    pairToken.value = pair
  }
  if (!wk) {
    localStorage.removeItem(WAKE_TOKEN_LS_KEY)
    wakeToken.value = ''
  } else {
    localStorage.setItem(WAKE_TOKEN_LS_KEY, wk)
    wakeToken.value = wk
  }
  flashToast('token 已存，重启生效', 2500)
  showTokenInput.value = false
  tokenDraft.value = ''
  wakeTokenDraft.value = ''
}

/// 注册声纹入口——菜单点击。Modal 三阶段：guide → recording → done。
/// 临时把 panel 放大到 360×540 给指引文字 + 按钮足够空间，结束恢复 200×360。
async function onStartVoiceprint() {
  if (!pairToken.value) {
    flashToast('先设 fuxi-im token', 2500)
    return
  }
  if (vpStage.value !== 'idle' && vpStage.value !== 'done' && vpStage.value !== 'error') return
  showMenu.value = false
  vpResult.value = ''
  vpElapsedSec.value = 0
  vpRms.value = 0
  try {
    await getCurrentWindow().setSize(new LogicalSize(360, 540))
  } catch (e) {
    console.warn('[vp] setSize 失败', e)
  }
  vpStage.value = 'guide'
}

async function onRunVoiceprintRecord() {
  vpStage.value = 'recording'
  vpElapsedSec.value = 0
  vpRms.value = 0
  try {
    await ensureMic()
    const pcm = await captureForSeconds({
      mic: mic!,
      seconds: VP_DURATION_SEC,
      onTick: ({ elapsedSec, rms }) => {
        vpElapsedSec.value = elapsedSec
        vpRms.value = rms
      },
    })
    vpStage.value = 'uploading'
    const wav = pcmToWav(pcm)
    const enr = await uploadEnroll({
      baseURL: BASE_URL,
      token: pairToken.value,
      wavBytes: wav,
    })
    if (!enr.enrolled) throw new Error('服务端 enrolled=false')
    // sanity verify：用同段 wav 跑一次 /verify 看 score（应 ≥ 0.5 远超阈值）
    vpStage.value = 'verifying'
    const ver = await uploadVerify({
      baseURL: BASE_URL,
      token: pairToken.value,
      wavBytes: wav,
    })
    vpResult.value = `score ${ver.score.toFixed(3)} / 阈值 ${ver.threshold} · ${ver.match ? '✓ 匹配' : '⚠ 不匹配（重录）'}`
    vpStage.value = 'done'
  } catch (e) {
    vpResult.value = String(e).slice(0, 200)
    vpStage.value = 'error'
  } finally {
    disposeMicIfIdle()
  }
}

async function onCloseVoiceprint() {
  vpStage.value = 'idle'
  vpResult.value = ''
  try {
    await getCurrentWindow().setSize(new LogicalSize(PANEL_W, PANEL_H))
  } catch (e) {
    console.warn('[vp] restore setSize 失败', e)
  }
}

function onCancelToken() {
  showTokenInput.value = false
  tokenDraft.value = ''
  wakeTokenDraft.value = ''
}

/// 播一段派蒙音 + 嘴动 loop；播完按 emotion 切回对应 idle sprite mode。
/// 喷头：xuannv_voice_line 自然 say 用、唤醒后「我在，你说」也用。
/// Phase 3：emotion 透到 sovits-proxy 切 ref + onEnd 切 idle sprite mode；
/// 未传 / 未知 emotion → DEFAULT_SET（Nomal）兜底。
async function speakWithMouth(text: string, emotion?: string): Promise<void> {
  if (!pairToken.value || !text.trim()) return
  const idleSet = emotionToIdleSet(emotion)
  player?.load(SAY_SET).catch(() => {})
  try {
    await playTts({
      baseURL: BASE_URL,
      token: pairToken.value,
      text,
      emotion,
      onStep: msg => console.log('[tts]', msg, emotion ? `emo=${emotion}` : ''),
      onEnd: () => { player?.load(idleSet).catch(() => {}) }
    })
  } catch (e) {
    player?.load(idleSet).catch(() => {})
    throw e
  }
}

/// 玄女眼睛 vision_request 处理：禁眼 → 上传 user_denied 占位（让后端 oneshot
/// 立即结束而不是 10s timeout，玄女更快拿到错误说人话）；正常 → capture →
/// upload。期间 vision.markCapturing/markIdle 让左下状态点变蓝/灰。
async function handleVisionRequest(reqId: string, target: 'webcam' | 'screen'): Promise<void> {
  if (!pairToken.value) {
    console.warn('[vision] 无 pairToken，跳 vision_request')
    return
  }
  if (vision.disabled) {
    // 禁眼时直接告诉后端「user_denied」让 oneshot 立即解，比 10s timeout 友好
    try {
      await uploadVisionFrame({
        baseURL: BASE_URL,
        token: pairToken.value,
        requestId: reqId,
        error: 'user_denied',
      })
    } catch (e) {
      console.warn('[vision] user_denied 占位上传失败（不阻塞）', e)
    }
    flashToast('眼睛被蒙了（菜单可解）', 2000)
    return
  }
  vision.markCapturing()
  try {
    const blob = target === 'webcam'
      ? await captureWebcamFrame()
      : await captureScreenFrame()
    await uploadVisionFrame({
      baseURL: BASE_URL,
      token: pairToken.value,
      requestId: reqId,
      blob,
      mime: 'image/png',
    })
    flashToast(`已拍 ${target === 'webcam' ? '摄像头' : '屏幕'}`, 1500)
  } catch (e) {
    // 失败时仍要 ack 后端：上传 error 字段让 oneshot 立解，玄女说人话
    let errCode = 'capture_failed'
    if (e instanceof PermissionDeniedError) errCode = 'permission_denied'
    else if (e instanceof NoDeviceError) errCode = 'no_device'
    try {
      await uploadVisionFrame({
        baseURL: BASE_URL,
        token: pairToken.value,
        requestId: reqId,
        error: errCode,
      })
    } catch (e2) {
      console.warn('[vision] error 占位上传也失败', e2)
    }
    flashToast(`眼睛失败: ${errCode}`, 2500)
    visionError.value = true
    window.setTimeout(() => { visionError.value = false }, 2000)
  } finally {
    vision.markIdle()
  }
}

/// Phase 3 情绪 → idle sprite set：happy/surprise → HAPPY_SET；
/// worry/sad → POOR_CONDITION_SET；normal/serious/未知 → DEFAULT_SET。
/// 这套映射跟 VPet 原 vup mod 的「Default/{Nomal,Happy,PoorCondition,Ill}」对齐。
function emotionToIdleSet(emotion?: string): SpriteSet {
  switch (emotion) {
    case 'happy':
    case 'surprise':
      return HAPPY_SET
    case 'worry':
    case 'sad':
      return POOR_CONDITION_SET
    default:
      return DEFAULT_SET
  }
}

async function ensureMic(): Promise<void> {
  if (mic) return
  mic = new MicRecorder()
  await mic.start()
}

function disposeMicIfIdle(): void {
  if (mic && !asrPcmUnsub && !wakePcmUnsub) {
    mic.stop()
    mic = null
  }
}

/// 起 ASR session（手动 = autoVad:false 用户再点送出；wake 触发 = autoVad:true VAD 1.5s 静音自动断）
async function startTalking(autoVad: boolean): Promise<void> {
  if (voiceState.value !== 'idle') return
  if (!pairToken.value) {
    flashToast('先设 fuxi-im token', 2500)
    return
  }
  try {
    voiceState.value = 'recording'
    showMenu.value = false
    await ensureMic()
    asr = new AsrClient({ baseURL: BASE_URL, token: pairToken.value })
    await asr.connect()
    if (autoVad) {
      vad = new EnergyVad({
        onSilence: () => finishTalking().catch(() => {})
      })
    }
    asrPcmUnsub = mic!.subscribe(chunk => {
      asr?.sendPcm(chunk)
      vad?.feed(chunk)
    })
    flashToast(autoVad ? '🎤 听写中（说完自动断）' : '🎤 录音中（再次点击送出）', 4000)
  } catch (e) {
    voiceState.value = 'idle'
    asrPcmUnsub?.()
    asrPcmUnsub = null
    asr?.abort()
    asr = null
    vad = null
    disposeMicIfIdle()
    flashToast(`录音起失败: ${String(e).slice(0, 50)}`, 3000)
  }
}

async function finishTalking(): Promise<void> {
  if (voiceState.value !== 'recording') return
  voiceState.value = 'transcribing'
  asrPcmUnsub?.()
  asrPcmUnsub = null
  vad = null
  try {
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
    // [语音] 前缀触发玄女公理 #8——cc 必调 `fuxi xuannv say` 发 XuannvVoiceLine
    // 事件，桌宠才能拿到 say 文字 + TTS。不加前缀只走 AgentResponded（PWA 文字流）
    await sendIntervene({
      baseURL: BASE_URL,
      token: pairToken.value,
      text: `[语音] ${text}`
    })
  } catch (e) {
    flashToast(`失败：${String(e).slice(0, 60)}`, 3500)
  } finally {
    voiceState.value = 'idle'
    // talk 流程结束——若启用唤醒就把 wake mic 重新订上，让用户能再次喊「玄女」
    if (wakeEnabled.value && wake && mic && !wakePcmUnsub) {
      console.log('[wake] resubscribe mic after talk')
      wakePcmUnsub = mic.subscribe(chunk => wake!.sendPcm(chunk))
    }
    // 没开 wake 才考虑关 mic（开 wake 时 mic 永驻）
    if (!wakeEnabled.value) disposeMicIfIdle()
  }
}

async function onTalkToggle(): Promise<void> {
  if (voiceState.value === 'idle') {
    await startTalking(false)
  } else if (voiceState.value === 'recording') {
    await finishTalking()
  }
}

async function enableWake(): Promise<void> {
  console.log('[wake] enableWake called, token len:', wakeToken.value.length)
  if (!wakeToken.value) {
    flashToast('先设 wake.token', 2500)
    return
  }
  wakeEnabled.value = true
  localStorage.setItem(WAKE_EN_LS_KEY, '1')
  try {
    await ensureMic()
    console.log('[wake] mic ready, creating WakeClient')
    wake = new WakeClient({
      baseURL: BASE_URL,
      token: wakeToken.value,
      onWake: async (kw) => {
        if (voiceState.value !== 'idle') return
        console.log('[wake] onWake fired, pause wake sub + play 我在你说')
        flashToast(`听见「${kw}」`, 1500)
        // 1. 暂停 wake mic 上推——TTS 自己的声从喇叭播会被 mic 拾到，避免 wake
        //    在自己说话时又触发 wake 自激
        wakePcmUnsub?.()
        wakePcmUnsub = null
        // 2. 占位 sending 防 wake 重入
        voiceState.value = 'sending'
        try {
          await speakWithMouth('我在，你说。')
        } catch (e) {
          console.warn('[wake response tts]', e)
        }
        // 3. TTS ended 后 500ms 缓冲——让扬声器残响沉、麦克风缓冲清。
        //    不加这条 mac echoCancellation 偶尔拦不住「我在你说」尾音进 ASR
        await new Promise(r => setTimeout(r, 500))
        voiceState.value = 'idle'
        // 4. 起 ASR + VAD 听用户说话
        startTalking(true).catch(() => {})
        // wake 重订 mic 在 finishTalking 里 finally 段做（先确保 talk 流程完整）
      },
      onStatus: s => { wakeStatus.value = s }
    })
    wake.start()
    let pcmCount = 0
    wakePcmUnsub = mic!.subscribe(chunk => {
      wake!.sendPcm(chunk)
      pcmCount++
      // 头 5 个 chunk + 每 50 个（~4s）打一次，确认数据流活
      if (pcmCount <= 5 || pcmCount % 50 === 0) {
        // 算 RMS 大致看音量
        const i16 = new Int16Array(chunk)
        let sumSq = 0
        for (let i = 0; i < i16.length; i++) sumSq += i16[i] * i16[i]
        const rms = Math.round(Math.sqrt(sumSq / i16.length))
        console.log(`[wake] pcm #${pcmCount} bytes=${chunk.byteLength} rms=${rms}`)
      }
    })
  } catch (e) {
    flashToast(`唤醒启动失败: ${String(e).slice(0, 50)}`, 3000)
    disableWake()
  }
}

function disableWake(): void {
  wakeEnabled.value = false
  localStorage.setItem(WAKE_EN_LS_KEY, '0')
  wakePcmUnsub?.()
  wakePcmUnsub = null
  wake?.stop()
  wake = null
  wakeStatus.value = 'off'
  disposeMicIfIdle()
}

function onWakeToggle(): void {
  if (wakeEnabled.value) disableWake()
  else enableWake()
}
</script>

<template>
  <div class="pet-canvas" ref="canvasContainer"
       @pointerdown="onPointerDown"
       @dblclick.prevent="onTouchHead"
       @contextmenu.prevent>
    <!-- 数值菜单：默认隐藏，右键 toggle；mousedown.stop 防止落到外层触发拖动 -->
    <div v-if="showMenu" class="context-menu" @mousedown.stop>
      <div class="row clickable talk" @click="onTalkToggle">
        {{ voiceState === 'recording' ? '🎤 录音中…点击送出' :
           voiceState === 'transcribing' ? '⏳ 转写中…' :
           voiceState === 'sending' ? '⏳ 发送中…' :
           '🎤 跟玄女说一句' }}
      </div>
      <div class="row clickable wake" @click="onWakeToggle">
        {{ wakeEnabled
            ? (wakeStatus === 'ready' ? '🟢 唤醒中（喊「玄女」）'
              : wakeStatus === 'connecting' ? '🟡 唤醒：连接中…'
              : '🔴 唤醒：断开（点击重连）')
            : '⚪ 启用唤醒「玄女」' }}
      </div>
      <div class="row clickable" @click="onSetToken">
        🔑 {{ pairToken ? '换 token' : '设 token' }}
      </div>
      <div class="row clickable" @click="onStartVoiceprint">
        🆔 注册声纹
      </div>
      <div class="sep"></div>
      <!-- 玄女眼睛 · 隐私 sub-section（不用 emoji，用 unicode ◉ / ✕ 跟整体审美对齐） -->
      <div class="row eye-title">◉ 眼睛</div>
      <div class="row clickable eye"
           :class="{ on: !vision.disabled }"
           @click="onEyeAllow">
        {{ !vision.disabled ? '◉ 允许（默认）' : '○ 切到 允许' }}
      </div>
      <div class="row clickable eye"
           @click="onEyeDisable15">
        ◐ 禁眼 15 分钟{{ vision.remainingSec !== null ? ` （剩 ${Math.ceil(vision.remainingSec / 60)} 分）` : '' }}
      </div>
      <div class="row clickable eye"
           :class="{ on: vision.disabledUntil === 'forever' }"
           @click="onEyeDisableForever">
        {{ vision.disabledUntil === 'forever' ? '● 永久禁眼（已开）' : '○ 永久禁眼' }}
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
      <div class="token-title">fuxi-im HMAC token（intervene/asr/tts 共用）</div>
      <textarea
        class="token-input"
        v-model="tokenDraft"
        rows="3"
        placeholder="eyJ... (home: python3 ~/.fuxi/im-mint-token.py)"
        spellcheck="false"
      />
      <div class="token-title">wake.token（home: ~/.fuxi/wake.token 64 hex）</div>
      <textarea
        class="token-input"
        v-model="wakeTokenDraft"
        rows="2"
        placeholder="74f5990b... (启用唤醒词必填，否则可空)"
        spellcheck="false"
      />
      <div class="token-actions">
        <button class="btn cancel" @click="onCancelToken">取消</button>
        <button class="btn save" @click="onSaveToken">保存</button>
      </div>
    </div>
    <!-- 声纹注册 modal：覆盖全屏（panel 已临时放大到 360×540） -->
    <div v-if="vpStage !== 'idle'" class="vp-modal" @mousedown.stop>
      <!-- 阶段 1：指引 -->
      <template v-if="vpStage === 'guide'">
        <div class="vp-title">注册声纹</div>
        <div class="vp-desc">
          点开始后，请自然朗读下方文字 <b>{{ VP_DURATION_SEC }} 秒</b>。
          注册后，陌生人喊「玄女」桌宠将不再响应——只听你说话。
        </div>
        <div class="vp-sample">{{ VP_SAMPLE_TEXT }}</div>
        <div class="vp-tips">
          建议：安静环境、距离麦克 20-30cm、用日常语气念（不要刻意压低 / 唱腔）。
        </div>
        <div class="vp-actions">
          <button class="btn cancel" @click="onCloseVoiceprint">取消</button>
          <button class="btn save" @click="onRunVoiceprintRecord">开始录音</button>
        </div>
      </template>

      <!-- 阶段 2：录音中 -->
      <template v-else-if="vpStage === 'recording'">
        <div class="vp-title">🎤 录音中…</div>
        <div class="vp-sample tight">{{ VP_SAMPLE_TEXT }}</div>
        <div class="vp-progress-wrap">
          <div class="vp-progress" :style="{ width: (vpElapsedSec / VP_DURATION_SEC * 100) + '%' }"></div>
        </div>
        <div class="vp-meter-row">
          <span>{{ vpElapsedSec.toFixed(1) }}s / {{ VP_DURATION_SEC }}s</span>
          <span class="vp-rms" :style="{ opacity: Math.min(1, vpRms * 8 + 0.2) }">
            音量 {{ (vpRms * 100).toFixed(0) }}
          </span>
        </div>
        <div class="vp-tips">说话中……请保持自然语气。</div>
      </template>

      <!-- 阶段 3：上传 / 验证 -->
      <template v-else-if="vpStage === 'uploading' || vpStage === 'verifying'">
        <div class="vp-title">{{ vpStage === 'uploading' ? '上传中…' : '验证中…' }}</div>
        <div class="vp-desc">正在把声纹送到 home GPU，提取 192 维 embedding。</div>
        <div class="vp-spinner"></div>
      </template>

      <!-- 阶段 4：完成 -->
      <template v-else-if="vpStage === 'done'">
        <div class="vp-title">✓ 注册成功</div>
        <div class="vp-desc">{{ vpResult }}</div>
        <div class="vp-tips">
          下次有陌生人喊「玄女」桌宠完全静默；你自己喊正常响应。
          想换声纹（感冒 / 重感冒后）随时重跑「注册声纹」即可，覆盖式更新。
        </div>
        <div class="vp-actions">
          <button class="btn save" @click="onCloseVoiceprint">完成</button>
        </div>
      </template>

      <!-- 阶段 5：失败 -->
      <template v-else-if="vpStage === 'error'">
        <div class="vp-title">⚠ 注册失败</div>
        <div class="vp-desc vp-err">{{ vpResult }}</div>
        <div class="vp-tips">检查：token 有效？home 上 sv.service / asr.service 起着？麦克权限给了？</div>
        <div class="vp-actions">
          <button class="btn cancel" @click="onCloseVoiceprint">关闭</button>
          <button class="btn save" @click="vpStage = 'guide'">重试</button>
        </div>
      </template>
    </div>

    <!-- 玄女回话气泡：xuannv_voice_line 事件触发，自动 fade 6s -->
    <div v-if="bubble" class="bubble">{{ bubble }}</div>
    <!-- 玄女眼睛状态点：左下角，灰=idle 蓝=capturing(脉冲) 红=禁眼 / 错误 -->
    <div class="vision-dot"
         :class="{
           capturing: vision.capturing,
           disabled: vision.disabled,
           error: visionError
         }"
         :title="vision.disabled
           ? (vision.disabledUntil === 'forever' ? '眼睛永久禁' : `禁眼 (剩 ${vision.remainingSec ?? 0}s)`)
           : (vision.capturing ? '眼睛采帧中…' : '眼睛 idle')"></div>
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
.row.clickable.wake {
  color: rgb(80, 130, 80);
}
.row.clickable.wake:hover {
  background: rgba(80, 130, 80, 0.08);
}
.row.clickable.quit {
  color: rgb(180, 60, 60);
}
.row.clickable.quit:hover {
  background: rgba(180, 60, 60, 0.08);
}
.row.eye-title {
  font-weight: 600;
  color: rgba(40, 40, 40, 0.75);
  margin-top: 2px;
}
.row.clickable.eye {
  color: rgba(40, 40, 40, 0.85);
  font-variant-numeric: tabular-nums;
}
.row.clickable.eye.on {
  color: rgb(40, 100, 180);
}
.row.clickable.eye:hover {
  background: rgba(40, 100, 180, 0.06);
}
.vision-dot {
  position: absolute;
  bottom: 6px;
  left: 6px;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #999;
  box-shadow: 0 0 2px rgba(0, 0, 0, 0.35);
  pointer-events: none;
  transition: background 0.25s ease;
}
.vision-dot.capturing {
  background: #3b82f6;
  animation: vision-pulse 0.6s ease-in-out infinite;
}
.vision-dot.disabled {
  background: #ef4444;
}
.vision-dot.error {
  background: #ef4444;
  box-shadow: 0 0 6px rgba(239, 68, 68, 0.7);
}
@keyframes vision-pulse {
  0%, 100% { transform: scale(1); opacity: 1; }
  50%      { transform: scale(1.4); opacity: 0.65; }
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

/* 声纹注册 modal —— panel 临时放大到 360×540 时占满 */
.vp-modal {
  position: fixed;
  top: 0; left: 0; right: 0; bottom: 0;
  background: rgba(255, 255, 255, 0.97);
  backdrop-filter: blur(8px);
  font-family: -apple-system, sans-serif;
  padding: 16px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  overflow-y: auto;
  z-index: 100;
}
.vp-title {
  font-size: 16px;
  font-weight: 600;
  color: rgba(20, 20, 20, 0.95);
}
.vp-desc {
  font-size: 12px;
  line-height: 1.55;
  color: rgba(40, 40, 40, 0.85);
}
.vp-sample {
  font-size: 13px;
  line-height: 1.7;
  color: rgba(30, 30, 30, 0.92);
  background: rgba(245, 245, 245, 0.9);
  border: 1px solid rgba(0, 0, 0, 0.08);
  border-radius: 6px;
  padding: 10px 12px;
  white-space: pre-line;
}
.vp-sample.tight {
  font-size: 11px;
  line-height: 1.5;
  max-height: 140px;
  overflow-y: auto;
}
.vp-tips {
  font-size: 11px;
  color: rgba(80, 80, 80, 0.75);
  line-height: 1.45;
}
.vp-err {
  color: rgba(180, 40, 40, 0.9);
  font-family: ui-monospace, monospace;
  word-break: break-all;
}
.vp-actions {
  margin-top: auto;
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}
.vp-progress-wrap {
  height: 8px;
  background: rgba(0, 0, 0, 0.06);
  border-radius: 4px;
  overflow: hidden;
}
.vp-progress {
  height: 100%;
  background: rgb(40, 140, 80);
  transition: width 100ms linear;
}
.vp-meter-row {
  display: flex;
  justify-content: space-between;
  font-size: 11px;
  color: rgba(40, 40, 40, 0.8);
  font-variant-numeric: tabular-nums;
}
.vp-rms {
  font-family: ui-monospace, monospace;
}
.vp-spinner {
  width: 24px;
  height: 24px;
  border: 3px solid rgba(0, 0, 0, 0.1);
  border-top-color: rgb(40, 100, 180);
  border-radius: 50%;
  animation: vp-spin 0.8s linear infinite;
  margin: 12px auto;
}
@keyframes vp-spin {
  to { transform: rotate(360deg); }
}
</style>

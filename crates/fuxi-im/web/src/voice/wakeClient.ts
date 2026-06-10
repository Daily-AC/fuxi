/// home /wake/api/wake —— 讯飞唤醒词检测 WS 客户端。
///
/// 协议见 apps/jarvis/WAKE_PROTOCOL.md：
///   client → hello + 16kHz mono int16 PCM binary 帧
///   server → ready / wake / ping / error / bye
///
/// PWA 语音模式：常驻 WS + 麦克风开着采样上推。server 检到「玄女」下行
/// wake 事件，前端收到后触发 voice flow（开 ASR 走 STT → intervene → 玄女
/// reply + TTS）。移植自 jarvis-pet（apps/jarvis-pet/src/voice/wakeClient.ts）。
///
/// 鉴权：URL 走 `?token=...`（浏览器 WebSocket 不能 set header；wake server
/// 已接受 query token，commit `feat(wake): query token 兼容`）。
///
/// 重连：1s/2s/4s/8s/16s/30s cap 退避。

export interface WakeClientOpts {
  /// 不带 path 的 base，如 https://im.qmledmq.cn:8443
  baseURL: string
  /// 预共享 wake token（home `~/.fuxi/wake.token` 那 64 字节 hex）
  token: string
  /// 唤醒事件回调
  onWake: (keyword: string, score: number) => void
  /// 状态回调（连接 / 断开）
  onStatus?: (status: 'connecting' | 'ready' | 'disconnected') => void
}

export class WakeClient {
  private ws: WebSocket | null = null
  private reconnectMs = 1000
  private stopped = false
  private pingTimer: number | null = null

  constructor(private opts: WakeClientOpts) {}

  start(): void {
    this.stopped = false
    this.openWs()
  }

  stop(): void {
    this.stopped = true
    if (this.pingTimer != null) {
      clearInterval(this.pingTimer)
      this.pingTimer = null
    }
    this.ws?.close(1000)
    this.ws = null
  }

  /// 上推 PCM 二进制帧（16kHz mono int16 LE）
  sendPcm(chunk: ArrayBuffer): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(chunk)
    }
  }

  private openWs(): void {
    this.opts.onStatus?.('connecting')
    const u = new URL(this.opts.baseURL)
    u.protocol = u.protocol === 'https:' ? 'wss:' : 'ws:'
    u.pathname = '/wake/api/wake'
    u.searchParams.set('token', this.opts.token)
    const ws = new WebSocket(u.toString())
    this.ws = ws

    ws.addEventListener('open', () => {
      ws.send(JSON.stringify({ type: 'hello', client: 'fuxi-pwa', version: '1.0.0' }))
    })

    ws.addEventListener('message', e => {
      if (typeof e.data !== 'string') return
      try {
        const m = JSON.parse(e.data)
        switch (m.type) {
          case 'ready':
            this.reconnectMs = 1000
            this.opts.onStatus?.('ready')
            break
          case 'wake':
            this.opts.onWake(m.keyword || '玄女', typeof m.score === 'number' ? m.score : 0)
            break
          case 'ping':
            ws.send(JSON.stringify({ type: 'pong', at: new Date().toISOString() }))
            break
          case 'error':
            console.warn('[wake] server error', m.code, m.message)
            break
          default:
            break
        }
      } catch {
        /* ignore non-json */
      }
    })

    ws.addEventListener('close', e => {
      console.warn('[wake] ws closed', e.code, e.reason)
      this.opts.onStatus?.('disconnected')
      this.ws = null
      if (this.stopped) return
      if (e.code === 4401 || e.code === 1008) {
        console.warn('[wake] auth rejected — stop reconnect, wait token update')
        return
      }
      setTimeout(() => this.openWs(), this.reconnectMs)
      this.reconnectMs = Math.min(this.reconnectMs * 2, 30000)
    })

    ws.addEventListener('error', err => {
      console.warn('[wake] ws error', err)
    })
  }
}

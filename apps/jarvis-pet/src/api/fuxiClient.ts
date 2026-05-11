import type { WireEvent } from '@/types/event'

/// fuxi-im /api/conv WebSocket 客户端 + 简单 REST 包装。
/// reconnect 策略：disconnect 后 1s/2s/4s/8s ... cap 30s 退避重连。

export interface FuxiClientOpts {
  baseURL: string         // e.g. https://im.qmledmq.cn:8443
  pairToken?: string      // Authorization Bearer，可空（开发可不带）
  onEvent: (ev: WireEvent) => void
  onStatus?: (status: 'connecting' | 'connected' | 'disconnected') => void
}

export class FuxiClient {
  private ws: WebSocket | null = null
  private reconnectMs = 1000
  private stopped = false

  constructor(private opts: FuxiClientOpts) {}

  connect(): void {
    this.stopped = false
    this.openWs()
  }

  stop(): void {
    this.stopped = true
    this.ws?.close()
    this.ws = null
  }

  private openWs(): void {
    this.opts.onStatus?.('connecting')
    const url = this.wsUrl()
    const ws = new WebSocket(url)
    this.ws = ws

    ws.addEventListener('open', () => {
      this.reconnectMs = 1000
      this.opts.onStatus?.('connected')
    })

    ws.addEventListener('message', e => {
      try {
        const ev = JSON.parse(e.data as string) as WireEvent
        this.opts.onEvent(ev)
      } catch (err) {
        console.error('[fuxiClient] message parse failed', err, e.data)
      }
    })

    ws.addEventListener('close', () => {
      this.opts.onStatus?.('disconnected')
      if (!this.stopped) {
        setTimeout(() => this.openWs(), this.reconnectMs)
        this.reconnectMs = Math.min(this.reconnectMs * 2, 30000)
      }
    })

    ws.addEventListener('error', err => {
      console.warn('[fuxiClient] ws error', err)
    })
  }

  private wsUrl(): string {
    const u = new URL(this.opts.baseURL)
    u.protocol = u.protocol === 'https:' ? 'wss:' : 'ws:'
    u.pathname = '/api/conv'
    if (this.opts.pairToken) {
      u.searchParams.set('token', this.opts.pairToken)
    }
    return u.toString()
  }
}

/// home /api/asr WebSocket 客户端。
///
/// 协议见 deploy/asr/README.md：
///   client → server  start JSON / PCM binary chunks / end JSON
///   server → client  ready / final / error JSON
///
/// 用法：
///   const asr = new AsrClient({baseURL, token})
///   await asr.connect()                                   // 完成 handshake
///   asr.sendPcm(chunk)                                    // 多次
///   const { text } = await asr.finish()                   // 返 final
///
/// 单次会话用一个实例；连续 N 句话 new N 次实例（WS 协议每会话一条）。

export interface AsrClientOpts {
  /// 不带 path 的 base，如 https://im.qmledmq.cn:8443
  baseURL: string
  token: string
}

export interface AsrFinal {
  text: string
  durationMs: number
  elapsedMs: number
}

type ServerMessage =
  | { type: 'ready' }
  | { type: 'final'; text: string; duration_ms: number; elapsed_ms: number }
  | { type: 'error'; error: string }

export class AsrClient {
  private ws: WebSocket | null = null
  private finalPromise: Promise<AsrFinal> | null = null
  private resolveFinal?: (f: AsrFinal) => void
  private rejectFinal?: (e: Error) => void

  constructor(private opts: AsrClientOpts) {}

  async connect(): Promise<void> {
    const u = new URL(this.opts.baseURL)
    u.protocol = u.protocol === 'https:' ? 'wss:' : 'ws:'
    u.pathname = '/api/asr'
    const ws = new WebSocket(u.toString())
    this.ws = ws

    await new Promise<void>((resolve, reject) => {
      const onOpen = () => {
        ws.removeEventListener('error', onError)
        ws.send(JSON.stringify({ type: 'start', token: this.opts.token, sample_rate: 16000 }))
      }
      const onMsg = (e: MessageEvent) => {
        try {
          const m = JSON.parse(e.data as string) as ServerMessage
          if (m.type === 'ready') {
            ws.removeEventListener('open', onOpen)
            ws.removeEventListener('message', onMsg)
            ws.addEventListener('message', this.handleMessage)
            resolve()
          } else if (m.type === 'error') {
            reject(new Error(`asr server: ${m.error}`))
          }
        } catch {
          /* ignore non-json */
        }
      }
      const onError = () => {
        reject(new Error('asr ws error before ready'))
      }
      ws.addEventListener('open', onOpen)
      ws.addEventListener('message', onMsg)
      ws.addEventListener('error', onError, { once: true })
      ws.addEventListener('close', e => {
        if (e.code === 4401) reject(new Error('asr unauthorized — token 不对'))
        else if (e.code !== 1000) reject(new Error(`asr ws closed ${e.code} ${e.reason}`))
      })
    })
  }

  private handleMessage = (e: MessageEvent) => {
    try {
      const m = JSON.parse(e.data as string) as ServerMessage
      if (m.type === 'final') {
        this.resolveFinal?.({
          text: m.text,
          durationMs: m.duration_ms,
          elapsedMs: m.elapsed_ms
        })
      } else if (m.type === 'error') {
        this.rejectFinal?.(new Error(m.error))
      }
    } catch {
      /* ignore non-json or partial / unknown messages */
    }
  }

  sendPcm(chunk: ArrayBuffer): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return
    this.ws.send(chunk)
  }

  /// 发 end 帧并等 final——返回识别文本。
  finish(): Promise<AsrFinal> {
    if (!this.ws) return Promise.reject(new Error('not connected'))
    if (this.finalPromise) return this.finalPromise
    this.finalPromise = new Promise((res, rej) => {
      this.resolveFinal = res
      this.rejectFinal = rej
    })
    this.ws.send(JSON.stringify({ type: 'end' }))
    return this.finalPromise
  }

  abort(): void {
    if (this.ws && this.ws.readyState === WebSocket.OPEN) {
      try {
        this.ws.send(JSON.stringify({ type: 'abort' }))
      } catch {
        /* ignore */
      }
    }
    this.ws?.close(1000)
    this.ws = null
  }
}

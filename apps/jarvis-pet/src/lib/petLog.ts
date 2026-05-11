/// 桌宠 console.log → Rust IPC pet_log → 磁盘 /tmp/jarvis-pet.log
/// 用法：调用 petLog('xxx') 替代 console.log。或者 setup() 一次性 monkey-patch
/// console.log 让全部 log 自动 forward。

import { invoke } from '@tauri-apps/api/core'

export function petLog(msg: string): void {
  invoke('pet_log', { msg }).catch(() => { /* webview shutting down，忽略 */ })
  // 同时本地 console（dev mode 看 Safari inspector 仍有用）
  // eslint-disable-next-line no-console
  console.log(msg)
}

/// 启动时调一次：把 console.log/warn/error 都桥接到 pet_log 文件。
/// 不破坏原 console（仍输出 webview 控制台），只是 mirror 到磁盘。
export function bridgeConsoleToFile(): void {
  const origLog = console.log.bind(console)
  const origWarn = console.warn.bind(console)
  const origError = console.error.bind(console)
  const fmt = (level: string, args: unknown[]) => {
    const parts = args.map(a => {
      if (typeof a === 'string') return a
      try { return JSON.stringify(a) } catch { return String(a) }
    }).join(' ')
    return `[${level}] ${parts}`
  }
  console.log = (...args: unknown[]) => {
    origLog(...args)
    invoke('pet_log', { msg: fmt('log', args) }).catch(() => {})
  }
  console.warn = (...args: unknown[]) => {
    origWarn(...args)
    invoke('pet_log', { msg: fmt('warn', args) }).catch(() => {})
  }
  console.error = (...args: unknown[]) => {
    origError(...args)
    invoke('pet_log', { msg: fmt('error', args) }).catch(() => {})
  }
}

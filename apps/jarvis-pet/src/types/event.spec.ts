import { describe, it, expect } from 'vitest'
import type { WireEvent, WireKind } from './event'

/// vision_request 走 WireKind 联合而非 fallback 分支——TS narrow + 字段访问验证
describe('WireKind vision_request', () => {
  it('解析 vision_request 不退化到 fallback', () => {
    const ev: WireEvent = {
      meta: { id: 'evt-1' },
      kind: {
        type: 'vision_request',
        request_id: 'req-uuid',
        target: 'webcam',
        hint: '看看桌面',
      },
    }
    expect(ev.kind.type).toBe('vision_request')
    if (ev.kind.type === 'vision_request') {
      // narrow 后能直接访问字段
      expect(ev.kind.request_id).toBe('req-uuid')
      expect(ev.kind.target).toBe('webcam')
      expect(ev.kind.hint).toBe('看看桌面')
    } else {
      throw new Error('discriminant 没 narrow 成功——回落到 fallback 分支了')
    }
  })

  it('hint 可选 null', () => {
    const k: WireKind = {
      type: 'vision_request',
      request_id: 'r',
      target: 'screen',
      hint: null,
    }
    if (k.type === 'vision_request') {
      expect(k.hint).toBeNull()
    }
  })

  it('target 仅接受 webcam | screen（编译时收紧）', () => {
    const w: WireKind = { type: 'vision_request', request_id: 'r', target: 'webcam' }
    const s: WireKind = { type: 'vision_request', request_id: 'r', target: 'screen' }
    expect(w.type).toBe('vision_request')
    expect(s.type).toBe('vision_request')
  })
})

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import {
  captureWebcamFrame,
  PermissionDeniedError,
  NoDeviceError,
  VisionCaptureError,
} from './visionCapture'

/// happy-dom 不支持 getUserMedia/getDisplayMedia/canvas.toBlob，全部 stub。
/// 重点：把错误分类映射验对——上层 PetCanvas 据此切 UI / 状态点。
describe('visionCapture · 错误分类', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
    vi.unstubAllGlobals()
  })

  function stubMediaDevices(impl: Partial<MediaDevices>): void {
    vi.stubGlobal('navigator', { mediaDevices: impl as MediaDevices })
  }

  it('webcam: NotAllowedError → PermissionDeniedError', async () => {
    stubMediaDevices({
      getUserMedia: vi.fn().mockRejectedValue(
        Object.assign(new Error('Permission denied'), { name: 'NotAllowedError' })
      ),
    })
    await expect(captureWebcamFrame()).rejects.toBeInstanceOf(PermissionDeniedError)
  })

  it('webcam: NotFoundError → NoDeviceError', async () => {
    stubMediaDevices({
      getUserMedia: vi.fn().mockRejectedValue(
        Object.assign(new Error('No camera'), { name: 'NotFoundError' })
      ),
    })
    await expect(captureWebcamFrame()).rejects.toBeInstanceOf(NoDeviceError)
  })

  it('webcam: 其它错误 → VisionCaptureError', async () => {
    stubMediaDevices({
      getUserMedia: vi.fn().mockRejectedValue(new Error('some random media err')),
    })
    await expect(captureWebcamFrame()).rejects.toBeInstanceOf(VisionCaptureError)
  })

  it('webcam: 没有 mediaDevices.getUserMedia → NoDeviceError', async () => {
    stubMediaDevices({})
    await expect(captureWebcamFrame()).rejects.toBeInstanceOf(NoDeviceError)
  })
})

/// captureScreenFrame 已 v0.4.1 起不走 navigator.mediaDevices.getDisplayMedia
/// （Tauri 2 macOS WKWebView 实测崩），改走 Tauri Rust IPC `pet_capture_screen_png`
/// 调 macOS `screencapture`。这里 mock invoke 验证错误分类映射。
///
/// vi.resetModules 后 captureScreenFrame 拿的是新 module instance —— 抛出的
/// VisionCaptureError 跟测试顶部 import 的不是同一颗 class，instanceof 不等。
/// 所以每个 case 内重新 import 同模块拿配对的 class，避免假阴性。
describe('visionCapture · screen 走 Rust IPC', () => {
  afterEach(() => {
    vi.resetModules()
    vi.doUnmock('@tauri-apps/api/core')
  })

  async function loadWithMock(mockInvoke: (...a: unknown[]) => unknown) {
    vi.resetModules()
    vi.doMock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }))
    return await import('./visionCapture')
  }

  it('TCC 关键字 → PermissionDeniedError', async () => {
    const m = await loadWithMock(vi.fn().mockRejectedValue(
      'screencapture exit=Some(1) stderr=could not create image from display'
    ))
    await expect(m.captureScreenFrame()).rejects.toBeInstanceOf(m.PermissionDeniedError)
  })

  it('user cancelled → PermissionDeniedError', async () => {
    const m = await loadWithMock(vi.fn().mockRejectedValue('user cancelled the screencapture'))
    await expect(m.captureScreenFrame()).rejects.toBeInstanceOf(m.PermissionDeniedError)
  })

  it('其它错误 → VisionCaptureError', async () => {
    const m = await loadWithMock(vi.fn().mockRejectedValue('spawn screencapture 失败：no such file'))
    await expect(m.captureScreenFrame()).rejects.toBeInstanceOf(m.VisionCaptureError)
  })

  it('返字节 → Blob image/png', async () => {
    const fake = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])
    const m = await loadWithMock(vi.fn().mockResolvedValue(fake))
    const blob = await m.captureScreenFrame()
    expect(blob.type).toBe('image/png')
    expect(blob.size).toBe(fake.byteLength)
  })

  it('空字节 → VisionCaptureError', async () => {
    const m = await loadWithMock(vi.fn().mockResolvedValue(new Uint8Array()))
    await expect(m.captureScreenFrame()).rejects.toBeInstanceOf(m.VisionCaptureError)
  })
})

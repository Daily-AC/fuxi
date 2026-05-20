import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import {
  captureWebcamFrame,
  captureScreenFrame,
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

  it('screen: NotAllowedError → PermissionDeniedError', async () => {
    stubMediaDevices({
      getDisplayMedia: vi.fn().mockRejectedValue(
        Object.assign(new Error('user cancelled'), { name: 'NotAllowedError' })
      ),
    })
    await expect(captureScreenFrame()).rejects.toBeInstanceOf(PermissionDeniedError)
  })

  it('webcam: 没有 mediaDevices.getUserMedia → NoDeviceError', async () => {
    stubMediaDevices({})
    await expect(captureWebcamFrame()).rejects.toBeInstanceOf(NoDeviceError)
  })

  it('screen: 没有 mediaDevices.getDisplayMedia → NoDeviceError', async () => {
    stubMediaDevices({})
    await expect(captureScreenFrame()).rejects.toBeInstanceOf(NoDeviceError)
  })
})

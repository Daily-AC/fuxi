/// 桌宠端「玄女眼睛」抓帧实现 —— webcam / screen 各取一帧 PNG Blob。
///
/// 为什么不丢 raw video 流？v1 单帧足够 cc 多模态 Read 用，玄女按 hint 决定
/// 看什么；连续看 / 视频流是 v1.x。
///
/// 错误分类（PermissionDeniedError / NoDeviceError / VisionCaptureError）让
/// PetCanvas 决定 UI 反馈和错误上报字段（spec 错误兜底矩阵）。

export class VisionCaptureError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'VisionCaptureError'
  }
}

export class PermissionDeniedError extends VisionCaptureError {
  constructor(message = '用户拒绝授权或系统级权限未开') {
    super(message)
    this.name = 'PermissionDeniedError'
  }
}

export class NoDeviceError extends VisionCaptureError {
  constructor(message = '没有可用的摄像 / 屏幕设备') {
    super(message)
    this.name = 'NoDeviceError'
  }
}

function classifyMediaError(err: unknown): VisionCaptureError {
  // DOMException.name 是浏览器约定的稳定字段；其他实现也按这个 name pattern
  const name = (err as { name?: string })?.name ?? ''
  const msg = (err as Error)?.message ?? String(err)
  if (name === 'NotAllowedError' || name === 'SecurityError') {
    return new PermissionDeniedError(msg)
  }
  if (name === 'NotFoundError' || name === 'OverconstrainedError') {
    return new NoDeviceError(msg)
  }
  return new VisionCaptureError(msg)
}

async function streamToFrameBlob(stream: MediaStream): Promise<Blob> {
  // 单帧采集流程：video element 起播 → 第一帧 loadeddata → drawImage → toBlob → stop tracks
  // 关键：用完立即 stop tracks，否则 macOS 摄像头绿灯常亮 / 屏幕录制指示长亮
  const video = document.createElement('video')
  video.srcObject = stream
  video.muted = true
  // autoplay 必须在 user gesture context 下；vision_request 由用户主动调用所以 OK
  ;(video as HTMLVideoElement & { playsInline?: boolean }).playsInline = true
  try {
    await video.play()
    // 等第一帧解码完成；loadeddata 比 canplay 严，确保 drawImage 不画黑帧
    if (video.readyState < 2) {
      await new Promise<void>((res, rej) => {
        const onErr = () => rej(new VisionCaptureError('video load 失败'))
        video.addEventListener('loadeddata', () => res(), { once: true })
        video.addEventListener('error', onErr, { once: true })
      })
    }
    const w = video.videoWidth || 1280
    const h = video.videoHeight || 720
    const canvas = document.createElement('canvas')
    canvas.width = w
    canvas.height = h
    const ctx = canvas.getContext('2d')
    if (!ctx) throw new VisionCaptureError('canvas 2d 上下文不可用')
    ctx.drawImage(video, 0, 0, w, h)
    const blob: Blob | null = await new Promise(res => canvas.toBlob(res, 'image/png'))
    if (!blob) throw new VisionCaptureError('canvas.toBlob 返回 null')
    return blob
  } finally {
    // 必须 stop——否则 mac 摄像头绿灯 / 屏幕录制指示常亮，用户会怕
    for (const t of stream.getTracks()) t.stop()
    video.srcObject = null
  }
}

export async function captureWebcamFrame(): Promise<Blob> {
  const md = navigator?.mediaDevices
  if (!md?.getUserMedia) {
    throw new NoDeviceError('navigator.mediaDevices.getUserMedia 不可用')
  }
  let stream: MediaStream
  try {
    stream = await md.getUserMedia({ video: true, audio: false })
  } catch (err) {
    throw classifyMediaError(err)
  }
  return streamToFrameBlob(stream)
}

export async function captureScreenFrame(): Promise<Blob> {
  // 不走 navigator.mediaDevices.getDisplayMedia——Tauri 2 macOS WKWebView
  // 实测对它支持坏（release 包里抛 generic 错且不弹系统权限弹窗）。改走
  // Tauri Rust 侧 IPC 命令 `pet_capture_screen_png` 调 macOS 内置
  // `screencapture`，TCC 弹窗归属 XuannvPet bundle 走系统正常路径。
  const { invoke } = await import('@tauri-apps/api/core')
  let bytes: Uint8Array
  try {
    const result = await invoke<number[] | Uint8Array>('pet_capture_screen_png')
    bytes = result instanceof Uint8Array ? result : new Uint8Array(result)
  } catch (err) {
    const msg = String(err)
    // screencapture 拒绝有几种 stderr 关键字：
    //   - "could not create image from display" / "TCC" → 权限未授
    //   - "user cancelled" → 用户在弹窗里点了不允许
    if (/TCC|create image|user cancelled|denied/i.test(msg)) {
      throw new PermissionDeniedError(`screencapture 拒：${msg}`)
    }
    throw new VisionCaptureError(`screencapture 失败：${msg}`)
  }
  if (!bytes || bytes.length === 0) {
    throw new VisionCaptureError('screencapture 返回空字节')
  }
  // Blob 构造在新版 TS lib.dom 收紧，Uint8Array<ArrayBufferLike> 不 assignable，
  // 显式取 .buffer（ArrayBufferLike）作 BlobPart 即可。mime 强制 image/png（screencapture -t png）
  return new Blob([bytes.buffer as ArrayBuffer], { type: 'image/png' })
}

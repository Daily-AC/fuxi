/// VPet sprite 帧文件名约定：<base>_<frameIndex_3digit>_<durationMs>.png
/// 例：default_001_120.png = 第 1 帧，120ms 显示
///     a_start_005_80.png = 第 5 帧，80ms 显示
/// 后缀 .png/.PNG 都接受。

export interface FrameMeta {
  frameIndex: number
  durationMs: number
}

export function parseFrameFilename(name: string): FrameMeta | null {
  // 去 .png 后缀（大小写不敏感）
  const lower = name.toLowerCase()
  if (!lower.endsWith('.png')) return null
  const base = name.slice(0, -4)

  // 取最后两个 _ 段
  const parts = base.split('_')
  if (parts.length < 3) return null

  const durationStr = parts[parts.length - 1]
  const indexStr = parts[parts.length - 2]
  const durationMs = parseInt(durationStr, 10)
  const frameIndex = parseInt(indexStr, 10)
  if (Number.isNaN(durationMs) || Number.isNaN(frameIndex)) return null
  if (durationMs <= 0 || frameIndex < 0) return null

  return { frameIndex, durationMs }
}

/// 一组帧——按 frameIndex 升序排列，每帧带 textureUrl + duration。
export interface SpriteFrame {
  textureUrl: string
  durationMs: number
}

export interface SpriteSet {
  graph: string       // GraphType，e.g. "Default"
  animat: string      // AnimatType，e.g. "Single" / "A_Start"
  mode: string        // ModeType，e.g. "Normal" / "Happy"
  loop: boolean       // 是否循环
  next?: string       // 链式下一段（A_Start → B_Loop 用）
  frames: SpriteFrame[]
}

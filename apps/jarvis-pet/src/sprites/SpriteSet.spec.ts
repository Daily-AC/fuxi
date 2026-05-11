import { describe, it, expect } from 'vitest'
import { parseFrameFilename } from './SpriteSet'

describe('parseFrameFilename', () => {
  it('解析 default_001_120.png → frame 1, duration 120ms', () => {
    expect(parseFrameFilename('default_001_120.png')).toEqual({
      frameIndex: 1,
      durationMs: 120
    })
  })

  it('解析 idle_010_50.png', () => {
    expect(parseFrameFilename('idle_010_50.png')).toEqual({
      frameIndex: 10,
      durationMs: 50
    })
  })

  it('文件名不符合约定 → 返 null（让调用方过滤掉）', () => {
    expect(parseFrameFilename('not_a_frame.png')).toBeNull()
    expect(parseFrameFilename('default_001.png')).toBeNull() // 缺 duration
    expect(parseFrameFilename('default.png')).toBeNull()
  })

  it('多下划线名字（如 a_start_b_loop_001_100.png）—— 取最后两段', () => {
    expect(parseFrameFilename('a_start_001_100.png')).toEqual({
      frameIndex: 1,
      durationMs: 100
    })
  })

  it('忽略大小写后缀', () => {
    expect(parseFrameFilename('default_001_120.PNG')).toEqual({
      frameIndex: 1,
      durationMs: 120
    })
  })
})

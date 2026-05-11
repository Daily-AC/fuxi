import { describe, it, expect } from 'vitest'
import { parseLps } from './lpsParser'

describe('parseLps', () => {
  it('解析单 section', () => {
    const input = `[character]
name: 玄女
author: fuxi
version: 0.4`
    const result = parseLps(input)
    expect(result).toEqual([
      { section: 'character', fields: { name: '玄女', author: 'fuxi', version: '0.4' } }
    ])
  })

  it('解析多 section（同名 section 算两条独立）', () => {
    const input = `[pnganimation]
graph: Default
animat: Single
mode: Normal
path: ./default
loop: true

[pnganimation]
graph: Touch_Head
animat: A_Start
mode: Happy
path: ./touch_head/happy
loop: false`
    const result = parseLps(input)
    expect(result).toHaveLength(2)
    expect(result[0].section).toBe('pnganimation')
    expect(result[0].fields.graph).toBe('Default')
    expect(result[1].fields.graph).toBe('Touch_Head')
    expect(result[1].fields.loop).toBe('false')
  })

  it('忽略空行 + # 注释', () => {
    const input = `# 这是注释
[character]
# 段内注释
name: 玄女

# 末尾注释`
    const result = parseLps(input)
    expect(result).toEqual([
      { section: 'character', fields: { name: '玄女' } }
    ])
  })

  it('field 值可以含冒号（只切第一个）', () => {
    const input = `[meta]
url: https://example.com/path:1234`
    const result = parseLps(input)
    expect(result[0].fields.url).toBe('https://example.com/path:1234')
  })

  it('段外内容报错', () => {
    const input = `name: 玄女`
    expect(() => parseLps(input)).toThrow(/段外/)
  })
})

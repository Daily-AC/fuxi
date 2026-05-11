/// VPet info.lps 文本格式 parser。
/// 格式：[section] 后跟 key: value 行；同一个 [section] 名出现多次算独立 entry。
/// 行首 # 是注释；空行忽略。值里的 ':' 不切（只切第一个）。

export interface LpsSection {
  section: string
  fields: Record<string, string>
}

export function parseLps(input: string): LpsSection[] {
  const result: LpsSection[] = []
  let current: LpsSection | null = null

  const lines = input.split(/\r?\n/)
  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i]
    const line = raw.trim()
    if (!line || line.startsWith('#')) continue

    const sectionMatch = line.match(/^\[(.+?)\]$/)
    if (sectionMatch) {
      current = { section: sectionMatch[1], fields: {} }
      result.push(current)
      continue
    }

    if (!current) {
      throw new Error(`第 ${i + 1} 行段外内容："${raw}"`)
    }

    const colonIdx = line.indexOf(':')
    if (colonIdx === -1) {
      throw new Error(`第 ${i + 1} 行缺 ':'："${raw}"`)
    }
    const key = line.slice(0, colonIdx).trim()
    const value = line.slice(colonIdx + 1).trim()
    current.fields[key] = value
  }

  return result
}

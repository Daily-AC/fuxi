/// POST /api/intervene —— 桌宠把用户的话送给玄女门客。
///
/// 玄女层 Idle 自动 degrade dispatch（Decision 04）；Busy 入 pending queue
/// （M2.1）。桌宠这层不区分，丢出去就完事，玄女回 say 走 WS /api/conv。

export async function sendIntervene(opts: {
  baseURL: string
  token: string
  text: string
  interrupt?: boolean
}): Promise<void> {
  const r = await fetch(`${opts.baseURL}/api/intervene`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${opts.token}`
    },
    body: JSON.stringify({ text: opts.text, interrupt: !!opts.interrupt })
  })
  if (!r.ok) {
    const body = await r.text().catch(() => '')
    throw new Error(`intervene ${r.status}: ${body.slice(0, 100)}`)
  }
}

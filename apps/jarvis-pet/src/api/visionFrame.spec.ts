import { describe, it, expect, vi, beforeEach } from 'vitest'
import { uploadVisionFrame, VisionUploadError } from './visionFrame'

describe('uploadVisionFrame', () => {
  let fetchMock: ReturnType<typeof vi.fn>

  beforeEach(() => {
    fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ ok: true }), { status: 200 })
    )
    vi.stubGlobal('fetch', fetchMock)
  })

  it('POST 到 /api/xuannv/look/frame，带 Authorization', async () => {
    const blob = new Blob([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], { type: 'image/png' })
    await uploadVisionFrame({
      baseURL: 'https://im.example.com:8443',
      token: 'tok-abc',
      requestId: 'req-1',
      blob,
      mime: 'image/png',
    })
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0]
    expect(url).toBe('https://im.example.com:8443/api/xuannv/look/frame')
    expect(init.method).toBe('POST')
    expect((init.headers as Record<string, string>)['Authorization']).toBe('Bearer tok-abc')
  })

  it('multipart 字段名严格：request_id / file / mime', async () => {
    const blob = new Blob([new Uint8Array([1, 2, 3])], { type: 'image/png' })
    await uploadVisionFrame({
      baseURL: 'https://im.example.com',
      token: 'tok',
      requestId: 'req-xyz',
      blob,
      mime: 'image/png',
    })
    const init = fetchMock.mock.calls[0][1] as RequestInit
    const body = init.body as FormData
    expect(body).toBeInstanceOf(FormData)
    expect(body.get('request_id')).toBe('req-xyz')
    expect(body.get('mime')).toBe('image/png')
    const file = body.get('file')
    expect(file).toBeInstanceOf(Blob)
    expect((file as Blob).type).toBe('image/png')
  })

  it('error 字段：用户禁眼时上传 user_denied 占位（不附 file）', async () => {
    await uploadVisionFrame({
      baseURL: 'https://im.example.com',
      token: 'tok',
      requestId: 'req-1',
      error: 'user_denied',
    })
    const init = fetchMock.mock.calls[0][1] as RequestInit
    const body = init.body as FormData
    expect(body.get('request_id')).toBe('req-1')
    expect(body.get('error')).toBe('user_denied')
    expect(body.get('file')).toBeNull()
  })

  it('5xx 抛 VisionUploadError 带状态码', async () => {
    fetchMock.mockResolvedValueOnce(new Response('boom', { status: 500 }))
    const blob = new Blob([new Uint8Array([1])], { type: 'image/png' })
    await expect(uploadVisionFrame({
      baseURL: 'https://x',
      token: 't',
      requestId: 'r',
      blob,
      mime: 'image/png',
    })).rejects.toThrow(VisionUploadError)
  })
})

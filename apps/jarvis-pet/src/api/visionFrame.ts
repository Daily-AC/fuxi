/// POST /api/xuannv/look/frame —— 桌宠端把采到的一帧（或禁眼占位）回传给 fuxi-im。
///
/// 字段约定（spec 2026-05-14-xuannv-vision-design.md）：
///  - `request_id` (text)：vision_request 事件里的 id，让后端 oneshot 配对
///  - `file` (binary, 可选)：blob 帧；用户禁眼时不附，靠 `error` 字段
///  - `mime` (text, 可选)：默认 image/png
///  - `error` (text, 可选)：`user_denied` / `permission_denied` / `no_device` 等

export class VisionUploadError extends Error {
  constructor(
    message: string,
    public readonly status: number,
    public readonly bodySnippet: string,
  ) {
    super(message)
    this.name = 'VisionUploadError'
  }
}

export type VisionUploadOpts = {
  baseURL: string
  token: string
  requestId: string
} & (
  | { blob: Blob; mime: string; error?: undefined }
  | { blob?: undefined; mime?: undefined; error: string }
)

export async function uploadVisionFrame(opts: VisionUploadOpts): Promise<void> {
  const form = new FormData()
  form.append('request_id', opts.requestId)
  if (opts.blob) {
    form.append('mime', opts.mime)
    // filename 让 axum/multer 等都能识别 file part；扩展名跟 mime 大致对齐
    const ext = opts.mime === 'image/jpeg' ? 'jpg' : 'png'
    form.append('file', opts.blob, `${opts.requestId}.${ext}`)
  } else {
    form.append('error', opts.error)
  }
  // 注意：不要手动 set Content-Type——浏览器会自动加 boundary，
  // 强行覆盖会让 axum 拒收 multipart
  const r = await fetch(`${opts.baseURL}/api/xuannv/look/frame`, {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${opts.token}`,
    },
    body: form,
  })
  if (!r.ok) {
    const body = await r.text().catch(() => '')
    throw new VisionUploadError(
      `vision-frame upload failed: ${r.status}`,
      r.status,
      body.slice(0, 200),
    )
  }
}

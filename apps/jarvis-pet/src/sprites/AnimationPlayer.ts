import { Sprite, Texture, Assets, type Application } from 'pixi.js'
import type { SpriteSet } from './SpriteSet'

/// 播放一组 SpriteSet —— 按 frame durationMs 切换 texture。
///
/// 用 PIXI.Ticker 驱动；每 tick 累加 elapsedMs，超过当前帧 duration 切下一帧。
/// loop=true 时循环；loop=false 播完触发 onComplete。
export class AnimationPlayer {
  private sprite: Sprite
  private textures: Texture[] = []
  private elapsedMs = 0
  private frameIdx = 0
  private set: SpriteSet | null = null
  private onCompleteHandler?: () => void

  constructor(private app: Application) {
    this.sprite = new Sprite()
    this.sprite.anchor.set(0.5, 1.0) // 锚点：底部居中（脚下）
    this.app.stage.addChild(this.sprite)
    this.app.ticker.add(this.tick, this)
  }

  /// 加载一组 SpriteSet 的所有 textures，准备播放。
  /// 调用前 sprite 不显示；async 等所有图加载完才返回。
  async load(set: SpriteSet): Promise<void> {
    const urls = set.frames.map(f => f.textureUrl)
    const loaded = await Assets.load(urls)
    this.textures = urls.map(u => loaded[u] as Texture)
    this.set = set
    this.frameIdx = 0
    this.elapsedMs = 0
    this.sprite.texture = this.textures[0]
    // fit to panel：PIXI v8 autoDensity:true 下 app.screen.{w,h} 是 logical
    // 尺寸（已除 resolution），可直接用作 scale 基准。锚点 (0.5,1.0) 让脚下踩底沿。
    const tex0 = this.textures[0]
    const scale = this.app.screen.height / tex0.height
    this.sprite.scale.set(scale, scale)
    this.sprite.x = this.app.screen.width / 2
    this.sprite.y = this.app.screen.height
  }

  /// 单次 tick 由 PIXI Ticker 自动调用
  private tick(ticker: { deltaMS: number }): void {
    if (!this.set || this.textures.length === 0) return
    this.elapsedMs += ticker.deltaMS
    const cur = this.set.frames[this.frameIdx]
    if (this.elapsedMs >= cur.durationMs) {
      this.elapsedMs -= cur.durationMs
      this.frameIdx++
      if (this.frameIdx >= this.set.frames.length) {
        if (this.set.loop) {
          this.frameIdx = 0
        } else {
          this.frameIdx = this.set.frames.length - 1
          this.set = null
          this.onCompleteHandler?.()
          return
        }
      }
      this.sprite.texture = this.textures[this.frameIdx]
    }
  }

  onComplete(handler: () => void): void {
    this.onCompleteHandler = handler
  }

  /// 一次性播 set，播完自动 load resumeSet 恢复。常用：触摸/Say 动画 → 回 Default 循环。
  /// 强制 loop=false 不管传入 set.loop 怎么标。
  async playOnce(set: SpriteSet, resumeSet: SpriteSet): Promise<void> {
    return new Promise((resolve, reject) => {
      this.onCompleteHandler = () => {
        this.onCompleteHandler = undefined
        this.load(resumeSet).then(resolve).catch(reject)
      }
      this.load({ ...set, loop: false }).catch(reject)
    })
  }

  destroy(): void {
    this.app.ticker.remove(this.tick, this)
    this.sprite.destroy()
  }
}

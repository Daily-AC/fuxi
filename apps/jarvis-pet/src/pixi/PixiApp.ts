import { Application } from 'pixi.js'

/// PixiJS Application 单例 —— 跟 Vue 组件解耦，便于测试 mock。
/// 初始化后 .canvas 是 HTMLCanvasElement，挂到 Vue 容器即可显示。
export class PixiApp {
  private app: Application | null = null

  async init(opts: { width: number; height: number }): Promise<HTMLCanvasElement> {
    if (this.app) {
      throw new Error('PixiApp 已初始化')
    }
    this.app = new Application()
    await this.app.init({
      width: opts.width,
      height: opts.height,
      // PIXI v8：background 取代了 v7 backgroundColor；'transparent' 让 WebGL
      // context 用 alpha:true 创建，clear color alpha=0 渲染透明，跟 NSPanel
      // opaque=false 配套才能让桌面背景透下来。
      background: 'transparent',
      backgroundAlpha: 0,
      antialias: true,
      // mac WebKit 性能：preferWebGL2，不用 WebGPU（mac WebKit WebGPU 还不稳）
      preference: 'webgl',
      // autoDensity:true + resolution:dpr 让 Retina sprite 清晰：canvas 物理
      // 像素 = logical * dpr，css 尺寸保持 logical。app.screen.{w,h} 仍是
      // logical，不受 dpr 污染，AnimationPlayer 直接用即可。
      resolution: window.devicePixelRatio || 1,
      autoDensity: true,
      // 不保留 drawing buffer：每帧 clear，避免 Safari WebGL 残影
      clearBeforeRender: true
    })
    // canvas DOM 显式 transparent，避免 webkit 默认 background 浮出
    this.app.canvas.style.background = 'transparent'
    return this.app.canvas
  }

  get pixi(): Application {
    if (!this.app) throw new Error('PixiApp 未初始化')
    return this.app
  }

  destroy(): void {
    if (this.app) {
      this.app.destroy(true, { children: true, texture: true })
      this.app = null
    }
  }
}

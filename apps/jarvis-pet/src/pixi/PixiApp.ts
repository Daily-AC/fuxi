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
      // 透明背景：让 Tauri 透明窗口的桌面背景透出来
      backgroundAlpha: 0,
      antialias: true,
      // mac WebKit 性能：preferWebGL2，不用 WebGPU（mac WebKit WebGPU 还不稳）
      preference: 'webgl',
      // 抗锯齿权衡：开了清晰但拖动时性能 -10%；桌宠主要静态可接受
      resolution: window.devicePixelRatio || 1,
      autoDensity: true
    })
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

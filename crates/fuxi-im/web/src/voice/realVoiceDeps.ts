/// VoiceController 的真实浏览器依赖装配——把移植来的 voice 五件套 + api client
/// 绑成 VoiceDeps。逻辑都在 voiceController.ts，这里只做胶水（不单测，由
/// home 实测覆盖）。
import type { ApiClient } from '~/lib/api'
import { AsrClient } from './asrClient'
import { MicRecorder } from './micRecorder'
import { playTts, stopTts } from './tts'
import { EnergyVad } from './vad'
import type { VoiceDeps } from './voiceController'
import { WakeClient } from './wakeClient'

export function realVoiceDeps(api: ApiClient): VoiceDeps {
  // asr/tts/wake 都挂在同一个 im 域（nginx 反代），同源直连
  const baseURL = location.origin
  return {
    fetchTokens: async () => {
      const r = await api.voiceTokens()
      return { imToken: r.im_token, wakeToken: r.wake_token ?? null }
    },
    createMic: () => new MicRecorder(),
    createWake: o =>
      new WakeClient({
        baseURL,
        token: o.token,
        onWake: () => o.onWake(),
        onStatus: o.onStatus
      }),
    createAsr: o => new AsrClient({ baseURL, token: o.token }),
    createVad: onSilence => new EnergyVad({ onSilence }),
    createTts: token => ({
      play: (text, emotion) => playTts({ baseURL, token, text, emotion }),
      stop: stopTts
    }),
    intervene: text => api.intervene({ text }).then(() => undefined)
  }
}

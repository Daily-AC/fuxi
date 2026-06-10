// VoiceController 状态机单测——语音模式全闭环的逻辑层回归门禁。
// 真实浏览器依赖（mic/wake/asr/tts）全部注入 fake，只测编排逻辑：
//   off → enable → listening → wake → dictating → intervene → listening
//   + 回复自动 TTS + 按住说话 + 错误兜底回 listening。
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  VoiceController,
  type VoiceDeps,
} from "../../src/voice/voiceController";

type PcmCb = (chunk: ArrayBuffer) => void;

function makeFakes() {
  const micSubs: PcmCb[] = [];
  const mic = {
    started: false,
    stopped: false,
    async start() {
      this.started = true;
    },
    stop() {
      this.stopped = true;
    },
    subscribe(cb: PcmCb) {
      micSubs.push(cb);
      return () => {
        const i = micSubs.indexOf(cb);
        if (i >= 0) micSubs.splice(i, 1);
      };
    },
  };

  let wakeCb: (() => void) | null = null;
  const wake = {
    started: false,
    stopped: false,
    pcm: [] as ArrayBuffer[],
    start() {
      this.started = true;
    },
    stop() {
      this.stopped = true;
    },
    sendPcm(c: ArrayBuffer) {
      this.pcm.push(c);
    },
  };

  const asrInstances: Array<{
    connected: boolean;
    aborted: boolean;
    pcm: ArrayBuffer[];
    finishText: string;
    finish: ReturnType<typeof vi.fn>;
  }> = [];
  function newAsr() {
    const inst = {
      connected: false,
      aborted: false,
      pcm: [] as ArrayBuffer[],
      finishText: "帮我查下天气",
      async connect() {
        inst.connected = true;
      },
      sendPcm(c: ArrayBuffer) {
        inst.pcm.push(c);
      },
      finish: vi.fn(async () => ({ text: inst.finishText })),
      abort() {
        inst.aborted = true;
      },
    };
    asrInstances.push(inst);
    return inst;
  }

  let vadSilence: (() => void) | null = null;
  const vads: Array<{ fed: number }> = [];
  function newVad(onSilence: () => void) {
    vadSilence = onSilence;
    const v = {
      fed: 0,
      feed() {
        v.fed++;
      },
      reset() {},
    };
    vads.push(v);
    return v;
  }

  const intervene = vi.fn(async (_text: string) => {});
  const playTts = vi.fn(async (_text: string, _emotion?: string) => {});
  const stopTts = vi.fn();
  const fetchTokens = vi.fn(async () => ({
    imToken: "im-tok",
    wakeToken: "wake-tok" as string | null,
  }));

  const deps: VoiceDeps = {
    fetchTokens,
    createMic: () => mic,
    createWake: (opts) => {
      wakeCb = opts.onWake;
      return wake;
    },
    createAsr: () => newAsr(),
    createVad: (onSilence) => newVad(onSilence),
    createTts: () => ({ play: playTts, stop: stopTts }),
    intervene,
  };

  return {
    deps,
    mic,
    micSubs,
    wake,
    asrInstances,
    fireWake: () => wakeCb?.(),
    fireSilence: () => vadSilence?.(),
    intervene,
    playTts,
    stopTts,
    fetchTokens,
    feedMic: (c: ArrayBuffer) => micSubs.forEach((cb) => cb(c)),
  };
}

const CHUNK = new Int16Array(16).buffer;

// flushes pending microtasks（controller 内部 async 链）
const tick = () => new Promise((r) => setTimeout(r, 0));

describe("VoiceController 语音模式", () => {
  let f: ReturnType<typeof makeFakes>;
  let vc: VoiceController;
  let states: string[];

  beforeEach(() => {
    f = makeFakes();
    vc = new VoiceController(f.deps);
    states = [];
    vc.onState((s) => states.push(s));
  });

  it("enable：换 token + 起 mic + 起 wake，进入 listening，PCM 流向 wake", async () => {
    await vc.enable();
    expect(f.fetchTokens).toHaveBeenCalled();
    expect(f.mic.started).toBe(true);
    expect(f.wake.started).toBe(true);
    expect(vc.state).toBe("listening");

    f.feedMic(CHUNK);
    expect(f.wake.pcm.length).toBe(1);
  });

  it("wake 事件 → 听写：ASR 收 PCM、wake 不再收；静音 → intervene → 回 listening", async () => {
    await vc.enable();
    f.fireWake();
    await tick();
    expect(vc.state).toBe("dictating");
    expect(f.asrInstances.length).toBe(1);

    const before = f.wake.pcm.length;
    f.feedMic(CHUNK);
    expect(f.asrInstances[0]!.pcm.length).toBe(1);
    expect(f.wake.pcm.length).toBe(before); // 听写期间 wake 暂停喂

    f.fireSilence();
    await tick();
    expect(f.intervene).toHaveBeenCalledWith("帮我查下天气");
    expect(vc.state).toBe("listening");

    // 听写结束 wake 恢复喂
    f.feedMic(CHUNK);
    expect(f.wake.pcm.length).toBe(before + 1);
  });

  it("ASR 出空文本不 intervene，直接回 listening", async () => {
    await vc.enable();
    f.fireWake();
    await tick();
    f.asrInstances[0]!.finishText = "  ";
    f.fireSilence();
    await tick();
    expect(f.intervene).not.toHaveBeenCalled();
    expect(vc.state).toBe("listening");
  });

  it("intervene 报错 → onError + 回 listening 不卡死", async () => {
    const errors: string[] = [];
    vc.onError((e) => errors.push(e));
    f.intervene.mockRejectedValueOnce(new Error("门客正忙"));

    await vc.enable();
    f.fireWake();
    await tick();
    f.fireSilence();
    await tick();
    expect(errors.length).toBe(1);
    expect(vc.state).toBe("listening");
  });

  it("语音模式开着时玄女回复自动 TTS（emotion 透传），off 时不播", async () => {
    await vc.enable();
    vc.onXuannvReply("好的以琳", "happy");
    await tick();
    expect(f.playTts).toHaveBeenCalledWith("好的以琳", "happy");

    await vc.disable();
    vc.onXuannvReply("这条不该播", undefined);
    await tick();
    expect(f.playTts).toHaveBeenCalledTimes(1);
  });

  it("disable：停 mic/wake/tts，回 off", async () => {
    await vc.enable();
    await vc.disable();
    expect(vc.state).toBe("off");
    expect(f.mic.stopped).toBe(true);
    expect(f.wake.stopped).toBe(true);
    expect(f.stopTts).toHaveBeenCalled();
  });

  it("wakeToken 为 null 时 enable 抛错（UI 据此隐藏开关）", async () => {
    f.fetchTokens.mockResolvedValueOnce({ imToken: "im-tok", wakeToken: null });
    await expect(vc.enable()).rejects.toThrow();
    expect(vc.state).toBe("off");
  });
});

describe("VoiceController 按住说话（独立于语音模式）", () => {
  it("pttStart 起 ASR；pttStop 返回识别文本，不自动 intervene", async () => {
    const f = makeFakes();
    const vc = new VoiceController(f.deps);

    await vc.pttStart();
    expect(f.asrInstances.length).toBe(1);
    f.feedMic(CHUNK);
    expect(f.asrInstances[0]!.pcm.length).toBe(1);

    const text = await vc.pttStop();
    expect(text).toBe("帮我查下天气");
    expect(f.intervene).not.toHaveBeenCalled();
    // 非语音模式下 PTT 结束要把 mic 关掉（别让浏览器红点常亮）
    expect(f.mic.stopped).toBe(true);
  });

  it("语音模式开着时 PTT 借用现有 mic，结束后 mic 不关、wake 恢复", async () => {
    const f = makeFakes();
    const vc = new VoiceController(f.deps);
    await vc.enable();

    await vc.pttStart();
    const before = f.wake.pcm.length;
    f.feedMic(CHUNK);
    expect(f.wake.pcm.length).toBe(before); // PTT 期间 wake 暂停

    await vc.pttStop();
    expect(f.mic.stopped).toBe(false);
    f.feedMic(CHUNK);
    expect(f.wake.pcm.length).toBe(before + 1);
  });
});

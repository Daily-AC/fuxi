// EnergyVad 纯逻辑单测——移植自 jarvis-pet，PWA 侧的回归门禁。
// 阈值语义：RMS >= threshold 算说话，连续 silenceChunks 个静音 chunk 且
// 之前至少 minVoiceChunks 个语音 chunk 才 fire onSilence（一次性）。
import { describe, expect, it, vi } from "vitest";
import { EnergyVad } from "../../src/voice/vad";

function chunk(amplitude: number, samples = 1024): ArrayBuffer {
  const a = new Int16Array(samples);
  a.fill(amplitude);
  return a.buffer;
}

const VOICE = chunk(3000);
const SILENCE = chunk(50);

describe("EnergyVad", () => {
  it("说话后静音满 N 个 chunk 才触发一次 onSilence", () => {
    const onSilence = vi.fn();
    const vad = new EnergyVad({ silenceChunks: 3, minVoiceChunks: 2, onSilence });

    vad.feed(VOICE);
    vad.feed(VOICE);
    vad.feed(SILENCE);
    vad.feed(SILENCE);
    expect(onSilence).not.toHaveBeenCalled();
    vad.feed(SILENCE);
    expect(onSilence).toHaveBeenCalledTimes(1);

    // fire 之后再喂不重复触发
    vad.feed(SILENCE);
    vad.feed(SILENCE);
    expect(onSilence).toHaveBeenCalledTimes(1);
  });

  it("没说够 minVoiceChunks 时纯静音不触发（防一开就 fire）", () => {
    const onSilence = vi.fn();
    const vad = new EnergyVad({ silenceChunks: 2, minVoiceChunks: 3, onSilence });

    for (let i = 0; i < 10; i++) vad.feed(SILENCE);
    expect(onSilence).not.toHaveBeenCalled();
  });

  it("中途说话会清零静音计数", () => {
    const onSilence = vi.fn();
    const vad = new EnergyVad({ silenceChunks: 3, minVoiceChunks: 1, onSilence });

    vad.feed(VOICE);
    vad.feed(SILENCE);
    vad.feed(SILENCE);
    vad.feed(VOICE); // 重新开口 → silentRun 归零
    vad.feed(SILENCE);
    vad.feed(SILENCE);
    expect(onSilence).not.toHaveBeenCalled();
    vad.feed(SILENCE);
    expect(onSilence).toHaveBeenCalledTimes(1);
  });

  it("reset 后可再次触发", () => {
    const onSilence = vi.fn();
    const vad = new EnergyVad({ silenceChunks: 1, minVoiceChunks: 1, onSilence });

    vad.feed(VOICE);
    vad.feed(SILENCE);
    expect(onSilence).toHaveBeenCalledTimes(1);

    vad.reset();
    vad.feed(VOICE);
    vad.feed(SILENCE);
    expect(onSilence).toHaveBeenCalledTimes(2);
  });
});

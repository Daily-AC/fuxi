package cn.qmledmq.fuxi.im.voice;

/** 极简能量 VAD——jarvis-pet / PWA 同款算法的 Java 移植。
 *
 *  每收一个 int16 PCM chunk 算 RMS：>= 阈值算说话，连续 silenceChunks 个
 *  静音 chunk 且之前至少 minVoiceChunks 个语音 chunk → fire 一次（之后
 *  要 reset 才能再 fire）。室内噪声 RMS 通常 <300，正常说话 1500-8000，
 *  阈值 600 是稳的中间值。 */
final class EnergyVad {
    interface OnSilence {
        void fire();
    }

    private final int threshold;
    private final int silenceChunks;
    private final int minVoiceChunks;
    private final OnSilence onSilence;

    private int silentRun = 0;
    private int voiceCount = 0;
    private boolean fired = false;

    EnergyVad(int threshold, int silenceChunks, int minVoiceChunks, OnSilence onSilence) {
        this.threshold = threshold;
        this.silenceChunks = silenceChunks;
        this.minVoiceChunks = minVoiceChunks;
        this.onSilence = onSilence;
    }

    /** chunk 是 little-endian int16 PCM 字节。 */
    void feed(byte[] pcm, int len) {
        if (fired) {
            return;
        }
        long sumSq = 0;
        int samples = len / 2;
        for (int i = 0; i < samples; i++) {
            int lo = pcm[i * 2] & 0xff;
            int hi = pcm[i * 2 + 1];
            int s = (hi << 8) | lo;
            sumSq += (long) s * s;
        }
        double rms = Math.sqrt((double) sumSq / Math.max(1, samples));
        if (rms >= threshold) {
            silentRun = 0;
            voiceCount++;
        } else {
            silentRun++;
            if (voiceCount >= minVoiceChunks && silentRun >= silenceChunks) {
                fired = true;
                onSilence.fire();
            }
        }
    }

    void reset() {
        silentRun = 0;
        voiceCount = 0;
        fired = false;
    }
}

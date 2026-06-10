package cn.qmledmq.fuxi.im.voice;

import android.content.Context;
import android.media.AudioAttributes;
import android.media.MediaPlayer;
import android.util.Log;

import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;

import cn.qmledmq.fuxi.im.FcmRegistrar;

/** POST /api/tts（GPT-SoVITS 角色音色）→ wav → MediaPlayer 播放。
 *
 *  - 同时只播一段；新播放打断旧的（同 PWA tts.ts 语义）。
 *  - 唤醒应答「我在」首次合成后缓存到 filesDir，之后唤醒零网络延迟。
 *  - wav 落临时文件再交 MediaPlayer——比 AudioTrack 手解 wav header 省事，
 *    payload 通常 <200KB，磁盘往返可忽略。 */
final class TtsPlayer {
    private static final String TAG = "TtsPlayer";
    private static final String URL_TTS = FcmRegistrar.BASE + "/api/tts";
    private static final String ACK_CACHE = "voice_ack.wav";

    private final Context ctx;
    private MediaPlayer current;

    TtsPlayer(Context ctx) {
        this.ctx = ctx.getApplicationContext();
    }

    /** 合成并播放；阻塞到播放结束（service 的语音流水线是单线程顺序的）。 */
    synchronized void playBlocking(String imToken, String text, String emotion) {
        byte[] wav = synth(imToken, text, emotion);
        if (wav == null) {
            return;
        }
        File f = new File(ctx.getCacheDir(), "voice_tts_" + System.nanoTime() + ".wav");
        try {
            try (FileOutputStream os = new FileOutputStream(f)) {
                os.write(wav);
            }
            playFileBlocking(f);
        } catch (Exception e) {
            Log.w(TAG, "TTS 播放失败", e);
        } finally {
            //noinspection ResultOfMethodCallIgnored
            f.delete();
        }
    }

    /** 只合成并缓存「我在」，不出声——service 启动时预热用。 */
    synchronized void prefetchAck(String imToken) {
        File cache = new File(ctx.getFilesDir(), ACK_CACHE);
        if (cache.exists()) {
            return;
        }
        byte[] wav = synth(imToken, "我在", null);
        if (wav == null) {
            return;
        }
        try (FileOutputStream os = new FileOutputStream(cache)) {
            os.write(wav);
        } catch (Exception e) {
            Log.w(TAG, "ack 缓存写入失败", e);
        }
    }

    /** 播放唤醒应答「我在」——优先本地缓存，miss 时合成并落缓存。 */
    synchronized void playAckBlocking(String imToken) {
        prefetchAck(imToken);
        File cache = new File(ctx.getFilesDir(), ACK_CACHE);
        if (!cache.exists()) {
            return; // sovits 挂了不该卡死唤醒——没 ack 直接进听写
        }
        try {
            playFileBlocking(cache);
        } catch (Exception e) {
            Log.w(TAG, "ack 播放失败", e);
        }
    }

    synchronized void stop() {
        if (current != null) {
            try {
                current.stop();
            } catch (Exception ignored) {
            }
            current.release();
            current = null;
        }
    }

    private void playFileBlocking(File f) throws Exception {
        stop();
        final Object done = new Object();
        MediaPlayer mp = new MediaPlayer();
        current = mp;
        mp.setAudioAttributes(new AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_ASSISTANT)
                .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                .build());
        mp.setDataSource(f.getAbsolutePath());
        mp.setOnCompletionListener(p -> {
            synchronized (done) {
                done.notifyAll();
            }
        });
        mp.setOnErrorListener((p, what, extra) -> {
            synchronized (done) {
                done.notifyAll();
            }
            return true;
        });
        mp.prepare();
        mp.start();
        // 上限 60s 防 MediaPlayer 卡死吊住整条语音流水线
        synchronized (done) {
            done.wait(60_000);
        }
        synchronized (this) {
            if (current == mp) {
                current = null;
            }
        }
        mp.release();
    }

    private byte[] synth(String imToken, String text, String emotion) {
        HttpURLConnection conn = null;
        try {
            JSONObject body = new JSONObject();
            body.put("text", text);
            if (emotion != null && !emotion.isEmpty()) {
                body.put("emotion", emotion);
            }
            byte[] payload = body.toString().getBytes(StandardCharsets.UTF_8);
            conn = (HttpURLConnection) new URL(URL_TTS).openConnection();
            conn.setRequestMethod("POST");
            conn.setDoOutput(true);
            conn.setConnectTimeout(5000);
            conn.setReadTimeout(30000);
            conn.setRequestProperty("Content-Type", "application/json");
            conn.setRequestProperty("Authorization", "Bearer " + imToken);
            conn.getOutputStream().write(payload);
            int code = conn.getResponseCode();
            if (code < 200 || code >= 300) {
                Log.w(TAG, "tts HTTP " + code);
                return null;
            }
            try (InputStream is = conn.getInputStream()) {
                java.io.ByteArrayOutputStream bos = new java.io.ByteArrayOutputStream();
                byte[] tmp = new byte[8192];
                int n;
                while ((n = is.read(tmp)) > 0) {
                    bos.write(tmp, 0, n);
                }
                return bos.toByteArray();
            }
        } catch (Exception e) {
            Log.w(TAG, "tts 合成异常", e);
            return null;
        } finally {
            if (conn != null) {
                conn.disconnect();
            }
        }
    }
}

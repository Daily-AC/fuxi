package cn.qmledmq.fuxi.im.voice;

import android.annotation.SuppressLint;
import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.media.AudioFormat;
import android.media.AudioRecord;
import android.media.MediaRecorder;
import android.os.IBinder;
import android.os.PowerManager;
import android.util.Log;

import androidx.core.app.NotificationCompat;

import org.json.JSONObject;

import java.nio.charset.StandardCharsets;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.SynchronousQueue;
import java.util.concurrent.TimeUnit;

import cn.qmledmq.fuxi.im.FcmRegistrar;
import cn.qmledmq.fuxi.im.R;

import okhttp3.OkHttpClient;
import okhttp3.Request;
import okhttp3.Response;
import okhttp3.WebSocket;
import okhttp3.WebSocketListener;
import okio.ByteString;

/** 后台常听前台服务——贾维斯闭环的原生实现（锁屏/切后台可用）。
 *
 *  链路：AudioRecord 16kHz → wake WS 喊「玄女」检测 → ack「我在」→ ASR WS
 *  听写（能量 VAD 1.5s 静音断）→ POST /api/intervene（[语音] 前缀，公理 #8）
 *  → conv WS 收 xuannv_voice_line → POST /api/tts → MediaPlayer 播。
 *
 *  v1.3 起原生全场景唯一接管（前台/后台/锁屏）：讯飞引擎进程级单 session，
 *  壳内 PWA 语音模式已按 UA 标记禁用，否则双端互抢 18310。唯一暂停源是
 *  常驻通知里的「暂停常听」按钮（释放麦克风、不响应 wake、不播 TTS）。
 *
 *  WHY 控制面只有通知按钮：远端 URL 模式 WebView 没有 Capacitor JS 桥，
 *  PWA 调不了原生插件（同 FcmRegistrar 注释）。 */
public class VoiceLoopService extends Service {
    private static final String TAG = "VoiceLoop";

    public static final String ACTION_START = "cn.qmledmq.fuxi.im.voice.START";
    public static final String ACTION_PAUSE = "cn.qmledmq.fuxi.im.voice.PAUSE";
    public static final String ACTION_RESUME = "cn.qmledmq.fuxi.im.voice.RESUME";

    private static final String CHANNEL_ID = "fuxi_voice_loop";
    private static final int NOTIF_ID = 9100;
    private static final String PREFS = "fuxi_voice_loop";
    private static final String KEY_USER_PAUSED = "user_paused";

    /** 16kHz mono int16 · 40ms = 1280 字节/chunk。 */
    private static final int SAMPLE_RATE = 16000;
    private static final int CHUNK_BYTES = 1280;
    /** VAD：40ms chunk × 38 ≈ 1.5s 静音断句；先说满 6 chunk（240ms）才算开过口。 */
    private static final int VAD_SILENCE_CHUNKS = 38;
    private static final int VAD_MIN_VOICE_CHUNKS = 6;
    private static final int VAD_THRESHOLD = 600;
    /** 听写硬上限——嘈杂环境 VAD 不触发时强制收束。 */
    private static final long DICTATION_MAX_MS = 15_000;

    private static final int PHASE_LISTENING = 0;
    private static final int PHASE_DICTATING = 1;

    private volatile boolean running = false;
    private volatile boolean userPaused = false;
    private volatile int phase = PHASE_LISTENING;

    private volatile VoiceTokens tokens;
    private OkHttpClient http;
    private TtsPlayer tts;
    private PowerManager.WakeLock wakeLock;

    private volatile WebSocket wakeWs;
    /** wake 握手完成（收到 server ready）前不得喂 PCM——audioLoop 在 WS 连接期间
     *  send() 会被 OkHttp 排队、握手后先于 onOpen 的 hello 冲出去，server 首帧
     *  校验「期望 hello 收到二进制」直接拒，1s 重连死循环（2026-06-10 实测）。 */
    private volatile boolean wakeReady = false;
    private volatile WebSocket convWs;
    private volatile WebSocket asrWs;
    private volatile boolean asrReady = false;
    private volatile EnergyVad vad;
    /** ASR final 文本的单次交接点。 */
    private final SynchronousQueue<String> asrFinal = new SynchronousQueue<>();

    /** 语音流水线串行执行器——ack/听写/intervene/TTS 顺序跑，天然互斥。 */
    private ExecutorService voiceExec;

    private int wakeBackoffMs = 1000;
    private int convBackoffMs = 1000;

    // ── 生命周期 ─────────────────────────────────────────────────────────

    /** 装好默认开启；mic 权限就绪时由 MainActivity 调用。 */
    public static void startIfEnabled(Context ctx) {
        SharedPreferences p = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        Intent i = new Intent(ctx, VoiceLoopService.class).setAction(ACTION_START);
        i.putExtra("user_paused", p.getBoolean(KEY_USER_PAUSED, false));
        try {
            ctx.startForegroundService(i);
        } catch (Exception e) {
            Log.w(TAG, "startForegroundService 失败", e);
        }
    }

    @Override
    public void onCreate() {
        super.onCreate();
        userPaused = getSharedPreferences(PREFS, MODE_PRIVATE).getBoolean(KEY_USER_PAUSED, false);
        ensureChannel();
        tts = new TtsPlayer(this);
        voiceExec = Executors.newSingleThreadExecutor();
        // WS 读超时 0（长连）；wake server 自己有 ping 保活
        http = new OkHttpClient.Builder()
                .readTimeout(0, TimeUnit.MILLISECONDS)
                .pingInterval(25, TimeUnit.SECONDS)
                .build();
        PowerManager pm = (PowerManager) getSystemService(POWER_SERVICE);
        wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "fuxi:voice-loop");
        wakeLock.acquire();
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        String action = intent == null ? ACTION_START : intent.getAction();
        if (ACTION_PAUSE.equals(action)) {
            setUserPaused(true);
        } else if (ACTION_RESUME.equals(action)) {
            setUserPaused(false);
        }
        startForeground(NOTIF_ID, buildNotification());
        if (!running) {
            running = true;
            new Thread(this::initThenAudioLoop, "voice-loop").start();
        }
        return START_STICKY;
    }

    @Override
    public void onDestroy() {
        running = false;
        closeQuietly(wakeWs);
        closeQuietly(convWs);
        closeQuietly(asrWs);
        if (tts != null) {
            tts.stop();
        }
        if (voiceExec != null) {
            voiceExec.shutdownNow();
        }
        if (wakeLock != null && wakeLock.isHeld()) {
            wakeLock.release();
        }
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }

    // ── 初始化 + 音频主循环 ─────────────────────────────────────────────

    private void initThenAudioLoop() {
        // token 就绪前退避等（未登录 / 网络没好）；30s 一试不耗电
        while (running && tokens == null) {
            tokens = VoiceTokens.fetch();
            if (tokens == null) {
                sleep(30_000);
            }
        }
        if (!running) {
            return;
        }
        if (tokens.wakeToken == null) {
            Log.w(TAG, "home 未部署 wake server——后台常听不可用，service 收摊");
            stopSelf();
            return;
        }
        voiceExec.submit(() -> tts.prefetchAck(tokens.imToken)); // 预热 ack 缓存，不出声
        openConvWs();
        openWakeWs();
        audioLoop();
    }

    @SuppressLint("MissingPermission") // MainActivity 拿到 RECORD_AUDIO 才会 start 本服务
    private void audioLoop() {
        AudioRecord rec = null;
        byte[] buf = new byte[CHUNK_BYTES];
        while (running) {
            if (paused()) {
                // 让位：释放麦克风（绿点消失、省电），250ms 轮询恢复条件
                if (rec != null) {
                    try {
                        rec.stop();
                    } catch (Exception ignored) {
                    }
                    rec.release();
                    rec = null;
                }
                sleep(250);
                continue;
            }
            if (rec == null) {
                rec = newRecord();
                if (rec == null) {
                    sleep(3000);
                    continue;
                }
                rec.startRecording();
            }
            int n = rec.read(buf, 0, buf.length);
            if (n <= 0) {
                sleep(10);
                continue;
            }
            ByteString bs = ByteString.of(buf, 0, n);
            if (phase == PHASE_DICTATING) {
                WebSocket asr = asrWs;
                EnergyVad v = vad;
                if (asr != null && asrReady) {
                    asr.send(bs);
                    if (v != null) {
                        v.feed(buf, n);
                    }
                }
            } else {
                WebSocket wake = wakeWs;
                if (wake != null && wakeReady) {
                    wake.send(bs);
                }
            }
        }
        if (rec != null) {
            try {
                rec.stop();
            } catch (Exception ignored) {
            }
            rec.release();
        }
    }

    private AudioRecord newRecord() {
        int min = AudioRecord.getMinBufferSize(
                SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT);
        try {
            return new AudioRecord(
                    MediaRecorder.AudioSource.VOICE_RECOGNITION,
                    SAMPLE_RATE,
                    AudioFormat.CHANNEL_IN_MONO,
                    AudioFormat.ENCODING_PCM_16BIT,
                    Math.max(min, CHUNK_BYTES * 8));
        } catch (Exception e) {
            Log.w(TAG, "AudioRecord 创建失败", e);
            return null;
        }
    }

    private boolean paused() {
        // v1.3 起前台不再让位：壳内 PWA 语音模式已禁用（讯飞引擎单 session，
        // 双端会互抢 18310），原生全场景唯一接管。只剩用户通知按钮一个暂停源。
        return userPaused;
    }

    /** RFC3339 UTC 时间串——wake 协议 Pong.at 用。minSdk 23 无 java.time，走 SimpleDateFormat。 */
    private static String utcNow() {
        java.text.SimpleDateFormat f =
                new java.text.SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss'Z'", java.util.Locale.US);
        f.setTimeZone(java.util.TimeZone.getTimeZone("UTC"));
        return f.format(new java.util.Date());
    }

    // ── wake WS ──────────────────────────────────────────────────────────

    private void openWakeWs() {
        if (!running) {
            return;
        }
        String url = wsBase() + "/wake/api/wake?token=" + tokens.wakeToken;
        wakeReady = false;
        wakeWs = http.newWebSocket(new Request.Builder().url(url).build(), new WebSocketListener() {
            @Override
            public void onOpen(WebSocket ws, Response r) {
                wakeBackoffMs = 1000;
                ws.send("{\"type\":\"hello\",\"client\":\"fuxi-android\",\"version\":\"1.3.1\"}");
            }

            @Override
            public void onMessage(WebSocket ws, String text) {
                try {
                    JSONObject m = new JSONObject(text);
                    String t = m.optString("type");
                    if ("wake".equals(t)) {
                        onWakeDetected();
                    } else if ("ping".equals(t)) {
                        // 协议 Pong 必须带 at（RFC3339）——缺字段 server 解析失败刷 warn
                        ws.send("{\"type\":\"pong\",\"at\":\"" + utcNow() + "\"}");
                    } else if ("ready".equals(t)) {
                        Log.i(TAG, "wake ready");
                        wakeReady = true;
                    }
                } catch (Exception ignored) {
                }
            }

            @Override
            public void onFailure(WebSocket ws, Throwable t, Response r) {
                wakeReady = false;
                scheduleWakeReconnect();
            }

            @Override
            public void onClosed(WebSocket ws, int code, String reason) {
                wakeReady = false;
                if (code != 1000) {
                    scheduleWakeReconnect();
                }
            }
        });
    }

    private void scheduleWakeReconnect() {
        if (!running) {
            return;
        }
        int delay = wakeBackoffMs;
        wakeBackoffMs = Math.min(wakeBackoffMs * 2, 30_000);
        new Thread(() -> {
            sleep(delay);
            openWakeWs();
        }).start();
    }

    // ── conv WS（玄女 voice_line → TTS）────────────────────────────────

    private void openConvWs() {
        if (!running) {
            return;
        }
        String url = wsBase() + "/api/conv?token=" + tokens.imToken;
        convWs = http.newWebSocket(new Request.Builder().url(url).build(), new WebSocketListener() {
            @Override
            public void onOpen(WebSocket ws, Response r) {
                convBackoffMs = 1000;
            }

            @Override
            public void onMessage(WebSocket ws, String text) {
                try {
                    JSONObject ev = new JSONObject(text);
                    JSONObject kind = ev.optJSONObject("kind");
                    if (kind == null || !"xuannv_voice_line".equals(kind.optString("type"))) {
                        return;
                    }
                    String line = kind.optString("text", "");
                    String emotion = kind.isNull("emotion") ? null : kind.optString("emotion", null);
                    if (!line.isEmpty() && !paused()) {
                        voiceExec.submit(() -> tts.playBlocking(tokens.imToken, line, emotion));
                    }
                } catch (Exception ignored) {
                }
            }

            @Override
            public void onFailure(WebSocket ws, Throwable t, Response r) {
                scheduleConvReconnect();
            }

            @Override
            public void onClosed(WebSocket ws, int code, String reason) {
                if (code != 1000) {
                    scheduleConvReconnect();
                }
            }
        });
    }

    private void scheduleConvReconnect() {
        if (!running) {
            return;
        }
        int delay = convBackoffMs;
        convBackoffMs = Math.min(convBackoffMs * 2, 30_000);
        new Thread(() -> {
            sleep(delay);
            openConvWs();
        }).start();
    }

    // ── 唤醒 → 听写 → intervene ────────────────────────────────────────

    private void onWakeDetected() {
        if (paused() || phase != PHASE_LISTENING) {
            return;
        }
        phase = PHASE_DICTATING;
        voiceExec.submit(this::dictationFlow);
    }

    private void dictationFlow() {
        try {
            tts.playAckBlocking(tokens.imToken); // 「我在」——缓存命中零延迟
            if (!openAsrAndAwaitReady()) {
                return;
            }
            vad = new EnergyVad(VAD_THRESHOLD, VAD_SILENCE_CHUNKS, VAD_MIN_VOICE_CHUNKS, () -> {
                WebSocket asr = asrWs;
                if (asr != null) {
                    asr.send("{\"type\":\"end\"}");
                }
            });
            // 等 final：VAD 断句后 server 回 final；嘈杂环境 VAD 不触发则硬上限收束
            String text = asrFinal.poll(DICTATION_MAX_MS, TimeUnit.MILLISECONDS);
            if (text == null) {
                WebSocket asr = asrWs;
                if (asr != null) {
                    asr.send("{\"type\":\"end\"}");
                }
                text = asrFinal.poll(8_000, TimeUnit.MILLISECONDS);
            }
            if (text != null && !text.trim().isEmpty()) {
                intervene("[语音] " + text.trim());
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        } finally {
            closeQuietly(asrWs);
            asrWs = null;
            asrReady = false;
            vad = null;
            phase = PHASE_LISTENING;
        }
    }

    /** 开 ASR WS、发 start 帧、等 ready。失败返 false（流水线放弃本轮）。 */
    private boolean openAsrAndAwaitReady() throws InterruptedException {
        final java.util.concurrent.CountDownLatch ready = new java.util.concurrent.CountDownLatch(1);
        String url = wsBase() + "/api/asr";
        asrReady = false;
        asrWs = http.newWebSocket(new Request.Builder().url(url).build(), new WebSocketListener() {
            @Override
            public void onOpen(WebSocket ws, Response r) {
                try {
                    JSONObject start = new JSONObject();
                    start.put("type", "start");
                    start.put("token", tokens.imToken);
                    start.put("sample_rate", SAMPLE_RATE);
                    ws.send(start.toString());
                } catch (Exception ignored) {
                }
            }

            @Override
            public void onMessage(WebSocket ws, String text) {
                try {
                    JSONObject m = new JSONObject(text);
                    String t = m.optString("type");
                    if ("ready".equals(t)) {
                        asrReady = true;
                        ready.countDown();
                    } else if ("final".equals(t)) {
                        asrFinal.offer(m.optString("text", ""), 2, TimeUnit.SECONDS);
                    } else if ("error".equals(t)) {
                        Log.w(TAG, "asr error: " + m.optString("error"));
                        asrFinal.offer("", 2, TimeUnit.SECONDS);
                    }
                } catch (Exception ignored) {
                }
            }

            @Override
            public void onFailure(WebSocket ws, Throwable t, Response r) {
                Log.w(TAG, "asr ws failure", t);
                ready.countDown();
            }
        });
        return ready.await(5, TimeUnit.SECONDS) && asrReady;
    }

    private void intervene(String text) {
        java.net.HttpURLConnection conn = null;
        try {
            JSONObject body = new JSONObject();
            body.put("text", text);
            byte[] payload = body.toString().getBytes(StandardCharsets.UTF_8);
            conn = (java.net.HttpURLConnection)
                    new java.net.URL(FcmRegistrar.BASE + "/api/intervene").openConnection();
            conn.setRequestMethod("POST");
            conn.setDoOutput(true);
            conn.setConnectTimeout(10000);
            conn.setReadTimeout(15000);
            conn.setRequestProperty("Content-Type", "application/json");
            conn.setRequestProperty("Authorization", "Bearer " + tokens.imToken);
            conn.getOutputStream().write(payload);
            int code = conn.getResponseCode();
            if (code < 200 || code >= 300) {
                Log.w(TAG, "intervene HTTP " + code);
            }
        } catch (Exception e) {
            Log.w(TAG, "intervene 异常", e);
        } finally {
            if (conn != null) {
                conn.disconnect();
            }
        }
    }

    // ── 通知 ────────────────────────────────────────────────────────────

    private void setUserPaused(boolean paused) {
        userPaused = paused;
        getSharedPreferences(PREFS, MODE_PRIVATE)
                .edit().putBoolean(KEY_USER_PAUSED, paused).apply();
        NotificationManager nm = (NotificationManager) getSystemService(NOTIFICATION_SERVICE);
        nm.notify(NOTIF_ID, buildNotification());
    }

    private Notification buildNotification() {
        Intent toggle = new Intent(this, VoiceLoopService.class)
                .setAction(userPaused ? ACTION_RESUME : ACTION_PAUSE);
        PendingIntent pi = PendingIntent.getService(
                this, 1, toggle, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        return new NotificationCompat.Builder(this, CHANNEL_ID)
                .setSmallIcon(R.drawable.ic_stat_notify)
                .setContentTitle("玄女在听")
                .setContentText(userPaused ? "常听已暂停" : "锁屏也能喊「玄女」唤醒")
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setPriority(NotificationCompat.PRIORITY_LOW)
                .addAction(0, userPaused ? "继续常听" : "暂停常听", pi)
                .build();
    }

    private void ensureChannel() {
        NotificationManager nm = (NotificationManager) getSystemService(NOTIFICATION_SERVICE);
        NotificationChannel ch = new NotificationChannel(
                CHANNEL_ID, "玄女常听", NotificationManager.IMPORTANCE_LOW);
        ch.setDescription("后台语音唤醒常驻通知");
        nm.createNotificationChannel(ch);
    }

    // ── 杂项 ────────────────────────────────────────────────────────────

    private static String wsBase() {
        return FcmRegistrar.BASE.replaceFirst("^https", "wss");
    }

    private static void closeQuietly(WebSocket ws) {
        if (ws != null) {
            try {
                ws.close(1000, null);
            } catch (Exception ignored) {
            }
        }
    }

    private static void sleep(long ms) {
        try {
            Thread.sleep(ms);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }
}

package cn.qmledmq.fuxi.im;

/** app 前台可见性的进程内标志。
 *
 *  WHY 静态标志而不是 IPC：VoiceLoopService 跟 MainActivity 同进程，
 *  service 只需在「要不要响应唤醒 / 播 TTS」时读一眼。前台时原生常听
 *  让位（避免跟 PWA 语音模式双麦克风、双唤醒、双 TTS），后台/锁屏时接管。 */
public final class AppVisibility {
    private static volatile boolean foreground = false;

    private AppVisibility() {}

    public static void setForeground(boolean fg) {
        foreground = fg;
    }

    public static boolean isForeground() {
        return foreground;
    }
}

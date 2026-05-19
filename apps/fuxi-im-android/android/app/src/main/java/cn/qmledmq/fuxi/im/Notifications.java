package cn.qmledmq.fuxi.im;

import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.content.Context;
import android.os.Build;

/** 通知渠道——FCM 后台消息靠 manifest 里 default_notification_channel_id
 *  指到这个渠道；渠道必须在收到推送前已存在，否则 FCM 退化到「杂项」渠道。 */
public final class Notifications {
    static final String CHANNEL_ID = "fuxi_im_default";

    private Notifications() {}

    public static void ensureChannel(Context ctx) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            return; // 渠道是 API 26+ 概念
        }
        NotificationManager nm = ctx.getSystemService(NotificationManager.class);
        if (nm == null || nm.getNotificationChannel(CHANNEL_ID) != null) {
            return;
        }
        NotificationChannel ch = new NotificationChannel(
                CHANNEL_ID, "玄女消息", NotificationManager.IMPORTANCE_HIGH);
        ch.setDescription("玄女在等你、任务完成等推送");
        nm.createNotificationChannel(ch);
    }
}

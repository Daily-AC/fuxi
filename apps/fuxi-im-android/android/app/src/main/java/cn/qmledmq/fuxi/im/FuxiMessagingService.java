package cn.qmledmq.fuxi.im;

import android.util.Log;

import com.google.firebase.messaging.FirebaseMessagingService;
import com.google.firebase.messaging.RemoteMessage;

/** 原生 FCM 服务：收 token 轮换 + 收消息。
 *
 *  后端发的是 notification 型消息——app 在后台/被杀时由系统托盘自动展示
 *  （图标/颜色/渠道走 manifest 的 default_notification_* meta），点击拉起
 *  MainActivity 并把 data.url 带进 intent extras。所以这里 onMessageReceived
 *  只会在「前台」被调到，此时用户正看着 PWA，不重复打扰。 */
public class FuxiMessagingService extends FirebaseMessagingService {
    private static final String TAG = "FuxiMessaging";

    @Override
    public void onNewToken(String token) {
        super.onNewToken(token);
        FcmRegistrar.clearCache(this);
        FcmRegistrar.register(this);
    }

    @Override
    public void onMessageReceived(RemoteMessage message) {
        super.onMessageReceived(message);
        Notifications.ensureChannel(this);
        Log.i(TAG, "前台收到推送，交由 PWA 实时呈现");
    }
}

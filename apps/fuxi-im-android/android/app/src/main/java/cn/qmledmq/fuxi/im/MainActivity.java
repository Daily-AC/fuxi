package cn.qmledmq.fuxi.im;

import android.Manifest;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Build;
import android.os.Bundle;

import androidx.core.app.ActivityCompat;

import com.getcapacitor.BridgeActivity;

/** Capacitor 壳：WebView 加载远端 PWA（server.url），外加原生 FCM。
 *
 *  - onCreate：建通知渠道 + 请求 Android 13+ 通知权限。
 *  - onResume：尝试上报 FCM token（cookie 就绪即成功，否则下次重试）。
 *  - 通知点击：FCM data.url 进 intent extras → 切 PWA hash 路由。 */
public class MainActivity extends BridgeActivity {
    private static final int REQ_NOTIF_PERM = 9001;

    /** 待跳转的 PWA 路径（来自被点击的推送）；导航完成即清空。 */
    private String pendingUrl;

    @Override
    public void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Notifications.ensureChannel(this);
        requestNotificationPermissionIfNeeded();
        pendingUrl = extractUrl(getIntent());
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        pendingUrl = extractUrl(intent);
        navigatePending();
    }

    @Override
    public void onResume() {
        super.onResume();
        FcmRegistrar.register(this);
        navigatePending();
    }

    /** 把待跳转路径切到 PWA 的 hash 路由。冷启动时 server.url 还在加载，
     *  延迟一拍再切；切不中也只是停在主屏，app 仍正常打开。 */
    private void navigatePending() {
        if (pendingUrl == null || pendingUrl.isEmpty()) {
            return;
        }
        final String url = pendingUrl;
        pendingUrl = null;
        getWindow().getDecorView().postDelayed(() -> {
            if (getBridge() == null || getBridge().getWebView() == null) {
                return;
            }
            int hashAt = url.indexOf('#');
            String hash = hashAt >= 0 ? url.substring(hashAt) : "";
            if (hash.isEmpty()) {
                return;
            }
            String js = "window.location.hash=" + jsString(hash) + ";";
            getBridge().getWebView().evaluateJavascript(js, null);
        }, 1200);
    }

    private static String extractUrl(Intent intent) {
        if (intent == null || intent.getExtras() == null) {
            return null;
        }
        Object u = intent.getExtras().get("url");
        return u == null ? null : u.toString();
    }

    private static String jsString(String s) {
        return "'" + s.replace("\\", "\\\\").replace("'", "\\'") + "'";
    }

    private void requestNotificationPermissionIfNeeded() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            return; // Android 13 以下通知无需运行时授权
        }
        if (checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS)
                != PackageManager.PERMISSION_GRANTED) {
            ActivityCompat.requestPermissions(this,
                    new String[]{Manifest.permission.POST_NOTIFICATIONS}, REQ_NOTIF_PERM);
        }
    }
}

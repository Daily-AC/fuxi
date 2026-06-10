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
    private static final int REQ_STARTUP_PERMS = 9001;

    /** 待跳转的 PWA 路径（来自被点击的推送）；导航完成即清空。 */
    private String pendingUrl;

    @Override
    public void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        Notifications.ensureChannel(this);
        requestStartupPermissionsIfNeeded();
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
        startVoiceLoopIfPermitted();
        navigatePending();
    }

    @Override
    public void onRequestPermissionsResult(int requestCode, String[] permissions, int[] grantResults) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults);
        // 首启授权框点完立刻拉起常听，不用等下次 onResume
        startVoiceLoopIfPermitted();
    }

    /** mic 权限就绪才起后台常听服务（服务内 AudioRecord 需要它）。 */
    private void startVoiceLoopIfPermitted() {
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO)
                == PackageManager.PERMISSION_GRANTED) {
            cn.qmledmq.fuxi.im.voice.VoiceLoopService.startIfEnabled(this);
        }
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

    /** 首启一次性请求全部运行时权限：通知（Android 13+）+ 麦克风（PWA 语音）。
     *  一起弹而不是等用户点语音开关再弹——用户装好 app 第一件事就把权限给齐，
     *  之后语音模式 / 推送都零摩擦。已授过的不重复弹（requestPermissions 只收
     *  未授权项）。用户拒绝也不阻塞——对应功能各自降级（无通知 / 语音开关报错）。 */
    private void requestStartupPermissionsIfNeeded() {
        java.util.ArrayList<String> wanted = new java.util.ArrayList<>();
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
                && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS)
                        != PackageManager.PERMISSION_GRANTED) {
            wanted.add(Manifest.permission.POST_NOTIFICATIONS);
        }
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO)
                != PackageManager.PERMISSION_GRANTED) {
            wanted.add(Manifest.permission.RECORD_AUDIO);
        }
        if (!wanted.isEmpty()) {
            ActivityCompat.requestPermissions(this,
                    wanted.toArray(new String[0]), REQ_STARTUP_PERMS);
        }
    }
}

package cn.qmledmq.fuxi.im;

import android.content.Context;
import android.content.SharedPreferences;
import android.os.Build;
import android.util.Log;
import android.webkit.CookieManager;

import com.google.firebase.messaging.FirebaseMessaging;

import org.json.JSONObject;

import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;

/** 把本机 FCM token 上报给 fuxi-im 后端。
 *
 *  WHY 走原生 HTTP 而不是 PWA 里 fetch：WebView 加载的是远端 PWA，不含
 *  Capacitor JS，没有桥可调。注册纯原生完成。
 *  WHY cookie 从 CookieManager 取：后端 /api/* 走 cookie 鉴权，登录态在
 *  WebView 的 CookieManager 里——原生层能读到（含 HttpOnly）。未登录时
 *  cookie 为空，跳过，等下次 onResume 重试。 */
public final class FcmRegistrar {
    private static final String TAG = "FcmRegistrar";
    static final String BASE = "https://im.qmledmq.cn:8443";
    private static final String REGISTER_URL = BASE + "/api/push/fcm-register";
    private static final String PREFS = "fuxi_fcm";
    private static final String KEY_LAST_TOKEN = "last_registered_token";

    private FcmRegistrar() {}

    public static void register(Context ctx) {
        final Context app = ctx.getApplicationContext();
        FirebaseMessaging.getInstance().getToken()
                .addOnSuccessListener(token -> {
                    if (token == null || token.isEmpty()) {
                        return;
                    }
                    new Thread(() -> postToken(app, token)).start();
                })
                .addOnFailureListener(e -> Log.w(TAG, "取 FCM token 失败", e));
    }

    /** token 轮换时清掉缓存，强制下次 register 真正上报。 */
    public static void clearCache(Context ctx) {
        ctx.getApplicationContext()
                .getSharedPreferences(PREFS, Context.MODE_PRIVATE)
                .edit().remove(KEY_LAST_TOKEN).apply();
    }

    private static void postToken(Context app, String token) {
        SharedPreferences prefs = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        String cookie = CookieManager.getInstance().getCookie(BASE);
        if (cookie == null || cookie.isEmpty()) {
            Log.i(TAG, "WebView 无 cookie（未登录），跳过 FCM 注册");
            return;
        }
        if (token.equals(prefs.getString(KEY_LAST_TOKEN, null))) {
            return; // 同 token 已成功注册过
        }
        HttpURLConnection conn = null;
        try {
            JSONObject body = new JSONObject();
            body.put("token", token);
            body.put("device_name", "Android · " + Build.MODEL);
            byte[] payload = body.toString().getBytes(StandardCharsets.UTF_8);

            conn = (HttpURLConnection) new URL(REGISTER_URL).openConnection();
            conn.setRequestMethod("POST");
            conn.setDoOutput(true);
            conn.setConnectTimeout(10000);
            conn.setReadTimeout(10000);
            conn.setRequestProperty("Content-Type", "application/json");
            conn.setRequestProperty("Cookie", cookie);
            try (OutputStream os = conn.getOutputStream()) {
                os.write(payload);
            }
            int code = conn.getResponseCode();
            if (code >= 200 && code < 300) {
                prefs.edit().putString(KEY_LAST_TOKEN, token).apply();
                Log.i(TAG, "FCM token 注册成功");
            } else {
                // 401 = cookie 失效/未登录——不缓存，下次重试。
                Log.w(TAG, "FCM token 注册失败 HTTP " + code);
            }
        } catch (Exception e) {
            Log.w(TAG, "FCM token 注册异常", e);
        } finally {
            if (conn != null) {
                conn.disconnect();
            }
        }
    }
}

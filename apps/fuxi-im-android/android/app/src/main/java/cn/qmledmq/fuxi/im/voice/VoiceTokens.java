package cn.qmledmq.fuxi.im.voice;

import android.util.Log;
import android.webkit.CookieManager;

import org.json.JSONObject;

import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;

import cn.qmledmq.fuxi.im.FcmRegistrar;

/** GET /api/voice/tokens —— 用 WebView 登录 cookie 换语音三件套 token。
 *
 *  同 {@link FcmRegistrar} 的模式：cookie 从 CookieManager 取（原生层能读
 *  HttpOnly），未登录时返 null，调用方稍后重试。
 *
 *  im_token：intervene/asr/tts 共用的 HMAC token（Bearer / WS 帧体）。
 *  wake_token：wake server 预共享 token（WS query）；home 没部署唤醒服务
 *  时为 null——常听没法做，service 直接收摊。 */
public final class VoiceTokens {
    private static final String TAG = "VoiceTokens";
    private static final String URL_TOKENS = FcmRegistrar.BASE + "/api/voice/tokens";

    public final String imToken;
    /** 可空——home 未部署 wake server。 */
    public final String wakeToken;

    private VoiceTokens(String imToken, String wakeToken) {
        this.imToken = imToken;
        this.wakeToken = wakeToken;
    }

    /** 阻塞拉取；未登录 / 网络失败返 null（调用方退避重试）。 */
    public static VoiceTokens fetch() {
        String cookie = CookieManager.getInstance().getCookie(FcmRegistrar.BASE);
        if (cookie == null || cookie.isEmpty()) {
            Log.i(TAG, "WebView 无 cookie（未登录），暂不取语音 token");
            return null;
        }
        HttpURLConnection conn = null;
        try {
            conn = (HttpURLConnection) new URL(URL_TOKENS).openConnection();
            conn.setRequestMethod("GET");
            conn.setConnectTimeout(10000);
            conn.setReadTimeout(10000);
            conn.setRequestProperty("Cookie", cookie);
            int code = conn.getResponseCode();
            if (code < 200 || code >= 300) {
                Log.w(TAG, "voice/tokens HTTP " + code);
                return null;
            }
            try (InputStream is = conn.getInputStream()) {
                byte[] buf = readAll(is);
                JSONObject j = new JSONObject(new String(buf, StandardCharsets.UTF_8));
                String im = j.optString("im_token", "");
                String wake = j.isNull("wake_token") ? null : j.optString("wake_token", null);
                if (im.isEmpty()) {
                    Log.w(TAG, "voice/tokens 响应缺 im_token");
                    return null;
                }
                return new VoiceTokens(im, wake);
            }
        } catch (Exception e) {
            Log.w(TAG, "voice/tokens 拉取异常", e);
            return null;
        } finally {
            if (conn != null) {
                conn.disconnect();
            }
        }
    }

    private static byte[] readAll(InputStream is) throws java.io.IOException {
        java.io.ByteArrayOutputStream bos = new java.io.ByteArrayOutputStream();
        byte[] tmp = new byte[4096];
        int n;
        while ((n = is.read(tmp)) > 0) {
            bos.write(tmp, 0, n);
        }
        return bos.toByteArray();
    }
}

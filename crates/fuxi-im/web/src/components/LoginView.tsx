import { Show, createSignal, type Component, onMount } from "solid-js";
import { ApiError } from "~/lib/api";
import { Mascot } from "~/components/Mascot/Mascot";
import { useApi } from "./ApiProvider";
import styles from "./LoginView.module.css";

// 鉴权 UI · 主路 = 主密码 · 副路 = PIN（"忘了 / 没设过"展开）
// daimeng 奶油糖果重绘：全屏 u-mesh u-noise 暖背景 + 玄女吉祥物 wave 招呼 +
// 软圆角输入卡 + 桃色渐变提交药丸。所有 login testid / 鉴权流程不变。

function defaultDeviceName(): string {
  if (typeof navigator === "undefined") return "我的设备";
  const ua = navigator.userAgent;
  if (/iPhone/i.test(ua)) return "我的 iPhone";
  if (/iPad/i.test(ua)) return "我的 iPad";
  if (/Android/i.test(ua)) return "我的 Android";
  if (/Mac OS X/i.test(ua)) return "我的 Mac";
  if (/Windows/i.test(ua)) return "我的 Windows";
  if (/Linux/i.test(ua)) return "我的 Linux";
  return "我的设备";
}

export type LoginMode = "password" | "pin";

export interface LoginViewProps {
  /** 成功回调（拿到 device_id 后由父级切到主 shell）。*/
  onSuccess(deviceId: string): void;
}

export const LoginView: Component<LoginViewProps> = (props) => {
  const { client } = useApi();
  const [mode, setMode] = createSignal<LoginMode>("password");
  const [password, setPassword] = createSignal("");
  const [pin, setPin] = createSignal("");
  const [deviceName, setDeviceName] = createSignal("我的设备");
  const [submitting, setSubmitting] = createSignal(false);
  const [errMsg, setErrMsg] = createSignal<string | null>(null);

  onMount(() => setDeviceName(defaultDeviceName()));

  const errorFor = (status: number, fallback: string): string => {
    if (status === 401) {
      return mode() === "password" ? "密码错。" : "PIN 不可用（错了或过期）。";
    }
    if (status === 503) return "服务端没设主密码。请 ssh 到 home 跑 `fuxi im set-password`。";
    if (status === 429) return "尝试过多，5 分钟后再试。";
    if (status === 400) return "格式不对。PIN 必须是 6 位数字。";
    return fallback;
  };

  const submit = async (e?: Event): Promise<void> => {
    e?.preventDefault();
    if (submitting()) return;
    const dn = deviceName().trim() || defaultDeviceName();
    setErrMsg(null);
    setSubmitting(true);
    try {
      let result: { device_id: string };
      if (mode() === "password") {
        result = await client.login({ password: password(), device_name: dn });
      } else {
        result = await client.pair({ pin: pin(), device_name: dn });
      }
      props.onSuccess(result.device_id);
    } catch (err) {
      if (err instanceof ApiError) setErrMsg(errorFor(err.status, err.message));
      else setErrMsg(err instanceof Error ? err.message : "登入失败");
    } finally {
      setSubmitting(false);
    }
  };

  const canSubmit = (): boolean => {
    if (submitting()) return false;
    if (mode() === "password") return password().length > 0;
    return /^\d{6}$/.test(pin());
  };

  return (
    <main
      class={`u-mesh u-noise ${styles.shell}`}
      data-testid="login-view"
      data-mode={mode()}
    >
      <div class={styles.greeting} aria-hidden="true">
        <Mascot state="wave" size={130} />
      </div>
      <form class={`u-card ${styles.card}`} onSubmit={submit} aria-labelledby="login-title">
        <header class={styles.header}>
          <h1 id="login-title" class={styles.title}>
            你回来啦～
          </h1>
          <p class={styles.subtitle}>
            {mode() === "password" ? "用主密码登入。" : "用 TUI 派发的 PIN 配对。"}
          </p>
        </header>

        <Show when={mode() === "password"}>
          <label class={styles.field}>
            <span class={styles.label}>主密码</span>
            <input
              class={styles.input}
              type="password"
              autocomplete="current-password"
              spellcheck={false}
              data-testid="login-password"
              value={password()}
              onInput={(e) => setPassword(e.currentTarget.value)}
              placeholder="在 home 跑 `fuxi im set-password` 设过的密码"
              disabled={submitting()}
            />
          </label>
        </Show>

        <Show when={mode() === "pin"}>
          <label class={styles.field}>
            <span class={styles.label}>6 位 PIN</span>
            <input
              class={styles.input}
              type="text"
              inputmode="numeric"
              pattern="[0-9]{6}"
              autocomplete="one-time-code"
              maxlength={6}
              data-testid="login-pin"
              value={pin()}
              onInput={(e) => setPin(e.currentTarget.value.replace(/\D/g, "").slice(0, 6))}
              placeholder="例如 482917"
              disabled={submitting()}
            />
            <span class={styles.hint}>
              在 fuxi TUI 里跑 <code>/pair</code> 拿 PIN（5 分钟有效）。
            </span>
          </label>
        </Show>

        <label class={styles.field}>
          <span class={styles.label}>设备名</span>
          <input
            class={styles.input}
            type="text"
            data-testid="login-device-name"
            value={deviceName()}
            onInput={(e) => setDeviceName(e.currentTarget.value)}
            placeholder="例如 我的 iPhone"
            disabled={submitting()}
          />
        </label>

        <Show when={errMsg()}>
          <div class={styles.error} role="alert" data-testid="login-error">
            {errMsg()}
          </div>
        </Show>

        <button
          type="submit"
          class={styles.submit}
          classList={{ [styles.submitActive ?? ""]: canSubmit() }}
          disabled={!canSubmit()}
          data-testid="login-submit"
          aria-busy={submitting()}
        >
          {submitting() ? "登入中…" : mode() === "password" ? "登入" : "配对"}
        </button>

        <div class={styles.divider} aria-hidden="true" />

        <button
          type="button"
          class={styles.modeSwitch}
          data-testid="login-mode-switch"
          onClick={() => {
            setErrMsg(null);
            setMode(mode() === "password" ? "pin" : "password");
          }}
          disabled={submitting()}
        >
          {mode() === "password" ? "我没设过密码 / 忘了？" : "回到主密码登入"}
        </button>

        <Show when={mode() === "pin"}>
          <p class={styles.fallbackHelp}>
            在 home 跑 <code>fuxi im set-password</code> 设主密码后即可走主路。
          </p>
        </Show>
      </form>
    </main>
  );
};

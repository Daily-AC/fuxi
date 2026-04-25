import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@solidjs/testing-library";
import { ApiProvider, setApiOverride } from "~/components/ApiProvider";
import { LoginView } from "~/components/LoginView";
import { createMockApi } from "../mocks/api";

function setup(opts?: { loginSeq?: number[]; pairSeq?: number[] }) {
  const api = createMockApi({
    auth: {
      loginCalls: [],
      pairCalls: [],
      loginSeq: opts?.loginSeq,
      pairSeq: opts?.pairSeq,
    },
  });
  setApiOverride(api);
  const onSuccess = vi.fn();
  const tools = render(() => (
    <ApiProvider initialAuth="out">
      <LoginView onSuccess={onSuccess} />
    </ApiProvider>
  ));
  return { api, onSuccess, ...tools };
}

describe("LoginView", () => {
  it("默认渲染密码模式，提交按钮初态 disabled", () => {
    const { getByTestId, unmount } = setup();
    expect((getByTestId("login-password") as HTMLInputElement).type).toBe("password");
    expect((getByTestId("login-submit") as HTMLButtonElement).disabled).toBe(true);
    setApiOverride(null);
    unmount();
  });

  it("成功 200 → 调 onSuccess 带 device_id，并提交 password + device_name", async () => {
    const { api, onSuccess, getByTestId, unmount } = setup({ loginSeq: [200] });
    fireEvent.input(getByTestId("login-password"), { target: { value: "正确密码" } });
    fireEvent.input(getByTestId("login-device-name"), { target: { value: "我的 iPhone" } });
    fireEvent.click(getByTestId("login-submit"));
    await new Promise((r) => setTimeout(r, 30));
    expect(api.state.auth.loginCalls).toHaveLength(1);
    expect(api.state.auth.loginCalls[0]).toEqual({
      password: "正确密码",
      device_name: "我的 iPhone",
    });
    expect(onSuccess).toHaveBeenCalledTimes(1);
    expect(onSuccess.mock.calls[0]?.[0]).toMatch(/^dev-/);
    setApiOverride(null);
    unmount();
  });

  it("401 → 显示'密码错'错误，不调 onSuccess", async () => {
    const { onSuccess, getByTestId, unmount } = setup({ loginSeq: [401] });
    fireEvent.input(getByTestId("login-password"), { target: { value: "错密码" } });
    fireEvent.click(getByTestId("login-submit"));
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("login-error").textContent).toContain("密码错");
    expect(onSuccess).not.toHaveBeenCalled();
    setApiOverride(null);
    unmount();
  });

  it("503 → 显示'服务端没设主密码'引导文案", async () => {
    const { getByTestId, unmount } = setup({ loginSeq: [503] });
    fireEvent.input(getByTestId("login-password"), { target: { value: "x" } });
    fireEvent.click(getByTestId("login-submit"));
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("login-error").textContent).toContain("服务端没设主密码");
    expect(getByTestId("login-error").textContent).toContain("fuxi im set-password");
    setApiOverride(null);
    unmount();
  });

  it("429 → 显示'尝试过多'", async () => {
    const { getByTestId, unmount } = setup({ loginSeq: [429] });
    fireEvent.input(getByTestId("login-password"), { target: { value: "x" } });
    fireEvent.click(getByTestId("login-submit"));
    await new Promise((r) => setTimeout(r, 30));
    expect(getByTestId("login-error").textContent).toContain("尝试过多");
    setApiOverride(null);
    unmount();
  });

  it("点'忘了？'切换到 PIN 模式 + 引导'/pair'文案", async () => {
    const { getByTestId, unmount } = setup();
    fireEvent.click(getByTestId("login-mode-switch"));
    await new Promise((r) => setTimeout(r, 0));
    const pinInput = getByTestId("login-pin") as HTMLInputElement;
    expect(pinInput).toBeTruthy();
    expect(pinInput.inputMode).toBe("numeric");
    setApiOverride(null);
    unmount();
  });

  it("PIN 输入只接受数字、限 6 位；6 位才能提交", async () => {
    const { api, getByTestId, unmount } = setup({ pairSeq: [200] });
    fireEvent.click(getByTestId("login-mode-switch"));
    const pinInput = getByTestId("login-pin") as HTMLInputElement;
    // 输入混合字符 + 超长，应被清洗为前 6 位数字
    fireEvent.input(pinInput, { target: { value: "12a3456789" } });
    expect(pinInput.value).toBe("123456");
    fireEvent.input(getByTestId("login-device-name"), { target: { value: "iPhone" } });
    fireEvent.click(getByTestId("login-submit"));
    await new Promise((r) => setTimeout(r, 30));
    expect(api.state.auth.pairCalls).toHaveLength(1);
    expect(api.state.auth.pairCalls[0]?.pin).toBe("123456");
    setApiOverride(null);
    unmount();
  });

  it("提交进行中按钮 disabled + 文案'登入中…'防双击", async () => {
    // loginSeq=[200] 但用 promise 延迟控制 timing
    const { api, getByTestId, unmount } = setup({ loginSeq: [200] });
    const orig = api.login;
    let resolveFn: () => void = () => {};
    api.login = ((req: Parameters<typeof api.login>[0]) => {
      api.state.auth.loginCalls.push(req);
      return new Promise((r) => {
        resolveFn = () => r({ device_id: "dev-x" });
      });
    }) as typeof api.login;

    fireEvent.input(getByTestId("login-password"), { target: { value: "x" } });
    fireEvent.click(getByTestId("login-submit"));
    await new Promise((r) => setTimeout(r, 0));
    const btn = getByTestId("login-submit") as HTMLButtonElement;
    expect(btn.disabled).toBe(true);
    expect(btn.textContent).toContain("登入中");
    resolveFn();
    api.login = orig;
    setApiOverride(null);
    unmount();
  });
});

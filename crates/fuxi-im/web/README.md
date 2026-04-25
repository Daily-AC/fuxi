# fuxi-im web · Solid PWA

玄女在你口袋里。三视图 hash router：

- `#/` 任务卡片网格
- `#/conv` 跟玄女顶层对话
- `#/task/:id` 单任务 chat（事件流 + 工具调用折叠）

视觉规则源头：`./.impeccable.md` + `/Users/e0_7/fuxi/docs/decisions/14-im-mobile-frontend.md` §G + memory `feedback_pwa_modern_not_tui`。

## 开发

```bash
pnpm install
pnpm dev          # vite dev，proxy /api → 127.0.0.1:9100
pnpm typecheck
pnpm lint
pnpm test         # vitest unit
pnpm e2e          # playwright（首次需 pnpm e2e:install）
pnpm build        # 输出到 dist/，给 fuxi-im axum 的 include_dir!() 吃
```

## 视觉硬规则（违反任何一条 = 任务失败）

- 不抄 WeChat / Slack / iMessage
- 不要 TUI 等宽 + Unicode block 装饰搬过来
- 默认不用 emoji
- 不要 shadow / gradient / glassmorphism
- 暗底（`#0a0a0a` 系）+ 中文 sans-serif（PingFang/系统字 16–17px）
- 等宽**只用于** code block / 工具输出 / agent id
- 圆角但纯平的卡片
- 触控热区 ≥ 44px
- 字符级流式动画

## 后端契约

见决策 14 §C。当前后端 α 已起，γ 在做 WS。开发期 mock fallback 在 `tests/mocks/api.ts`。

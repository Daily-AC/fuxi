# jarvis-pet

> Tauri 2 桌宠 app。语音唤醒 + ASR + intervene + say + TTS 派蒙音色全链路（Phase 2 ship 2026-05-11）。

## 状态
- 当前阶段：Phase 3 emotion handoff（接班中）
- 主仓库：（在 fuxi monorepo 里 / 单独 repo？用户补）
- 最近大事件：2026-05-11 Phase 2 语音闭环 ship

## 部署
- 客户端：mac `~/Applications/XuannvPet.app`（Tauri 2 release）
- 后端：fuxi-im 接 intervene 入口；GPT-SoVITS 自托管 TTS
- 用到的 service：[[caddy]]（如远程访问） + GPT-SoVITS（独立 service，未单列）

## 入口
- 用户访问：mac app 直接打开
- 开发：`cd ~/fuxi/crates/<jarvis-pet>` 或单独 repo（用户补）

## 关键路径
- 萝莉斯透明 + 拖动 + 右键菜单（Phase 1）
- 唤醒+ASR+intervene+say+TTS（Phase 2）
- 情绪映射 3-5 派蒙情绪 ref + sovits emotion 路由 + xuannv say --emotion + sprite mode 切（Phase 3 接班中）

## 依赖
- fuxi（intervene 入口）
- GPT-SoVITS（音色合成）
- 讯飞唤醒
- WeType / Whisper（ASR）

## 已知 issue / 待办
- Phase 3 情绪映射接班中（见 memory `project_jarvis_pet_v0.4_phase3_emotion_handoff`）

## 引用
- handoff：[../../handoff/](../../handoff/)（搜 jarvis）
- memory：`project_jarvis_pet_*`（多份）

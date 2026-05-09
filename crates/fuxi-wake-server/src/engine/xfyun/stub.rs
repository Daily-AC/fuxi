//! 非 Linux x86_64 平台 stub——所有讯飞调用返 `unimplemented!`-style 错误。
//!
//! 走 mac dev 时这条路径让 `cargo build` / `cargo check` 通过；真要跑唤醒只能 home Linux。

use anyhow::{Result, anyhow};

use super::ProcessInitParams;

pub fn init_process(_params: ProcessInitParams) -> Result<()> {
    Err(anyhow!(
        "xfyun engine 未编译进 FFI——本平台不是 Linux x86_64 或 SDK 目录缺失。\
         请在 home Linux 上构建并部署，或本机加 --mock 跑 mock 引擎"
    ))
}

pub fn session_unimplemented<T>() -> Result<T> {
    Err(anyhow!(
        "xfyun engine 未编译进 FFI（非 Linux x86_64 / SDK 缺）"
    ))
}

pub struct Stub;

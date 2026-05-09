//! bindgen 生成的讯飞 C API binding——`#[cfg(xfyun_ffi)]` 守。
//!
//! 该模块只 include build.rs 输出的原始 binding。Rust 侧的安全封装走 `linux.rs`。

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::all)]

include!(concat!(env!("OUT_DIR"), "/aikit_bindings.rs"));

//! build.rs —— 仅 Linux x86_64 上跑 bindgen + 链接 `libaikit.so`。
//!
//! mac dev：`cargo build` 全量跳过（不 link、不 bindgen），让 stub 实现编译过；
//! Linux 部署：用 ENV `FUXI_XFYUN_SDK_DIR`（默认 `/Users/e0_7/fuxi/Linux_ivw_e867a88f2_v1.0.11_v2.2.15-rc5`）
//! 找头文件 + .so，bindgen 输出落 `OUT_DIR/aikit_bindings.rs`。
//!
//! 决策：bindgen 只在"Linux x86_64 + SDK 目录存在 + 头文件可读"三条件全满足时跑——
//! 否则视为 stub 路径（不输出 bindings 文件，src/engine/xfyun/linux.rs 也不会被
//! `cfg(xfyun_ffi)` 纳入编译）。
//!
//! 如要在 Linux x86_64 上**显式跳过 FFI**（比如 CI 没装 SDK 也想编 stub），
//! ENV `FUXI_XFYUN_SKIP_FFI=1` 可以强制走 stub 路径。

use std::env;
use std::path::PathBuf;

const DEFAULT_SDK_DIR: &str = "/Users/e0_7/fuxi/Linux_ivw_e867a88f2_v1.0.11_v2.2.15-rc5";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=FUXI_XFYUN_SDK_DIR");
    println!("cargo:rerun-if-env-changed=FUXI_XFYUN_SKIP_FFI");

    // 始终声明自定义 cfg——edition 2024 / clippy `-D warnings` 否则会触发
    // `unexpected_cfgs` lint。
    println!("cargo:rustc-check-cfg=cfg(xfyun_ffi)");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let skip = env::var("FUXI_XFYUN_SKIP_FFI").ok().as_deref() == Some("1");

    if target_os != "linux" || target_arch != "x86_64" || skip {
        // stub 路径——不 link 不 bindgen，xfyun/linux.rs 也不进编译
        println!(
            "cargo:warning=fuxi-wake-server: 跳过讯飞 FFI（target={target_os}/{target_arch}, skip={skip}）—走 stub"
        );
        return;
    }

    let sdk_dir =
        PathBuf::from(env::var("FUXI_XFYUN_SDK_DIR").unwrap_or_else(|_| DEFAULT_SDK_DIR.into()));
    let include_dir = sdk_dir.join("include");
    let libs_dir = sdk_dir.join("libs");
    let header = include_dir.join("aikit_biz_api_c.h");

    if !header.exists() || !libs_dir.exists() {
        println!(
            "cargo:warning=fuxi-wake-server: SDK 目录 {} 缺头文件 / libs（{}, {}）—走 stub",
            sdk_dir.display(),
            header.display(),
            libs_dir.display()
        );
        return;
    }

    println!("cargo:rerun-if-changed={}", header.display());
    println!("cargo:rustc-link-search=native={}", libs_dir.display());
    println!("cargo:rustc-link-lib=dylib=aikit");
    // FFI 入口 + linux.rs 模块的编译开关——通过 cfg(xfyun_ffi) 守。
    println!("cargo:rustc-cfg=xfyun_ffi");

    let bindings = bindgen::Builder::default()
        .header(header.to_string_lossy().to_string())
        .clang_arg(format!("-I{}", include_dir.display()))
        // 只保留我们要用的符号；过滤掉 deprecated C++ helpers 进入 binding。
        .allowlist_function("AIKIT_.*")
        .allowlist_function("AIKITBuilder_.*")
        .allowlist_type("AIKIT_.*")
        .allowlist_type("BuilderType_?")
        .allowlist_type("BuilderDataType_?")
        .allowlist_type("BuilderData_?")
        .allowlist_var("AIKIT_DATA_PTR_.*")
        // Linux x86_64：stdint.h 在 /usr/include 里 - 让 clang 自己找
        .layout_tests(false)
        .derive_default(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("bindgen aikit_biz_api_c.h 失败");

    let out_path = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("aikit_bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("写 aikit_bindings.rs 失败");
    println!(
        "cargo:warning=fuxi-wake-server: bindgen 完成，bindings -> {}",
        out_path.display()
    );
}

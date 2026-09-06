// SPDX-License-Identifier: LGPL-2.1-or-later
// Copyright (C) 2026 yearsyan and contributors

// Linux: bake a transitive DT_RPATH ($ORIGIN) into libgame_stream.so so the
// FFmpeg shared libraries shipped next to the extension resolve without
// LD_LIBRARY_PATH. DT_RPATH (not DT_RUNPATH) is required because libavcodec's
// own dependencies (e.g. libswresample) must also resolve through it.
// macOS: same idea with @loader_path for FFmpeg dylibs next to the extension.
fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "linux" {
        println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
    } else if target_os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@loader_path");
    }
}

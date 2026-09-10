//! Emits the linker instructions for the statically built media libraries.
//!
//! Called from the build scripts of every package that produces binaries
//! linking `geneva-media`. The binding crate links the core libraries from
//! the prefix; the codec libraries they were built against, and the C++
//! runtime two of them need, are read from the prefix's pkg-config files
//! here and passed to the linker as one group so archive order does not
//! matter.

#![forbid(unsafe_code)]

use std::process::Command;

/// Prints the `cargo:` directives that link the media libraries.
///
/// Reads the prefix from `MEDIA_LIBS_DIR` or `FFMPEG_DIR` (set for the
/// workspace by `.cargo/config.toml`) and panics with instructions if the
/// libraries have not been built.
pub fn link_media_libraries() {
    println!("cargo:rerun-if-env-changed=MEDIA_LIBS_DIR");
    println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
    let prefix = std::env::var("MEDIA_LIBS_DIR")
        .or_else(|_| std::env::var("FFMPEG_DIR"))
        .unwrap_or_else(|_| {
            panic!(
                "the `media` feature needs the media libraries built by \
                 scripts/build-media-libs.sh (set MEDIA_LIBS_DIR to another prefix)"
            )
        });
    assert!(
        std::path::Path::new(&prefix)
            .join("lib/pkgconfig/libavcodec.pc")
            .exists(),
        "no media libraries under {prefix}; run scripts/build-media-libs.sh first"
    );
    let pkgconfig_dir = format!("{prefix}/lib/pkgconfig");
    let query = |flag: &str| -> Vec<String> {
        let out = Command::new("pkg-config")
            .env("PKG_CONFIG_PATH", &pkgconfig_dir)
            .env("PKG_CONFIG_LIBDIR", &pkgconfig_dir)
            .args([
                "--static",
                flag,
                "libavformat",
                "libavcodec",
                "libswscale",
                "libswresample",
                "libavutil",
            ])
            .output()
            .unwrap_or_else(|e| panic!("running pkg-config: {e}"));
        assert!(
            out.status.success(),
            "pkg-config could not describe the media libraries under {prefix}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    };
    println!("cargo:rustc-link-search=native={prefix}/lib");
    for dir in query("--libs-only-L") {
        println!(
            "cargo:rustc-link-search=native={}",
            dir.trim_start_matches("-L")
        );
    }
    // Archives from the prefix are passed by path so that a same-named
    // system library can never be picked up instead; anything else (libm,
    // libdl, the C++ runtime) stays a plain `-l` flag.
    let mut libs: Vec<String> = Vec::new();
    for l in query("--libs-only-l") {
        let name = l.trim_start_matches("-l");
        let archive = std::path::Path::new(&prefix)
            .join("lib")
            .join(format!("lib{name}.a"));
        let arg = if archive.exists() {
            archive.display().to_string()
        } else {
            l.clone()
        };
        if !libs.contains(&arg) {
            libs.push(arg);
        }
    }
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // Static archives must be resolvable in any order; GNU ld needs a group
    // for that, Apple's linker does not.
    let grouped = target_os == "linux";
    if grouped {
        println!("cargo:rustc-link-arg=-Wl,--start-group");
    }
    for l in &libs {
        println!("cargo:rustc-link-arg={l}");
    }
    if grouped {
        println!("cargo:rustc-link-arg=-Wl,--end-group");
    }
    // The H.264 and AV1 encoders are C++.
    match target_os.as_str() {
        "macos" | "ios" => println!("cargo:rustc-link-arg=-lc++"),
        "linux" => println!("cargo:rustc-link-arg=-lstdc++"),
        _ => {}
    }
}

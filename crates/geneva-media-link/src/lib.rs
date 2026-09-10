//! Emits the linker instructions for the statically built media libraries.
//!
//! Called from the build scripts of every package that produces binaries
//! linking `geneva-media`. The binding crate links the core libraries from
//! the prefix; the codec libraries they were built against, and the C++
//! runtime two of them need, are read from the prefix's pkg-config files
//! here and passed to the linker as one group so archive order does not
//! matter.

#![forbid(unsafe_code)]

use std::path::Path;
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
        Path::new(&prefix)
            .join("lib/pkgconfig/libavcodec.pc")
            .exists(),
        "no media libraries under {prefix}; run scripts/build-media-libs.sh first"
    );
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    println!("cargo:rustc-link-search=native={prefix}/lib");
    for dir in pkg_config(&prefix, "--libs-only-L") {
        println!(
            "cargo:rustc-link-search=native={}",
            dir.trim_start_matches("-L")
        );
    }

    // Archives from the prefix are passed by path so that a same-named
    // system library can never be picked up instead. The C++ runtime the
    // H.264 and AV1 encoders need is linked statically on Linux when its
    // archive can be found, so the binary depends on nothing but the C
    // library. Everything else (libm, libdl, ...) stays a plain `-l` flag.
    let cxx_runtime = if target_os == "linux" {
        static_cxx_runtime()
    } else {
        None
    };
    let mut libs: Vec<String> = Vec::new();
    for flag in pkg_config(&prefix, "--libs-only-l") {
        let name = flag.trim_start_matches("-l");
        let archive = Path::new(&prefix).join("lib").join(format!("lib{name}.a"));
        let arg = if archive.exists() {
            archive.display().to_string()
        } else if name == "stdc++" {
            match target_os.as_str() {
                // Apple's C++ runtime goes by another name.
                "macos" | "ios" => "-lc++".to_owned(),
                _ => cxx_runtime.clone().unwrap_or(flag),
            }
        } else {
            flag
        };
        if !libs.contains(&arg) {
            libs.push(arg);
        }
    }

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
    match target_os.as_str() {
        "macos" | "ios" => println!("cargo:rustc-link-arg=-lc++"),
        "linux" => {
            match &cxx_runtime {
                Some(path) => println!("cargo:rustc-link-arg={path}"),
                None => println!("cargo:rustc-link-arg=-lstdc++"),
            }
            // GCC emits calls to its out-of-line atomics helpers on
            // AArch64; they live in the static libgcc, which Rust does
            // not link on its own.
            let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
            if arch == "aarch64" {
                if let Some(libgcc) = static_libgcc() {
                    println!("cargo:rustc-link-arg={libgcc}");
                }
            }
        }
        _ => {}
    }
}

/// The path of the static libgcc archive, if the compiler knows it.
fn static_libgcc() -> Option<String> {
    let out = Command::new("cc")
        .arg("-print-libgcc-file-name")
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (Path::new(&path).is_absolute() && Path::new(&path).exists()).then_some(path)
}

/// Runs pkg-config against the prefix only, so nothing from the system
/// leaks in, and returns the whitespace-separated output.
fn pkg_config(prefix: &str, flag: &str) -> Vec<String> {
    let dir = format!("{prefix}/lib/pkgconfig");
    let out = Command::new("pkg-config")
        .env("PKG_CONFIG_PATH", &dir)
        .env("PKG_CONFIG_LIBDIR", &dir)
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
}

/// The path of the static C++ runtime archive, if the compiler knows it.
fn static_cxx_runtime() -> Option<String> {
    let out = Command::new("cc")
        .arg("-print-file-name=libstdc++.a")
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (Path::new(&path).is_absolute() && Path::new(&path).exists()).then_some(path)
}

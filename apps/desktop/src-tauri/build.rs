fn main() {
    link_clang_runtime();
    tauri_build::build();
}

/// Link clang's compiler runtime on macOS.
///
/// ggml's Metal backend is Objective-C and uses `@available` to gate the
/// macOS 15 residency-set APIs. That compiles to a call to
/// `__isPlatformVersionAtLeast`, which lives in `libclang_rt.osx.a`.
///
/// A plain `cargo build` links it implicitly and needs none of this. Passing an
/// explicit `--target` does not — which is exactly what `make bundle-mac` does
/// for the universal build, so without this the only build that fails is the
/// one that ships:
///
/// ```text
/// Undefined symbols for architecture arm64:
///   "___isPlatformVersionAtLeast", referenced from:
///       ...ggml-metal-device.m.o
/// ```
///
/// Best-effort: if the toolchain cannot be interrogated, say nothing and let
/// the link proceed. It succeeds without help in the common case, and a build
/// script has no business failing a build it might not need to.
fn link_clang_runtime() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    let clang = std::env::var("CC").unwrap_or_else(|_| "clang".to_string());
    let Ok(output) = std::process::Command::new(clang)
        .arg("-print-resource-dir")
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }

    let Ok(dir) = String::from_utf8(output.stdout) else {
        return;
    };
    let darwin = std::path::Path::new(dir.trim()).join("lib/darwin");
    if !darwin.join("libclang_rt.osx.a").is_file() {
        return;
    }

    println!("cargo:rustc-link-search=native={}", darwin.display());
    println!("cargo:rustc-link-lib=static=clang_rt.osx");
}

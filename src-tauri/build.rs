fn main() {
    #[cfg(target_os = "macos")]
    {
        swift_rs::SwiftLinker::new("14.0")
            .with_package("AxPlugin", "./plugins/ax/macos")
            .link();
        globalize_swift_rs_shim();
    }

    #[cfg(target_os = "macos")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");

    tauri_build::build()
}

/// Xcode 27's SwiftPM turns the non-public `@_cdecl` functions in swift-rs's
/// runtime shim (`SwiftRs.o`) into local symbols, and swift-rs only promotes
/// the package's own object back, so the link fails on `_retain_object`.
/// https://github.com/Brendonovich/swift-rs/issues/81
/// Globalizing a symbol that is already global changes nothing, so this is
/// harmless on older Xcode.
#[cfg(target_os = "macos")]
fn globalize_swift_rs_shim() {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn archives(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                archives(&path, found);
            } else if path.file_name().is_some_and(|n| n == "libAxPlugin.a") {
                found.push(path);
            }
        }
    }

    let objcopy = std::env::var("LLVM_OBJCOPY").ok().or_else(|| {
        ["llvm-objcopy", "/opt/homebrew/opt/llvm/bin/llvm-objcopy"]
            .into_iter()
            .find(|c| Command::new(c).arg("--version").output().is_ok())
            .map(String::from)
    });
    let Some(objcopy) = objcopy else {
        println!("cargo:warning=llvm-objcopy not found; set LLVM_OBJCOPY if linking fails on _retain_object");
        return;
    };

    let mut found = Vec::new();
    archives(
        &Path::new(&std::env::var("OUT_DIR").unwrap()).join("swift-rs/AxPlugin"),
        &mut found,
    );
    for archive in found {
        let status = Command::new(&objcopy)
            .args([
                "--globalize-symbol=_retain_object",
                "--globalize-symbol=_release_object",
                "--globalize-symbol=_string_from_bytes",
                "--globalize-symbol=_data_from_bytes",
            ])
            .arg(&archive)
            .status()
            .expect("run llvm-objcopy");
        assert!(
            status.success(),
            "llvm-objcopy failed on {}",
            archive.display()
        );
    }
}

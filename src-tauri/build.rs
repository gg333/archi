use sha2::{Digest, Sha256};
use std::{env, fs, path::Path};

fn verify_sidecars() {
    let manifest = include_str!("binaries/SHA256SUMS");
    let target = env::var("TARGET").expect("Cargo did not provide TARGET");
    let target_name = format!(
        "7zz-{target}{}",
        if target.contains("windows") {
            ".exe"
        } else {
            ""
        }
    );
    let mut target_verified = false;

    for line in manifest.lines().filter(|line| !line.trim().is_empty()) {
        let (expected, name) = line
            .split_once("  ")
            .expect("Invalid binaries/SHA256SUMS entry");
        let path = Path::new("binaries").join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        let actual = format!(
            "{:x}",
            Sha256::digest(fs::read(&path).unwrap_or_else(|error| {
                panic!("Could not read bundled engine {}: {error}", path.display())
            }))
        );
        assert_eq!(actual, expected, "Bundled engine checksum mismatch: {name}");
        target_verified |= name == target_name;
    }

    assert!(
        target_verified,
        "No verified bundled engine is recorded for {target}"
    );
}

fn main() {
    verify_sidecars();

    #[cfg(target_os = "macos")]
    {
        cc::Build::new()
            .file("macos/archive_services.m")
            .flag("-fobjc-arc")
            .compile("archive_services");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=QuickLookUI");
        println!("cargo:rustc-link-lib=framework=UniformTypeIdentifiers");
    }

    tauri_build::build()
}

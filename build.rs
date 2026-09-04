fn main() {
    println!("cargo:rerun-if-changed=packaging/macos/sparkle_bridge.m");
    if std::env::var("CARGO_FEATURE_SPARKLE").is_err() {
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("macos") {
        return;
    }

    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let sparkle_dir = manifest.join("resources/sparkle");
    let framework = sparkle_dir.join("Sparkle.framework");
    if !framework.is_dir() {
        panic!(
            "Sparkle.framework missing at {}; run scripts/fetch-sparkle.sh",
            framework.display()
        );
    }

    cc::Build::new()
        .file("packaging/macos/sparkle_bridge.m")
        .flag("-fobjc-arc")
        .flag(&format!("-F{}", sparkle_dir.display()))
        .compile("sparkle_bridge");

    println!(
        "cargo:rustc-link-search=framework={}",
        sparkle_dir.display()
    );
    println!("cargo:rustc-link-lib=framework=Sparkle");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    println!(
        "cargo:rustc-link-arg=-Wl,-rpath,{}",
        sparkle_dir.display()
    );
}

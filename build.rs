use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=MYPROXY_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=MYPROXY_VERSION");
    let channel = std::env::var("MYPROXY_BUILD_CHANNEL").unwrap_or_else(|_| {
        if std::env::var("PROFILE").ok().as_deref() == Some("release") {
            "prod"
        } else {
            "dev"
        }.into()
    });
    assert!(
        matches!(channel.as_str(), "prod" | "nightly" | "dev"),
        "invalid build channel"
    );
    let version = std::env::var("MYPROXY_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap());
    println!("cargo:rustc-env=MYPROXY_BUILD_CHANNEL={channel}");
    println!("cargo:rustc-env=MYPROXY_VERSION={version}");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed=packaging/macos/sparkle_bridge.m");
    println!("cargo:rerun-if-changed=macos/NetworkShared");
    println!("cargo:rerun-if-changed=macos/NetworkHost");
    println!("cargo:rerun-if-changed=scripts/build-network-host.sh");

    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("macos") {
        compile_network_host(&manifest);
    }

    if std::env::var("CARGO_FEATURE_SPARKLE").is_err() {
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_OS").ok().as_deref() != Some("macos") {
        return;
    }

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

fn compile_network_host(manifest: &PathBuf) {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("network-host");
    let status = Command::new(manifest.join("scripts/build-network-host.sh"))
        .arg(&out)
        .status()
        .expect("run scripts/build-network-host.sh");
    if !status.success() {
        panic!("scripts/build-network-host.sh failed");
    }

    let sdk = String::from_utf8(
        Command::new("xcrun")
            .args(["--show-sdk-path"])
            .output()
            .expect("xcrun --show-sdk-path")
            .stdout,
    )
    .expect("sdk path utf8");
    let sdk = sdk.trim();

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=MyproxyNetworkHost");
    println!("cargo:rustc-link-lib=static=MyproxyNetworkShared");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=NetworkExtension");
    println!("cargo:rustc-link-lib=framework=SystemExtensions");
    println!("cargo:rustc-link-lib=framework=Network");
    println!("cargo:rustc-link-lib=framework=Security");
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    println!("cargo:rustc-link-lib=dylib=swiftCore");
    println!("cargo:rustc-link-lib=dylib=swiftFoundation");
    println!("cargo:rustc-link-lib=dylib=swiftDispatch");
    println!("cargo:rustc-link-lib=dylib=swiftObjectiveC");
    println!("cargo:rustc-link-lib=dylib=swift_Concurrency");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
}

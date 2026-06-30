//! Build script for the Zebvix node binary.
//!
//! Embeds version information and performs feature-flag checks.

use std::process::Command;

fn main() {
    // Re-run if git state changes.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/");

    // Embed git commit hash.
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    // Embed build date.
    let build_date = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|ts| ts.parse::<i64>().ok())
        .map(|_| "reproducible-build".to_string())
        .unwrap_or_else(|| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            format!("{}", now.as_secs())
        });

    println!("cargo:rustc-env=ZBX_GIT_HASH={}", git_hash);
    println!("cargo:rustc-env=ZBX_BUILD_DATE={}", build_date);
    println!("cargo:rustc-env=ZBX_VERSION={}", env!("CARGO_PKG_VERSION"));

    // Verify required system libraries.
    #[cfg(target_os = "linux")]
    {
        if pkg_config::probe_library("rocksdb").is_err() {
            eprintln!("cargo:warning=librocksdb-dev not found; will use bundled build (slow)");
        }
    }
}
/// 当前版本（与 AGENTS.md 最新 changelog 条目保持一致）
const DEV_VERSION: &str = "2026.06.29.0010";

fn main() {
    println!("cargo:rerun-if-env-changed=GHDOWN_VERSION");
    println!("cargo:rerun-if-changed=build.rs");
    let ver = std::env::var("GHDOWN_VERSION").unwrap_or_else(|_| DEV_VERSION.to_string());
    println!("cargo:rustc-env=GHDOWN_VERSION={ver}");
}

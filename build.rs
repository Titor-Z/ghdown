fn main() {
    let ver = std::env::var("GHDOWN_VERSION")
        .unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.1.0".to_string()));
    println!("cargo:rustc-env=GHDOWN_VERSION={ver}");
}

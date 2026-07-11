fn main() {
    // date-only: keeps builds reproducible within a day and the footer honest
    let date = std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=FSHARE_BUILD_DATE={date}");
    println!("cargo:rerun-if-changed=build.rs");
}

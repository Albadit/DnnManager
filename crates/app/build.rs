// Puts assets/dnn.ico in the Windows exe as icon resource 1. Explorer, the Start menu and desktop shortcuts and the
// taskbar show the exe's icon, and GPUI loads resource 1 as every window's icon (gpui 0.2.2,
// src/platform/windows/platform.rs, load_icon). Without it they all show Windows' blank program icon.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=resources/windows/app.rc");
    println!("cargo:rerun-if-changed=../../assets/dnn.ico");
    // The build script runs on the host: embed-resource is there only on Windows (Cargo.toml), and the exe gets the
    // icon only when it is for Windows too. Elsewhere this is compiled out whole.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("resources/windows/app.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("embedding the exe's icon");
    }
}

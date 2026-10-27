//! Windows: the exe gets the app icon and version details (Explorer, the taskbar,
//! the installer and "Apps" in Settings show them). Only on a Windows host: the
//! resource compiler comes with the MSVC tools.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/screenpen.ico");
    println!("cargo:rerun-if-env-changed=SCREENPEN_VERSION");
    let target_windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if !(target_windows && cfg!(windows)) {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/screenpen.ico")
        .set("ProductName", "screenpen")
        .set("FileDescription", "screenpen: draw on top of the screen")
        .set("CompanyName", "Ivan Khramov")
        .set("LegalCopyright", "MIT License, Ivan Khramov")
        .set("OriginalFilename", "screenpen.exe");
    if let Ok(v) = std::env::var("SCREENPEN_VERSION") {
        res.set("ProductVersion", &v);
    }
    if let Err(e) = res.compile() {
        panic!("Windows resources: {e}");
    }
}

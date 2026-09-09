// Rust guideline compliant 2026-02-16

//! Embeds the Windows executable icon and version metadata.
//!
//! Without this the binaries ship with the default blank icon, which is what
//! Explorer, the taskbar and the tray all fall back to.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");

    // Only Windows builds carry a resource section; on any other target this
    // is a no-op rather than a build failure.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("FileDescription", "iRacing overlay and session launcher");
    res.set("ProductName", "race-tools");
    if let Err(err) = res.compile() {
        // A missing resource compiler shouldn't stop anyone building the
        // tools; they just get the default icon.
        println!("cargo:warning=could not embed the executable icon: {err}");
    }
}

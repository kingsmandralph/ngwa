//! Gives the Windows executable its icon and version details.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/ngwa.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("assets/icon/ngwa.ico")
            .set("ProductName", "Ngwa")
            .set("FileDescription", "Ngwa, a fast Matrix client");
        if let Err(e) = resource.compile() {
            // A missing resource compiler shouldn't stop a build.
            println!("cargo:warning=could not add the Windows icon: {e}");
        }
    }
}

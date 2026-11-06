// Gives the Windows executable its icon. Other platforms need nothing here.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Spotseek");
        res.set("FileDescription", "Spotseek");
        if let Err(e) = res.compile() {
            println!("cargo:warning=no Windows icon: {e}");
        }
    }
}

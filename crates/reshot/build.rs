fn main() {
    println!("cargo:rerun-if-changed=assets/reshot.ico");
    let mut res = winresource::WindowsResource::new();
    // Resource id 1: the shell thread loads the tray icon from it.
    res.set_icon_with_id("assets/reshot.ico", "1");
    // No manifest here: gpui embeds one (PerMonitorV2 DPI awareness, common controls v6),
    // and a second MANIFEST resource fails the link.
    res.set("ProductName", "Reshot");
    res.set("FileDescription", "Reshot");
    res.compile().expect("failed to embed Windows resources");
}

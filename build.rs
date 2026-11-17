fn main() {
    // Embed the icon and the manifest (visual styles + DPI awareness).
    // 嵌入图标和清单（视觉样式 + DPI 感知）。
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set_manifest_file("assets/app.manifest");
    res.compile().expect("failed to embed resources");
}

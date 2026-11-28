fn main() {
    // Embeds the exe icon. The application manifest (PerMonitorV2 DPI, common controls v6) comes from gpui's
    // `windows-manifest` feature; adding our own fails the link with CVT1100 (duplicate RT_MANIFEST).
    #[cfg(windows)]
    embed_resource::compile("icon.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}

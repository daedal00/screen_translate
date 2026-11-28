fn main() {
    // Embeds the exe icon and the application manifest (PerMonitorV2 DPI, common controls v6). gpui's own
    // manifest is not linked into a crate without bins; a duplicate RT_MANIFEST error (CVT1100) here means it is.
    #[cfg(windows)]
    embed_resource::compile("icon.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
}

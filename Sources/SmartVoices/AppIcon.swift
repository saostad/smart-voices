import AppKit

/// The Smart Voices app icon: from the bundle in the built .app, or from the
/// repo's Resources folder when running unbundled (swift run / render flags).
@MainActor
func appIconImage() -> NSImage? {
    if let url = Bundle.main.url(forResource: "AppIcon", withExtension: "icns"),
        let image = NSImage(contentsOf: url)
    {
        return image
    }
    if let image = NSImage(contentsOfFile: "Resources/AppIcon.icns") {
        return image
    }
    return NSApplication.shared.applicationIconImage
}

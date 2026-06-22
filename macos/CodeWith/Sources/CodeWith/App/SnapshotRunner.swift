import SwiftUI
import AppKit

/// A single named screen to render during snapshot mode.
struct SnapshotItem {
    let name: String
    let size: CGSize
    let view: AnyView
}

/// Renders every screen in the catalog to PNG using `ImageRenderer`. This runs
/// entirely in-process (no Screen Recording permission, no window server
/// dependency for capture), producing pixel-exact images of the SwiftUI render.
@MainActor
enum SnapshotRunner {
    static func run() {
        // A minimal NSApplication is initialised so AppKit text/material
        // rendering has an app context, but we never show a window.
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)

        let dir = ProcessInfo.processInfo.environment["CODEWITH_SNAPSHOT_DIR"] ?? "/tmp/codewith-shots"
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)

        let scale = CGFloat(Double(ProcessInfo.processInfo.environment["CODEWITH_SNAPSHOT_SCALE"] ?? "2") ?? 2)

        for item in SnapshotCatalog.items {
            render(item, into: dir, scale: scale)
        }
        FileHandle.standardError.write("snapshot: wrote \(SnapshotCatalog.items.count) screens to \(dir)\n".data(using: .utf8)!)
        exit(0)
    }

    private static func render(_ item: SnapshotItem, into dir: String, scale: CGFloat) {
        let renderer = ImageRenderer(content:
            item.view
                .environment(\.snapshotMode, true)
                .frame(width: item.size.width, height: item.size.height)
                .environment(\.colorScheme, .light)
        )
        renderer.scale = scale
        renderer.isOpaque = false
        guard let nsImage = renderer.nsImage,
              let tiff = nsImage.tiffRepresentation,
              let rep = NSBitmapImageRep(data: tiff),
              let png = rep.representation(using: .png, properties: [:]) else {
            FileHandle.standardError.write("snapshot: FAILED to render \(item.name)\n".data(using: .utf8)!)
            return
        }
        let path = "\(dir)/\(item.name).png"
        try? png.write(to: URL(fileURLWithPath: path))
    }
}

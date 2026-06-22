import SwiftUI
import AppKit

@main
enum CodeWithMain {
    static func main() {
        if ProcessInfo.processInfo.environment["CODEWITH_SNAPSHOT"] != nil {
            MainActor.assumeIsolated { SnapshotRunner.run() }
            return
        }
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        app.setActivationPolicy(.regular)
        app.run()
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let hosting = NSHostingController(rootView: AppShell())
        let win = NSWindow(contentViewController: hosting)
        win.setContentSize(WindowSize.app)
        win.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        win.titlebarAppearsTransparent = true
        win.titleVisibility = .hidden
        win.isMovableByWindowBackground = true
        win.center()
        win.makeKeyAndOrderFront(nil)
        self.window = win
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

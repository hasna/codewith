import SwiftUI
import AppKit

extension Notification.Name {
    static let codeWithOpenURL = Notification.Name("CodeWithOpenURL")
}

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
        NSAppleEventManager.shared().setEventHandler(
            self,
            andSelector: #selector(handleGetURLEvent(_:withReplyEvent:)),
            forEventClass: AEEventClass(kInternetEventClass),
            andEventID: AEEventID(kAEGetURL)
        )
        let hosting = NSHostingController(rootView: AppShell())
        // Let content extend under the transparent title bar so headers sit flush
        // at the top (no title-bar safe-area inset).
        if #available(macOS 13.3, *) { hosting.safeAreaRegions = [] }
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

    @objc private func handleGetURLEvent(_ event: NSAppleEventDescriptor, withReplyEvent replyEvent: NSAppleEventDescriptor) {
        guard let value = event.paramDescriptor(forKeyword: AEKeyword(keyDirectObject))?.stringValue,
              let url = URL(string: value) else { return }
        NotificationCenter.default.post(name: .codeWithOpenURL, object: url)
    }
}

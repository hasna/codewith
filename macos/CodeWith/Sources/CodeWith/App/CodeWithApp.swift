import SwiftUI
import AppKit

extension Notification.Name {
    static let codeWithOpenURL = Notification.Name("CodeWithOpenURL")
    static let codeWithMenuBarPreferenceChanged = Notification.Name("CodeWithMenuBarPreferenceChanged")
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

final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    private var window: NSWindow?
    private var statusItem: NSStatusItem?
    private var keepInMenuBar = true
    private var menuBarObserver: NSObjectProtocol?

    deinit {
        if let menuBarObserver {
            NotificationCenter.default.removeObserver(menuBarObserver)
        }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSAppleEventManager.shared().setEventHandler(
            self,
            andSelector: #selector(handleGetURLEvent(_:withReplyEvent:)),
            forEventClass: AEEventClass(kInternetEventClass),
            andEventID: AEEventID(kAEGetURL)
        )
        menuBarObserver = NotificationCenter.default.addObserver(
            forName: .codeWithMenuBarPreferenceChanged,
            object: nil,
            queue: .main
        ) { [weak self] note in
            guard let enabled = note.object as? Bool else { return }
            self?.setMenuBarEnabled(enabled)
        }
        setMenuBarEnabled(true)
        showMainWindow()
    }

    private func showMainWindow() {
        if let window {
            window.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
            return
        }

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
        win.delegate = self
        win.makeKeyAndOrderFront(nil)
        self.window = win
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        !keepInMenuBar
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard keepInMenuBar else { return true }
        sender.orderOut(nil)
        return false
    }

    func windowWillClose(_ notification: Notification) {
        guard let closedWindow = notification.object as? NSWindow, closedWindow === window else { return }
        window = nil
    }

    /// Reopen (Dock click, `open -a`, Finder relaunch) must restore the window:
    /// closing it only orders it out while the app lives in the menu bar, and
    /// without this AppKit silently does nothing on reopen.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag { showMainWindow() }
        return true
    }

    private func setMenuBarEnabled(_ enabled: Bool) {
        keepInMenuBar = enabled
        if enabled {
            ensureStatusItem()
        } else if let statusItem {
            NSStatusBar.system.removeStatusItem(statusItem)
            self.statusItem = nil
        }
    }

    private func ensureStatusItem() {
        if statusItem == nil {
            statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
            statusItem?.button?.title = "CodeWith"
        }
        let menu = NSMenu()
        let show = NSMenuItem(title: "Show CodeWith", action: #selector(showMainWindowFromMenuBar), keyEquivalent: "")
        show.target = self
        menu.addItem(show)
        menu.addItem(.separator())
        let quit = NSMenuItem(title: "Quit CodeWith", action: #selector(quitFromMenuBar), keyEquivalent: "q")
        quit.target = self
        menu.addItem(quit)
        statusItem?.menu = menu
    }

    @objc private func showMainWindowFromMenuBar() {
        showMainWindow()
    }

    @objc private func quitFromMenuBar() {
        keepInMenuBar = false
        NSApp.terminate(nil)
    }

    @objc private func handleGetURLEvent(_ event: NSAppleEventDescriptor, withReplyEvent replyEvent: NSAppleEventDescriptor) {
        guard let value = event.paramDescriptor(forKeyword: AEKeyword(keyDirectObject))?.stringValue,
              let url = URL(string: value) else { return }
        showMainWindow()
        NotificationCenter.default.post(name: .codeWithOpenURL, object: url)
    }
}

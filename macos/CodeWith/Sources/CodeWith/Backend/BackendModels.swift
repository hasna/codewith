import Foundation

/// A session (thread) summary as returned by `thread/list` / `thread/read`.
struct ThreadInfo: Identifiable, Hashable {
    let id: String
    var name: String
    var cwd: String?
    var preview: String?
    var updatedAt: String?
    var createdAt: String?
    var modelProvider: String?
    var status: String?
    var messages: [ChatMessage] = []

    init(from v: JSONValue) {
        id = v["id"]?.string ?? UUID().uuidString
        let nm = v["name"]?.string
        let pv = v["preview"]?.string
        name = (nm?.isEmpty == false ? nm : nil) ?? (pv?.isEmpty == false ? pv : nil) ?? "Untitled session"
        cwd = v["cwd"]?.string
        preview = pv
        updatedAt = v["updatedAt"]?.string ?? v["updatedAt"]?.double.map { String($0) }
        createdAt = v["createdAt"]?.string
        modelProvider = v["modelProvider"]?.string
        status = v["status"]?.string
    }

    /// Short relative-age label (e.g. "3w", "1m") parsed best-effort from updatedAt.
    var ageLabel: String {
        guard let updatedAt, let ts = ThreadInfo.parseDate(updatedAt) else { return "" }
        let secs = Date().timeIntervalSince(ts)
        switch secs {
        case ..<60: return "now"
        case ..<3600: return "\(Int(secs/60))m"
        case ..<86400: return "\(Int(secs/3600))h"
        case ..<604800: return "\(Int(secs/86400))d"
        case ..<2_592_000: return "\(Int(secs/604800))w"
        default: return "\(Int(secs/2_592_000))mo"
        }
    }

    static func parseDate(_ s: String) -> Date? {
        if let d = Double(s) { return Date(timeIntervalSince1970: d > 1_000_000_000_000 ? d/1000 : d) }
        let iso = ISO8601DateFormatter()
        iso.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return iso.date(from: s) ?? ISO8601DateFormatter().date(from: s)
    }
}

/// A project = a working directory / repo that has sessions.
struct ProjectInfo: Identifiable, Hashable {
    var id: String { path }
    var name: String
    var path: String
    var threadCount: Int

    /// Derive projects by grouping threads on their cwd.
    static func derive(from threads: [ThreadInfo]) -> [ProjectInfo] {
        var groups: [String: Int] = [:]
        var order: [String] = []
        for t in threads {
            guard let cwd = t.cwd, !cwd.isEmpty else { continue }
            if groups[cwd] == nil { order.append(cwd) }
            groups[cwd, default: 0] += 1
        }
        return order.map { path in
            ProjectInfo(name: (path as NSString).lastPathComponent, path: path, threadCount: groups[path] ?? 0)
        }
    }
}

/// A loop = a schedule or monitor running against a thread.
struct LoopInfo: Identifiable, Hashable {
    let id: String
    var title: String
    var subtitle: String
    var kind: Kind
    var active: Bool
    enum Kind: String { case schedule, monitor }
}

/// Account / profile info from `account/read`.
struct AccountInfo {
    var name: String
    var email: String
    var plan: String
    var initials: String

    init(from v: JSONValue) {
        // account/read → { account: Account|null, requiresOpenaiAuth }
        let acc = v["account"] ?? .null
        if acc.isNull {
            name = "Signed out"; email = ""; plan = ""; initials = "?"
            return
        }
        email = acc["email"]?.string ?? ""
        plan = acc["planType"]?.string ?? acc["plan"]?.string ?? ""
        name = acc["displayName"]?.string ?? acc["name"]?.string
            ?? (email.isEmpty ? (acc["type"]?.string ?? "Account") : email)
        let parts = name.split(separator: " ")
        let derived = parts.prefix(2).compactMap { $0.first }.map(String.init).joined().uppercased()
        initials = derived.isEmpty ? "?" : derived
    }
    static let signedOut = AccountInfo(from: .object(["name": .string("Signed out")]))
}

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
    var gitOriginUrl: String?
    var gitBranch: String?
    var gitSha: String?
    var messages: [ChatMessage] = []

    init(from v: JSONValue) {
        id = v["id"]?.string ?? UUID().uuidString
        let nm = v["name"]?.string
        let pv = v["preview"]?.string
        name = (nm?.isEmpty == false ? nm : nil) ?? (pv?.isEmpty == false ? pv : nil) ?? "Untitled session"
        cwd = v["cwd"]?.string
        preview = pv
        updatedAt = v["updatedAt"]?.string ?? v["updatedAt"]?.double.map { String(Int($0)) }
        createdAt = v["createdAt"]?.string ?? v["createdAt"]?.double.map { String(Int($0)) }
        modelProvider = v["modelProvider"]?.string
        // status is an object {type: "idle"|"active"|...} on the wire.
        status = v["status"]?["type"]?.string ?? v["status"]?.string
        gitOriginUrl = v["gitInfo"]?["originUrl"]?.string
        gitBranch = v["gitInfo"]?["branch"]?.string
        gitSha = v["gitInfo"]?["sha"]?.string
    }

    /// Repo-identity grouping key: normalized git origin if present, else cwd.
    var projectKey: String? {
        if let o = gitOriginUrl { return ProjectInfo.normalizeOrigin(o) }
        if let c = cwd, !c.isEmpty { return c }
        return nil
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

/// A project = a repo / working directory that has sessions. Grouped by git
/// origin when available (so sub-dirs of one repo are one project), else cwd.
struct ProjectInfo: Identifiable, Hashable {
    var id: String { groupKey }
    var name: String
    var path: String
    var groupKey: String
    var originUrl: String?
    var branch: String?
    var threadCount: Int
    var lastActivity: Int

    /// Derive projects from threads, grouping by repo identity (origin) or cwd.
    static func derive(from threads: [ThreadInfo]) -> [ProjectInfo] {
        struct Acc { var name: String; var path: String; var origin: String?; var branch: String?; var count: Int; var last: Int }
        var acc: [String: Acc] = [:]
        var order: [String] = []
        for t in threads {
            guard let cwd = t.cwd, !cwd.isEmpty else { continue }
            let key = t.projectKey ?? cwd
            let updated = Int(t.updatedAt ?? "") ?? 0
            if var a = acc[key] {
                a.count += 1
                if updated >= a.last { a.last = updated; a.branch = t.gitBranch ?? a.branch }
                acc[key] = a
            } else {
                order.append(key)
                acc[key] = Acc(
                    name: t.gitOriginUrl.map(repoName(fromOrigin:)) ?? (cwd as NSString).lastPathComponent,
                    path: cwd, origin: t.gitOriginUrl, branch: t.gitBranch, count: 1, last: updated)
            }
        }
        return order.map { k in
            let a = acc[k]!
            return ProjectInfo(name: a.name, path: a.path, groupKey: k, originUrl: a.origin,
                               branch: a.branch, threadCount: a.count, lastActivity: a.last)
        }
    }

    static func repoName(fromOrigin url: String) -> String {
        var s = url.hasSuffix(".git") ? String(url.dropLast(4)) : url
        if let slash = s.lastIndex(of: "/") { s = String(s[s.index(after: slash)...]) }
        else if let colon = s.lastIndex(of: ":") { s = String(s[s.index(after: colon)...]) }
        return s.isEmpty ? url : s
    }
    static func normalizeOrigin(_ url: String) -> String {
        var s = url.hasSuffix(".git") ? String(url.dropLast(4)) : url
        s = s.replacingOccurrences(of: "git@github.com:", with: "github.com/")
             .replacingOccurrences(of: "https://github.com/", with: "github.com/")
             .replacingOccurrences(of: "ssh://git@github.com/", with: "github.com/")
        return s.lowercased()
    }
}

/// A machine in the fleet (from `machines topology -j`).
struct MachineInfo: Identifiable, Hashable {
    var id: String
    var os: String
    var status: String   // online / offline / unknown
    var role: String
    var isLocal: Bool
    var online: Bool { status == "online" }

    init(id: String, os: String, status: String, role: String, isLocal: Bool) {
        self.id = id
        self.os = os
        self.status = status
        self.role = role
        self.isLocal = isLocal
    }

    init(registryValue v: JSONValue) {
        id = v["displayName"]?.string ?? v["machineId"]?.string ?? UUID().uuidString
        os = v["capabilities"]?["os"]?.string
            ?? v["capabilities"]?["platform"]?.string
            ?? v["adapterName"]?.string
            ?? "unknown"
        status = (v["healthState"]?.string ?? "unknown").lowercased()
        let source = v["sourceKind"]?.string ?? ""
        let trust = v["trustState"]?.string ?? ""
        role = [source, trust].filter { !$0.isEmpty }.joined(separator: " · ")
        isLocal = (v["trustState"]?.string ?? "").lowercased() == "local"
            || (v["sourceKind"]?.string ?? "").lowercased() == "local"
    }
}

/// An auth profile from `codewith profile list`.
struct AuthProfileInfo: Identifiable, Hashable {
    var id: String { name }
    var name: String
    var email: String
    var provider: String
    var plan: String
}

/// An installable app/skill from `app/list`.
struct AppItemInfo: Identifiable, Hashable {
    var id: String { name }
    var name: String
    var detail: String
    var enabled: Bool
}

/// A loop = a schedule or monitor running against a thread.
struct LoopInfo: Identifiable, Hashable {
    let id: String
    var title: String
    var subtitle: String
    var kind: Kind
    var active: Bool
    var threadId: String = ""
    enum Kind: String { case schedule, monitor }
}

/// Account / profile info from `account/read`.
struct AccountInfo {
    var name: String
    var email: String
    var plan: String
    var initials: String
    var requiresOpenAIAuth: Bool

    init(from v: JSONValue) {
        // account/read → { account: Account|null, requiresOpenaiAuth }
        requiresOpenAIAuth = v["requiresOpenaiAuth"]?.bool ?? true
        let acc = v["account"] ?? .null
        if acc.isNull {
            if requiresOpenAIAuth {
                name = "Signed out"; email = ""; plan = ""; initials = "?"
            } else {
                name = "Local provider"; email = ""; plan = "No account required"; initials = "LP"
            }
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

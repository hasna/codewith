import XCTest
@testable import CodeWith

final class JSONValueTests: XCTestCase {
    private func decode(_ s: String) -> JSONValue { try! JSONDecoder().decode(JSONValue.self, from: Data(s.utf8)) }

    func testDecodeScalars() {
        XCTAssertEqual(decode("null"), .null)
        XCTAssertEqual(decode("true"), .bool(true))
        XCTAssertEqual(decode("42"), .number(42))
        XCTAssertEqual(decode("\"hi\""), .string("hi"))
    }
    func testNestedAccess() {
        let v = decode("{\"a\":[1,{\"b\":\"c\"}]}")
        XCTAssertEqual(v["a"]?.array?.count, 2)
        XCTAssertEqual(v["a"]?.array?[1]["b"]?.string, "c")
    }
    func testRoundTrips() throws {
        let v = decode("{\"x\":[true,null,3.5,\"y\"]}")
        let data = try JSONEncoder().encode(v)
        XCTAssertEqual(try JSONDecoder().decode(JSONValue.self, from: data), v)
    }
    func testNumberCoercesStringDigits() {
        XCTAssertEqual(JSONValue.string("12").double, 12)
        XCTAssertEqual(JSONValue.string("12").int, 12)
        XCTAssertNil(JSONValue.string("xx").double)
    }
    func testIntTruncates() { XCTAssertEqual(JSONValue.number(3.9).int, 3) }
    func testBoolRejectsNumbers() { XCTAssertNil(JSONValue.number(1).bool) }
    func testSubscriptOnNonObjectNil() {
        let arr = JSONValue.array([])
        XCTAssertNil(arr["x"])
    }
    func testIsNull() { XCTAssertTrue(JSONValue.null.isNull); XCTAssertFalse(JSONValue.string("null").isNull) }
}

final class MessageRoutingTests: XCTestCase {
    private func classify(_ s: String) -> AppServerClient.Incoming { AppServerClient.classify(Data(s.utf8)) }

    func testResponseWithResult() {
        XCTAssertEqual(classify("{\"id\":7,\"result\":{\"ok\":true}}"),
                       .response(id: 7, result: .object(["ok": .bool(true)])))
    }
    func testResponseWithError() {
        XCTAssertEqual(classify("{\"id\":7,\"error\":{\"code\":-5,\"message\":\"no\"}}"),
                       .failure(id: 7, code: -5, message: "no"))
    }
    func testErrorDefaults() {
        XCTAssertEqual(classify("{\"id\":7,\"error\":{}}"), .failure(id: 7, code: -1, message: "unknown"))
    }
    func testNotification() {
        XCTAssertEqual(classify("{\"method\":\"turn/completed\",\"params\":{\"x\":1}}"),
                       .notification(method: "turn/completed", params: .object(["x": .number(1)])))
    }
    func testNotificationNoParams() {
        XCTAssertEqual(classify("{\"method\":\"initialized\"}"), .notification(method: "initialized", params: .null))
    }
    func testServerRequest() {
        XCTAssertEqual(
            classify("{\"id\":9,\"method\":\"item/commandExecution/requestApproval\",\"params\":{\"threadId\":\"t\"}}"),
            .serverRequest(
                id: .number(9),
                method: "item/commandExecution/requestApproval",
                params: .object(["threadId": .string("t")])
            )
        )
    }
    func testIdWithoutResultIgnored() { XCTAssertEqual(classify("{\"id\":7}"), .ignored) }
    func testIdPlusResultBeatsMethod() {
        XCTAssertEqual(classify("{\"id\":7,\"method\":\"x\",\"result\":1}"), .response(id: 7, result: .number(1)))
    }
    func testMalformedIgnored() { XCTAssertEqual(classify("not json"), .ignored) }
    func testEmptyObjectIgnored() { XCTAssertEqual(classify("{}"), .ignored) }
    func testNullErrorIsSuccess() { XCTAssertEqual(classify("{\"id\":7,\"result\":1,\"error\":null}"), .response(id: 7, result: .number(1))) }
}

// Regression: the app must never resolve its own GUI executable as the "codewith"
// CLI. On macOS's case-insensitive filesystem the candidate "Contents/MacOS/codewith"
// is the SAME file as the app binary "Contents/MacOS/CodeWith"; spawning it reboots
// the GUI, which spawns again — an unbounded fork bomb. The device+inode guard must
// see through the case folding.
final class BinaryResolutionTests: XCTestCase {
    private var tmp: NSString { NSTemporaryDirectory() as NSString }

    func testSameFileIdentifiesIdenticalPath() {
        let p = tmp.appendingPathComponent("cw-same-\(getpid())")
        FileManager.default.createFile(atPath: p, contents: Data("x".utf8))
        defer { try? FileManager.default.removeItem(atPath: p) }
        XCTAssertTrue(AppServerClient.sameFile(p, p))
    }

    func testSameFileDistinguishesDifferentFiles() {
        let a = tmp.appendingPathComponent("cw-a-\(getpid())")
        let b = tmp.appendingPathComponent("cw-b-\(getpid())")
        FileManager.default.createFile(atPath: a, contents: Data("a".utf8))
        FileManager.default.createFile(atPath: b, contents: Data("b".utf8))
        defer { try? FileManager.default.removeItem(atPath: a); try? FileManager.default.removeItem(atPath: b) }
        XCTAssertFalse(AppServerClient.sameFile(a, b))
    }

    func testSameFileSeesThroughCaseFolding() {
        let lower = tmp.appendingPathComponent("codewith-case-\(getpid())")
        let upper = tmp.appendingPathComponent("CODEWITH-case-\(getpid())")
        FileManager.default.createFile(atPath: lower, contents: Data("x".utf8))
        defer { try? FileManager.default.removeItem(atPath: lower) }
        // Only assert identity when the volume actually folds case (it does on macOS).
        if FileManager.default.isReadableFile(atPath: upper) {
            XCTAssertTrue(AppServerClient.sameFile(lower, upper))
        }
    }

    func testNonexistentPathsAreNotSameFile() {
        XCTAssertFalse(AppServerClient.sameFile("/no/such/aaa-\(getpid())", "/no/such/bbb-\(getpid())"))
    }

    func testOwnExecutableIsDetectedAsSelf() throws {
        let selfPath = try XCTUnwrap(Bundle.main.executablePath)
        XCTAssertTrue(AppServerClient.isSelfExecutable(selfPath))
    }

    func testResolvedBinaryIsNeverTheRunningExecutable() {
        if let resolved = AppServerClient.binaryPath {
            XCTAssertFalse(AppServerClient.isSelfExecutable(resolved))
        }
    }
}

final class BackendModelsTests: XCTestCase {
    private func obj(_ p: [String: JSONValue]) -> JSONValue { .object(p) }

    func testThreadInfoFields() {
        let t = ThreadInfo(from: obj(["id": .string("x"), "name": .string("My Thread"),
                                      "cwd": .string("/a/b"), "preview": .string("hi"),
                                      "modelProvider": .string("openai"), "status": .string("idle")]))
        XCTAssertEqual(t.id, "x"); XCTAssertEqual(t.name, "My Thread")
        XCTAssertEqual(t.cwd, "/a/b"); XCTAssertEqual(t.modelProvider, "openai")
    }
    func testThreadNameFallbackToPreview() {
        XCTAssertEqual(ThreadInfo(from: obj(["name": .string(""), "preview": .string("Do X")])).name, "Do X")
    }
    func testThreadNameFallbackToUntitled() {
        XCTAssertEqual(ThreadInfo(from: obj([:])).name, "Untitled session")
    }
    func testProjectDeriveGroupsByCwd() {
        let ts = ["/a", "/b", "/a"].map { ThreadInfo(from: obj(["cwd": .string($0)])) }
        let ps = ProjectInfo.derive(from: ts)
        XCTAssertEqual(ps.count, 2)
        XCTAssertEqual(ps.first { $0.path == "/a" }?.threadCount, 2)
    }
    func testProjectDerivePreservesOrderAndSkipsEmpty() {
        let ts = [ThreadInfo(from: obj(["cwd": .string("/z")])),
                  ThreadInfo(from: obj(["cwd": .string("")])),
                  ThreadInfo(from: obj([:])),
                  ThreadInfo(from: obj(["cwd": .string("/a")]))]
        let ps = ProjectInfo.derive(from: ts)
        XCTAssertEqual(ps.map(\.path), ["/z", "/a"])
    }
    func testProjectNameIsLastPathComponent() {
        let ps = ProjectInfo.derive(from: [ThreadInfo(from: obj(["cwd": .string("/Users/me/code/foo")]))])
        XCTAssertEqual(ps.first?.name, "foo")
    }
    func testProjectGroupsByGitOriginAcrossSubdirs() {
        // Two sessions in different sub-dirs of the same repo → one project.
        func t(_ cwd: String) -> ThreadInfo {
            ThreadInfo(from: obj(["cwd": .string(cwd), "gitInfo": obj(["originUrl": .string("git@github.com:hasnaxyz/iapp-mail.git"), "branch": .string("main")])]))
        }
        let ps = ProjectInfo.derive(from: [t("/Users/me/iapp-mail"), t("/Users/me/iapp-mail/sub")])
        XCTAssertEqual(ps.count, 1)
        XCTAssertEqual(ps.first?.name, "iapp-mail")
        XCTAssertEqual(ps.first?.branch, "main")
        XCTAssertEqual(ps.first?.threadCount, 2)
    }
    func testProjectFallsBackToCwdWhenNoGit() {
        let ps = ProjectInfo.derive(from: [ThreadInfo(from: obj(["cwd": .string("/Users/me/plain")]))])
        XCTAssertEqual(ps.first?.name, "plain")
        XCTAssertEqual(ps.first?.groupKey, "/Users/me/plain")
        XCTAssertNil(ps.first?.originUrl)
    }
    func testRepoNameParsesSshAndHttps() {
        XCTAssertEqual(ProjectInfo.repoName(fromOrigin: "git@github.com:o/iapp-mail.git"), "iapp-mail")
        XCTAssertEqual(ProjectInfo.repoName(fromOrigin: "https://github.com/o/r.git"), "r")
    }
    func testNormalizeOriginSshEqualsHttps() {
        XCTAssertEqual(ProjectInfo.normalizeOrigin("git@github.com:o/r.git"),
                       ProjectInfo.normalizeOrigin("https://github.com/o/r.git"))
    }
    func testThreadStatusDecodesObjectType() {
        let t = ThreadInfo(from: obj(["status": obj(["type": .string("idle")])]))
        XCTAssertEqual(t.status, "idle")
    }
    func testThreadGitInfoDecoded() {
        let t = ThreadInfo(from: obj(["gitInfo": obj(["originUrl": .string("git@github.com:o/r.git"), "branch": .string("dev"), "sha": .string("abc")])]))
        XCTAssertEqual(t.gitBranch, "dev"); XCTAssertEqual(t.gitSha, "abc")
        XCTAssertEqual(t.projectKey, "github.com/o/r")
    }
    func testAgeLabelBuckets() {
        func age(_ secsAgo: TimeInterval) -> String {
            let iso = ISO8601DateFormatter()
            return ThreadInfo(from: obj(["updatedAt": .string(iso.string(from: Date().addingTimeInterval(-secsAgo)))])).ageLabel
        }
        XCTAssertEqual(age(90), "1m")
        XCTAssertEqual(age(2*3600), "2h")
        XCTAssertEqual(age(3*86400), "3d")
        XCTAssertEqual(age(14*86400), "2w")
    }
    func testAgeLabelEmptyWhenNoDate() {
        XCTAssertEqual(ThreadInfo(from: obj([:])).ageLabel, "")
    }
    func testAccountNested() {
        let a = AccountInfo(from: obj(["account": obj(["displayName": .string("Andrei Hasna"),
                                                       "email": .string("a@b.c"), "planType": .string("pro")])]))
        XCTAssertEqual(a.name, "Andrei Hasna"); XCTAssertEqual(a.email, "a@b.c")
        XCTAssertEqual(a.plan, "pro"); XCTAssertEqual(a.initials, "AH")
    }
    func testAccountNullSignedOut() {
        let a = AccountInfo(from: obj(["account": .null]))
        XCTAssertEqual(a.name, "Signed out"); XCTAssertEqual(a.email, "")
    }
    func testAccountNullForNoAuthProviderIsSignedInState() {
        let a = AccountInfo(from: obj(["account": .null, "requiresOpenaiAuth": .bool(false)]))
        XCTAssertEqual(a.name, "Local provider")
        XCTAssertEqual(a.plan, "No account required")
        XCTAssertFalse(a.requiresOpenAIAuth)
    }
}

final class ParseItemTests: XCTestCase {
    private func obj(_ p: [String: JSONValue]) -> JSONValue { .object(p) }
    private func parse(_ p: [String: JSONValue]) -> ChatMessage? { AppServerClient.parseItem(obj(p)) }

    func testUserMessageString() {
        let m = parse(["type": .string("userMessage"), "content": .string("hello")])
        XCTAssertEqual(m?.role, .user); XCTAssertEqual(m?.text, "hello")
    }
    func testUserMessageArrayContent() {
        let m = parse(["type": .string("userMessage"),
                       "content": .array([obj(["text": .string("a")]), obj(["text": .string("b")])])])
        XCTAssertEqual(m?.text, "a\nb")
    }
    func testAgentMessage() {
        XCTAssertEqual(parse(["type": .string("agentMessage"), "text": .string("reply")])?.role, .assistant)
        XCTAssertNil(parse(["type": .string("agentMessage"), "text": .string("")]))
    }
    func testCommandExecution() {
        XCTAssertEqual(parse(["type": .string("commandExecution"), "command": .string("ls -la")])?.text, "Ran ls -la")
    }
    func testFileChange() {
        XCTAssertEqual(parse(["type": .string("fileChange"), "changes": .array([obj([:]), obj([:])])])?.text, "Edited 2 files")
        XCTAssertEqual(parse(["type": .string("fileChange"), "changes": .array([obj([:])])])?.text, "Edited 1 file")
    }
    func testToolCalls() {
        XCTAssertEqual(parse(["type": .string("mcpToolCall"), "tool": .string("search")])?.text, "Called search")
        XCTAssertEqual(parse(["type": .string("mcpToolCall")])?.text, "Called a tool")
    }
    func testWebSearch() {
        XCTAssertEqual(parse(["type": .string("webSearch"), "query": .string("swift")])?.text, "Searched: swift")
        XCTAssertEqual(parse(["type": .string("webSearch")])?.text, "Searched the web")
    }
    func testUnknownSkipped() {
        XCTAssertNil(parse(["type": .string("reasoning")]))
        XCTAssertNil(parse([:]))
    }
    func testExtractText() {
        XCTAssertEqual(AppServerClient.extractText(.string("x")), "x")
        XCTAssertEqual(AppServerClient.extractText(.array([obj(["text": .string("a")]), obj(["n": .number(1)]), obj(["text": .string("b")])])), "a\nb")
        XCTAssertEqual(AppServerClient.extractText(nil), "")
    }
}

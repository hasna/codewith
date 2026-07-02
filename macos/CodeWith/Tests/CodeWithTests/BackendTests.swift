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

final class SettingsConfigurationTests: XCTestCase {
    func testApprovalPolicyConfirmationOnlyForNever() {
        XCTAssertTrue(SettingsConfiguration.requiresConfirmation(approval: "never"))
        XCTAssertFalse(SettingsConfiguration.requiresConfirmation(approval: "on-request"))
        XCTAssertFalse(SettingsConfiguration.requiresConfirmation(approval: "on-failure"))
        XCTAssertFalse(SettingsConfiguration.requiresConfirmation(approval: "untrusted"))
    }

    func testSandboxConfirmationOnlyForFullAccess() {
        XCTAssertTrue(SettingsConfiguration.requiresConfirmation(sandbox: "danger-full-access"))
        XCTAssertFalse(SettingsConfiguration.requiresConfirmation(sandbox: "workspace-write"))
        XCTAssertFalse(SettingsConfiguration.requiresConfirmation(sandbox: "read-only"))
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
    func testThreadInfoDecodesMachineId() {
        let t = ThreadInfo(from: obj(["machineId": .string("machine-a")]))
        XCTAssertEqual(t.machineId, "machine-a")
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
    func testConfigRequirementsFiltersSupportedOptions() {
        let requirements = ConfigRequirementsInfo(from: obj([
            "allowedApprovalPolicies": .array([.string("on-request"), .string("never"), .string("unknown")]),
            "allowedSandboxModes": .array([.string("read-only")]),
        ]))
        XCTAssertEqual(
            requirements.approvalOptions(defaults: ["untrusted", "on-request", "never"]),
            ["on-request", "never"])
        XCTAssertEqual(
            requirements.sandboxOptions(defaults: ["read-only", "workspace-write", "danger-full-access"]),
            ["read-only"])
        XCTAssertFalse(requirements.allowsSandbox("danger-full-access"))
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
    func testLoopCreationDraftValidatesIntervalSchedules() {
        var draft = LoopCreationDraft()
        draft.scheduleMode = .interval
        draft.intervalAmount = "0"
        XCTAssertFalse(draft.canCreate)
        XCTAssertEqual(draft.validationMessage, "Enter a positive interval.")

        draft.intervalAmount = "15"
        XCTAssertTrue(draft.canCreate)
        XCTAssertEqual(draft.intervalAmountValue, 15)
    }
    func testLoopCreationDraftValidatesCronSchedules() {
        var draft = LoopCreationDraft()
        draft.scheduleMode = .cron
        draft.cronExpression = "   "
        XCTAssertFalse(draft.canCreate)
        XCTAssertEqual(draft.validationMessage, "Enter a cron expression.")

        draft.cronExpression = "*/15 * * * *"
        XCTAssertTrue(draft.canCreate)
        XCTAssertEqual(draft.normalizedCronExpression, "*/15 * * * *")
    }
    func testLoopCreationDraftValidatesMonitorRouting() {
        var draft = LoopCreationDraft()
        draft.kind = .monitor
        draft.command = "tail -f app.log"
        draft.routing = .file
        XCTAssertFalse(draft.canCreate)
        XCTAssertEqual(draft.validationMessage, "Enter an output file for file routing.")

        draft.outputFile = "monitor.log"
        XCTAssertTrue(draft.canCreate)

        draft.routing = .stream
        XCTAssertFalse(draft.canCreate)
        XCTAssertEqual(draft.validationMessage, "Output files are only valid for file routing.")
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
    func testMachineInfoKeepsRegistryIdAndDisplayName() {
        let machine = MachineInfo(registryValue: obj([
            "machineId": .string("machine-1"),
            "displayName": .string("Apple 03"),
            "healthState": .string("degraded"),
            "trustState": .string("trusted"),
            "sourceKind": .string("manual"),
            "capabilities": obj(["os": .string("darwin")]),
        ]))
        XCTAssertEqual(machine.id, "machine-1")
        XCTAssertEqual(machine.machineId, "machine-1")
        XCTAssertEqual(machine.displayName, "Apple 03")
        XCTAssertEqual(machine.status, "degraded")
        XCTAssertEqual(machine.os, "darwin")
        XCTAssertEqual(machine.role, "manual · trusted")
    }
    func testMachinePairingInfoPrefersManualDisplayCode() {
        let pairing = MachinePairingInfo(from: obj([
            "pairingCode": .string("pair-code"),
            "manualPairingCode": .string("ABCD-EFGH"),
            "environmentId": .string("env-1"),
            "expiresAt": .number(123),
        ]))
        XCTAssertEqual(pairing.pairingCode, "pair-code")
        XCTAssertEqual(pairing.manualPairingCode, "ABCD-EFGH")
        XCTAssertEqual(pairing.displayCode, "ABCD-EFGH")
        XCTAssertEqual(pairing.environmentId, "env-1")
        XCTAssertEqual(pairing.expiresAt, 123)
    }
    func testDesktopSettingsDecodeDefaultsAndConfiguredValues() {
        XCTAssertEqual(DesktopSettingsInfo(from: .null), DesktopSettingsInfo())
        XCTAssertEqual(
            DesktopSettingsInfo(from: obj([
                "workMode": .string("everyday"),
                "fileOpenDestination": .string("finder"),
                "language": .string("en"),
                "showMenuBar": .bool(false),
                "bottomPanel": .bool(true),
                "personality": .string("friendly"),
                "memoryEnabled": .bool(false),
                "chronicleResearch": .bool(true),
                "skipToolAssistedChats": .bool(true),
            ])),
            DesktopSettingsInfo(
                workMode: "everyday",
                fileOpenDestination: "finder",
                language: "en",
                showMenuBar: false,
                bottomPanel: true,
                personality: "friendly",
                memoryEnabled: false,
                chronicleResearch: true,
                skipToolAssistedChats: true))
    }
    func testDesktopSettingsDecodeRuntimeConfigValues() {
        XCTAssertEqual(
            DesktopSettingsInfo(
                desktop: obj([
                    "workMode": .string("everyday"),
                    "fileOpenDestination": .string("finder"),
                ]),
                config: obj([
                    "personality": .string("friendly"),
                    "features": obj([
                        "memories": .bool(true),
                        "chronicle": .bool(true),
                    ]),
                    "memories": obj([
                        "use_memories": .bool(true),
                        "generate_memories": .bool(true),
                        "disable_on_external_context": .bool(true),
                    ]),
                ])),
            DesktopSettingsInfo(
                workMode: "everyday",
                fileOpenDestination: "finder",
                personality: "friendly",
                memoryEnabled: true,
                chronicleResearch: true,
                skipToolAssistedChats: true))
    }
    func testDesktopSettingsDecodeFileOpenerFromRuntimeConfig() {
        XCTAssertEqual(
            DesktopSettingsInfo(
                desktop: .null,
                config: obj(["file_opener": .string("none")]))
            .fileOpenDestination,
            "system")
        XCTAssertEqual(
            DesktopSettingsInfo(
                desktop: .null,
                config: obj(["file_opener": .string("cursor")]))
            .fileOpenDestination,
            "cursor")
        XCTAssertEqual(
            DesktopSettingsInfo(
                desktop: obj(["fileOpenDestination": .string("finder")]),
                config: obj(["file_opener": .string("cursor")]))
            .fileOpenDestination,
            "finder")
    }
    func testGoalInfoDecodesThreadGoal() {
        let goal = GoalInfo(from: obj([
            "goalId": .string("goal-1"),
            "threadId": .string("thread-1"),
            "objective": .string("Ship it"),
            "status": .string("active"),
            "tokenBudget": .number(1000),
            "tokensUsed": .number(25),
            "timeUsedSeconds": .number(3),
        ]))
        XCTAssertEqual(goal.id, "goal-1")
        XCTAssertEqual(goal.threadId, "thread-1")
        XCTAssertEqual(goal.objective, "Ship it")
        XCTAssertEqual(goal.status, "active")
        XCTAssertEqual(goal.tokenBudget, 1000)
        XCTAssertEqual(goal.tokensUsed, 25)
        XCTAssertEqual(goal.timeUsedSeconds, 3)
    }
    func testGoalPlanInfoDecodesPlanAndNodes() {
        let plan = GoalPlanInfo(from: obj([
            "planId": .string("plan-1"),
            "threadId": .string("thread-1"),
            "status": .string("active"),
            "autoExecute": .string("readyOnly"),
            "nodeCount": .number(2),
            "completedNodeCount": .number(1),
            "activeNodeCount": .number(1),
            "pendingNodeCount": .number(0),
            "nodes": .array([
                obj([
                    "nodeId": .string("node-1"),
                    "planId": .string("plan-1"),
                    "threadId": .string("thread-1"),
                    "key": .string("implement"),
                    "objective": .string("Fix bugs"),
                    "status": .string("active"),
                    "ready": .bool(true),
                ]),
            ]),
        ]))
        XCTAssertEqual(plan.planId, "plan-1")
        XCTAssertEqual(plan.threadId, "thread-1")
        XCTAssertEqual(plan.status, "active")
        XCTAssertEqual(plan.autoExecute, "readyOnly")
        XCTAssertEqual(plan.nodeCount, 2)
        XCTAssertEqual(plan.completedNodeCount, 1)
        XCTAssertEqual(plan.activeNodeCount, 1)
        XCTAssertEqual(plan.pendingNodeCount, 0)
        XCTAssertEqual(plan.nodes.first?.nodeId, "node-1")
        XCTAssertEqual(plan.nodes.first?.objective, "Fix bugs")
        XCTAssertTrue(plan.nodes.first?.ready == true)
        XCTAssertEqual(plan.progressText, "1/2 complete")
        XCTAssertEqual(plan.nodes.first?.canActivate, false)
    }
    func testGoalPlanNodeCanActivateOnlyReadyPendingNodes() {
        XCTAssertTrue(GoalPlanNodeInfo(from: obj([
            "nodeId": .string("node-1"),
            "status": .string("pending"),
            "ready": .bool(true),
        ])).canActivate)
        XCTAssertFalse(GoalPlanNodeInfo(from: obj([
            "nodeId": .string("node-2"),
            "status": .string("active"),
            "ready": .bool(true),
        ])).canActivate)
        XCTAssertFalse(GoalPlanNodeInfo(from: obj([
            "nodeId": .string("node-3"),
            "status": .string("pending"),
            "ready": .bool(false),
        ])).canActivate)
    }
    func testThreadGoalPlanActivateNodeParams() {
        XCTAssertEqual(
            AppServerClient.threadGoalPlanActivateNodeParams(threadId: "thread-1", nodeId: "node-1"),
            obj([
                "threadId": .string("thread-1"),
                "nodeId": .string("node-1"),
            ]))
    }
    func testLoopInfoDecodesScheduleAndMonitor() {
        let schedule = AppServerClient.loopInfo(fromSchedule: obj([
            "threadId": .string("thread-1"),
            "scheduleId": .string("schedule-1"),
            "prompt": .string("Check CI"),
            "status": .string("active"),
            "schedule": obj(["type": .string("interval"), "amount": .number(5), "unit": .string("minutes")]),
        ]), fallbackThreadId: "fallback")
        XCTAssertEqual(schedule.id, "schedule-1")
        XCTAssertEqual(schedule.title, "Check CI")
        XCTAssertEqual(schedule.subtitle, "every 5 minutes")
        XCTAssertEqual(schedule.kind, .schedule)
        XCTAssertTrue(schedule.active)
        XCTAssertEqual(schedule.status, "active")
        XCTAssertTrue(schedule.canToggle)
        XCTAssertTrue(schedule.canRunNow)
        XCTAssertEqual(schedule.toggleLabel, "Pause")
        XCTAssertEqual(schedule.threadId, "thread-1")

        let monitor = AppServerClient.loopInfo(fromMonitor: obj([
            "threadId": .string("thread-2"),
            "monitorId": .string("monitor-1"),
            "name": .string("Watch logs"),
            "command": .string("tail -f app.log"),
            "status": .string("stopped"),
        ]), fallbackThreadId: "fallback")
        XCTAssertEqual(monitor.id, "monitor-1")
        XCTAssertEqual(monitor.title, "Watch logs")
        XCTAssertEqual(monitor.subtitle, "tail -f app.log")
        XCTAssertEqual(monitor.kind, .monitor)
        XCTAssertFalse(monitor.active)
        XCTAssertEqual(monitor.status, "stopped")
        XCTAssertTrue(monitor.canToggle)
        XCTAssertFalse(monitor.canRunNow)
        XCTAssertEqual(monitor.toggleLabel, "Restart")
        XCTAssertEqual(monitor.threadId, "thread-2")
    }
    func testLoopInfoExpiredScheduleDisablesActions() {
        let schedule = AppServerClient.loopInfo(fromSchedule: obj([
            "threadId": .string("thread-1"),
            "scheduleId": .string("schedule-1"),
            "prompt": .string("One shot"),
            "status": .string("expired"),
            "schedule": obj(["type": .string("once")]),
        ]), fallbackThreadId: "fallback")
        XCTAssertFalse(schedule.active)
        XCTAssertEqual(schedule.status, "expired")
        XCTAssertFalse(schedule.canToggle)
        XCTAssertFalse(schedule.canRunNow)
    }
    func testIsLoopScheduleExcludesOneTimeSchedules() {
        XCTAssertFalse(AppServerClient.isLoopSchedule(obj([
            "schedule": obj(["type": .string("once")]),
        ])))
        XCTAssertTrue(AppServerClient.isLoopSchedule(obj([
            "schedule": obj(["type": .string("dynamic")]),
        ])))
        XCTAssertTrue(AppServerClient.isLoopSchedule(obj([
            "schedule": obj(["type": .string("interval")]),
        ])))
    }
    func testActiveSessionPeerInfoDecodesPeer() {
        let peer = ActiveSessionPeerInfo(from: obj([
            "peerId": .string("peer-1"),
            "threadId": .string("thread-1"),
            "kind": .string("spawnedAgent"),
            "cwd": .string("/tmp/project"),
            "displayName": .string("Reviewer"),
            "capabilities": .array([.string("receiveMessage"), .string("triggerTurn")]),
        ]))
        XCTAssertEqual(peer.peerId, "peer-1")
        XCTAssertEqual(peer.threadId, "thread-1")
        XCTAssertEqual(peer.displayName, "Reviewer")
        XCTAssertEqual(peer.kind, "spawnedAgent")
        XCTAssertTrue(peer.canReceiveMessage)
        XCTAssertTrue(peer.canTriggerTurn)
        XCTAssertEqual(peer.menuSubtitle, "Agent in /tmp/project")
    }
    func testActiveSessionPeerInfoCanQueueWithoutTriggering() {
        let peer = ActiveSessionPeerInfo(from: obj([
            "peerId": .string("peer-1"),
            "threadId": .string("thread-1"),
            "capabilities": .array([.string("queueMessage")]),
        ]))
        XCTAssertTrue(peer.canQueueMessage)
        XCTAssertTrue(peer.canReceiveMessage)
        XCTAssertFalse(peer.canTriggerTurn)
    }
    func testAgentRunInfoDecodesRun() {
        let agent = AgentRunInfo(from: obj([
            "agentId": .string("agent-12345678"),
            "threadId": .string("thread-1"),
            "status": .string("waitingOnUser"),
            "desiredState": .string("running"),
            "retentionState": .string("active"),
            "source": .string("macos"),
            "rolloutPath": .string("/tmp/project"),
        ]))
        XCTAssertEqual(agent.agentId, "agent-12345678")
        XCTAssertEqual(agent.threadId, "thread-1")
        XCTAssertEqual(agent.status, "waitingOnUser")
        XCTAssertEqual(agent.desiredState, "running")
        XCTAssertEqual(agent.retentionState, "active")
        XCTAssertEqual(agent.source, "macos")
        XCTAssertEqual(agent.rolloutPath, "/tmp/project")
        XCTAssertEqual(agent.displayName, "Agent agent-12")
        XCTAssertEqual(agent.menuSubtitle, "waiting on user · project")
        XCTAssertTrue(agent.canOpenThread)
        XCTAssertFalse(agent.isDeleted)
    }
    func testAgentAttachmentInfoDecodesPendingInteractions() {
        let attachment = AgentAttachmentInfo(from: obj([
            "agent": obj([
                "agentId": .string("agent-12345678"),
                "threadId": .string("thread-1"),
                "status": .string("waitingOnUser"),
            ]),
            "statusSnapshot": obj([
                "status": .string("waitingOnUser"),
                "summary": .string("Needs approval"),
            ]),
            "events": .array([obj(["eventId": .string("event-1")])]),
            "pendingInteractions": .array([
                obj([
                    "interactionId": .string("interaction-1"),
                    "agentId": .string("agent-12345678"),
                    "kind": .string("approval"),
                    "status": .string("waiting"),
                    "requestPayload": obj(["title": .string("Approve command")]),
                ]),
            ]),
        ]), fallbackAgentId: "agent-12345678")

        XCTAssertEqual(attachment.agent?.agentId, "agent-12345678")
        XCTAssertEqual(attachment.status, "waitingOnUser")
        XCTAssertEqual(attachment.summary, "Needs approval")
        XCTAssertEqual(attachment.eventCount, 1)
        XCTAssertEqual(attachment.pendingCount, 1)
        XCTAssertEqual(attachment.pendingInteractions.first?.summary, "Approve command")
    }
    func testMachineInfoKeepsStableMachineIdAndDisplayName() {
        let machine = MachineInfo(registryValue: obj([
            "machineId": .string("machine-1"),
            "displayName": .string("Laptop"),
            "healthState": .string("online"),
        ]))
        XCTAssertEqual(machine.id, "machine-1")
        XCTAssertEqual(machine.machineId, "machine-1")
        XCTAssertEqual(machine.displayName, "Laptop")
    }
    func testWorkflowInfoDecodesSpecAndRun() {
        let workflow = WorkflowInfo(workflow: obj([
            "threadId": .string("thread-1"),
            "workflowRecordId": .string("workflow-1"),
            "displayName": .string("Ship"),
            "status": .string("draft"),
            "stepCount": .number(2),
            "agentCount": .number(1),
            "updatedAt": .number(20),
        ]), fallbackThreadId: "fallback")
        XCTAssertEqual(workflow.id, "workflow:workflow-1")
        XCTAssertEqual(workflow.threadId, "thread-1")
        XCTAssertEqual(workflow.title, "Ship")
        XCTAssertEqual(workflow.subtitle, "2 steps · 1 agent")

        let run = WorkflowInfo(run: obj([
            "runId": .string("run-1"),
            "status": .string("running"),
            "succeededStepCount": .number(1),
            "failedStepCount": .number(0),
            "activeStepCount": .number(1),
        ]), fallbackThreadId: "thread-2")
        XCTAssertEqual(run.id, "run:run-1")
        XCTAssertEqual(run.threadId, "thread-2")
        XCTAssertEqual(run.subtitle, "1 succeeded · 0 failed · 1 active")
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

final class AppServerRequestShapeTests: XCTestCase {
    private func obj(_ p: [String: JSONValue]) -> JSONValue { .object(p) }

    func testThreadGoalSetParams() {
        XCTAssertEqual(
            AppServerClient.threadGoalSetParams(
                threadId: "thread-1",
                objective: "Improve reliability",
                status: "active",
                tokenBudget: 2500),
            obj([
                "threadId": .string("thread-1"),
                "objective": .string("Improve reliability"),
                "status": .string("active"),
                "tokenBudget": .number(2500),
            ]))
    }

    func testThreadGoalSetParamsCanClearTokenBudget() {
        XCTAssertEqual(
            AppServerClient.threadGoalSetParams(
                threadId: "thread-1",
                tokenBudgetUpdate: .clear),
            obj([
                "threadId": .string("thread-1"),
                "tokenBudget": .null,
            ]))
    }

    func testThreadGoalListParams() {
        XCTAssertEqual(
            AppServerClient.threadGoalListParams(threadId: "thread-1", cursor: "cursor-1", limit: 20),
            obj([
                "threadId": .string("thread-1"),
                "cursor": .string("cursor-1"),
                "limit": .number(20),
            ]))
    }

    func testThreadScheduleCreateParams() {
        XCTAssertEqual(
            AppServerClient.threadScheduleCreateParams(
                threadId: "thread-1",
                prompt: "Check CI",
                promptSource: "inline",
                schedule: AppServerClient.intervalScheduleSpec(amount: 30, unit: .minutes),
                timezone: "UTC",
                nextRunAt: 100,
                expiresAt: 200),
            obj([
                "threadId": .string("thread-1"),
                "prompt": .string("Check CI"),
                "promptSource": .string("inline"),
                "schedule": obj([
                    "type": .string("interval"),
                    "amount": .number(30),
                    "unit": .string("minutes"),
                ]),
                "timezone": .string("UTC"),
                "nextRunAt": .number(100),
                "expiresAt": .number(200),
            ]))
    }

    func testThreadMonitorCreateParams() {
        XCTAssertEqual(
            AppServerClient.threadMonitorCreateParams(
                threadId: "thread-1",
                name: "Watch logs",
                prompt: "Tell me when an error appears",
                command: "tail -f app.log",
                cwd: "/tmp/project",
                routing: "both",
                outputFile: "monitor.log"),
            obj([
                "threadId": .string("thread-1"),
                "name": .string("Watch logs"),
                "prompt": .string("Tell me when an error appears"),
                "command": .string("tail -f app.log"),
                "cwd": .string("/tmp/project"),
                "routing": .string("both"),
                "outputFile": .string("monitor.log"),
            ]))
    }

    func testActiveSessionSendParams() {
        XCTAssertEqual(
            AppServerClient.activeSessionSendParams(
                targetPeerId: "peer-1",
                message: "Review this",
                senderThreadId: "thread-1",
                senderLabel: "CodeWith.app",
                delivery: "triggerTurn"),
            obj([
                "targetPeerId": .string("peer-1"),
                "message": .string("Review this"),
                "senderThreadId": .string("thread-1"),
                "senderLabel": .string("CodeWith.app"),
                "delivery": .string("triggerTurn"),
            ]))
    }

    func testAgentListParams() {
        XCTAssertEqual(
            AppServerClient.agentListParams(cursor: "cursor-1", limit: 25),
            obj([
                "cursor": .string("cursor-1"),
                "limit": .number(25),
            ]))
    }

    func testAgentReadAndAttachParams() {
        XCTAssertEqual(
            AppServerClient.agentIdParams(agentId: "agent-1"),
            obj(["agentId": .string("agent-1")]))
        XCTAssertEqual(
            AppServerClient.agentAttachParams(agentId: "agent-1", cursor: "cursor-1", limit: 10),
            obj([
                "agentId": .string("agent-1"),
                "cursor": .string("cursor-1"),
                "limit": .number(10),
            ]))
        XCTAssertEqual(
            AppServerClient.agentEventsListParams(agentId: "agent-1", cursor: "cursor-2", limit: 5),
            obj([
                "agentId": .string("agent-1"),
                "cursor": .string("cursor-2"),
                "limit": .number(5),
            ]))
    }

    func testAgentPendingInteractionRespondParams() {
        XCTAssertEqual(
            AppServerClient.agentPendingInteractionRespondParams(
                agentId: "agent-1",
                interactionId: "interaction-1",
                response: obj(["ok": .bool(true)]),
                terminalStatus: "responded"),
            obj([
                "agentId": .string("agent-1"),
                "interactionId": .string("interaction-1"),
                "response": obj(["ok": .bool(true)]),
                "terminalStatus": .string("responded"),
            ]))
    }

    func testTurnStartParamsIncludeModelProviderAndEffort() {
        XCTAssertEqual(
            AppServerClient.turnStartParams(
                threadId: "thread-1",
                input: "hello",
                model: "gpt-5.5",
                provider: "openai",
                effort: "medium"),
            obj([
                "threadId": .string("thread-1"),
                "input": .array([obj(["type": .string("text"), "text": .string("hello")])]),
                "model": .string("gpt-5.5"),
                "modelProvider": .string("openai"),
                "effort": .string("medium"),
            ]))
    }

    func testTurnStartParamsIncludePlanCollaborationMode() {
        XCTAssertEqual(
            AppServerClient.turnStartParams(
                threadId: "thread-1",
                input: "make a plan",
                model: "gpt-5.5",
                provider: "openai",
                effort: "medium",
                collaborationMode: AppServerClient.planCollaborationMode(model: "gpt-5.5", effort: "medium")),
            obj([
                "threadId": .string("thread-1"),
                "input": .array([obj(["type": .string("text"), "text": .string("make a plan")])]),
                "model": .string("gpt-5.5"),
                "modelProvider": .string("openai"),
                "effort": .string("medium"),
                "collaborationMode": obj([
                    "mode": .string("plan"),
                    "settings": obj([
                        "model": .string("gpt-5.5"),
                        "reasoning_effort": .string("medium"),
                        "developer_instructions": .null,
                    ]),
                ]),
            ]))
    }

    func testRemoteControlPairingStartParams() {
        XCTAssertEqual(
            AppServerClient.remoteControlPairingStartParams(manualCode: true),
            obj(["manualCode": .bool(true)]))
    }

    func testRemoteControlPairingStatusParamsPreferManualCode() {
        let manualPairing = MachinePairingInfo(from: obj([
            "pairingCode": .string("pair-code"),
            "manualPairingCode": .string("ABCD-EFGH"),
        ]))
        XCTAssertEqual(
            AppServerClient.remoteControlPairingStatusParams(pairing: manualPairing),
            obj(["manualPairingCode": .string("ABCD-EFGH")]))

        let pairing = MachinePairingInfo(from: obj(["pairingCode": .string("pair-code")]))
        XCTAssertEqual(
            AppServerClient.remoteControlPairingStatusParams(pairing: pairing),
            obj(["pairingCode": .string("pair-code")]))
    }

    func testConfigWriteParamsForDesktopSettings() {
        XCTAssertEqual(
            AppServerClient.configWriteParams(keyPath: "desktop.showMenuBar", value: .bool(false)),
            obj([
                "keyPath": .string("desktop.showMenuBar"),
                "value": .bool(false),
                "mergeStrategy": .string("replace"),
            ]))
    }

    func testConfigBatchWriteParamsCanReloadUserConfig() {
        XCTAssertEqual(
            AppServerClient.configBatchWriteParams(
                edits: [
                    (keyPath: "approval_policy", value: .string("on-request")),
                    (keyPath: "sandbox_mode", value: .string("workspace-write")),
                ],
                reloadUserConfig: true),
            obj([
                "edits": .array([
                    obj([
                        "keyPath": .string("approval_policy"),
                        "value": .string("on-request"),
                        "mergeStrategy": .string("replace"),
                    ]),
                    obj([
                        "keyPath": .string("sandbox_mode"),
                        "value": .string("workspace-write"),
                        "mergeStrategy": .string("replace"),
                    ]),
                ]),
                "reloadUserConfig": .bool(true),
            ]))
    }

    func testConfigWriteParamsForInstructions() {
        XCTAssertEqual(
            AppServerClient.configWriteParams(keyPath: "developer_instructions", value: .string("Prefer concise replies.")),
            obj([
                "keyPath": .string("developer_instructions"),
                "value": .string("Prefer concise replies."),
                "mergeStrategy": .string("replace"),
            ]))
    }

    func testConfigWriteParamsCanClearDeveloperInstructions() {
        XCTAssertEqual(
            AppServerClient.configWriteParams(keyPath: "developer_instructions", value: .null),
            obj([
                "keyPath": .string("developer_instructions"),
                "value": .null,
                "mergeStrategy": .string("replace"),
            ]))
    }

    func testConfigWriteParamsForMemorySettings() {
        XCTAssertEqual(
            AppServerClient.configWriteParams(keyPath: "features.memories", value: .bool(true)),
            obj([
                "keyPath": .string("features.memories"),
                "value": .bool(true),
                "mergeStrategy": .string("replace"),
            ]))
        XCTAssertEqual(
            AppServerClient.configWriteParams(keyPath: "memories.disable_on_external_context", value: .bool(true)),
            obj([
                "keyPath": .string("memories.disable_on_external_context"),
                "value": .bool(true),
                "mergeStrategy": .string("replace"),
            ]))
    }

    func testThreadSettingsUpdatePersonalityParams() {
        XCTAssertEqual(
            AppServerClient.threadSettingsUpdatePersonalityParams(threadId: "thread-1", personality: "friendly"),
            obj([
                "threadId": .string("thread-1"),
                "personality": .string("friendly"),
            ]))
    }

    func testThreadSettingsUpdateParamsIncludeSessionModelProviderAndEffort() {
        XCTAssertEqual(
            AppServerClient.threadSettingsUpdateParams(
                threadId: "thread-1",
                model: "gpt-5.5",
                provider: "openrouter",
                effort: "medium"),
            obj([
                "threadId": .string("thread-1"),
                "model": .string("gpt-5.5"),
                "modelProvider": .string("openrouter"),
                "effort": .string("medium"),
            ]))
    }

    func testThreadMemoryModeSetParams() {
        XCTAssertEqual(
            AppServerClient.threadMemoryModeSetParams(threadId: "thread-1", enabled: false),
            obj([
                "threadId": .string("thread-1"),
                "mode": .string("disabled"),
            ]))
    }
    func testThreadSettingsUpdateParamsIncludesPermissionsAndAuthProfile() {
        XCTAssertEqual(
            AppServerClient.threadSettingsUpdateParams(
                threadId: "thread-1",
                model: "gpt-5.5-codex",
                provider: "openai",
                effort: "high",
                permissions: ":workspace",
                authProfile: .set("work")),
            obj([
                "threadId": .string("thread-1"),
                "model": .string("gpt-5.5-codex"),
                "modelProvider": .string("openai"),
                "effort": .string("high"),
                "permissions": .string(":workspace"),
                "authProfile": .string("work"),
            ]))
    }

    func testThreadSettingsUpdateParamsCanClearAuthProfile() {
        XCTAssertEqual(
            AppServerClient.threadSettingsUpdateParams(
                threadId: "thread-1",
                authProfile: .clearDefault),
            obj([
                "threadId": .string("thread-1"),
                "authProfile": .null,
            ]))
    }

    func testEmptyThreadSettingsUpdateResponseHasNoSettings() {
        XCTAssertNil(ThreadSessionSettings(from: obj([:])))
    }

    func testThreadSessionSettingsParseCamelAndSnakeCase() {
        XCTAssertEqual(
            ThreadSessionSettings(from: obj([
                "threadSettings": obj([
                    "model": .string("gpt-5.5-codex"),
                    "model_provider": .string("openai"),
                    "reasoningEffort": .string("high"),
                    "active_permission_profile": obj(["id": .string(":workspace")]),
                    "auth_profile": .string("work"),
                ]),
            ])),
            ThreadSessionSettings(
                model: "gpt-5.5-codex",
                provider: "openai",
                effort: "high",
                permissionProfileId: ":workspace",
                authProfile: "work"))
    }

    func testThreadSessionSettingsCanClearAuthProfile() {
        XCTAssertEqual(
            ThreadSessionSettings(from: obj([
                "threadSettings": obj([
                    "authProfile": .null,
                ]),
            ])),
            ThreadSessionSettings(clearsAuthProfile: true))
    }

    func testConfigRequirementsKeepGranularApprovalPolicy() {
        let requirements = ConfigRequirementsInfo(from: obj([
            "allowedApprovalPolicies": .array([
                .string("on-request"),
                obj(["granular": obj(["read": .string("ask")])]),
            ]),
        ]))
        XCTAssertEqual(requirements.allowedApprovalPolicies, ["on-request", "granular"])
    }

    func testConfigRequirementsDecodesPermissionProfileConstraints() {
        let requirements = ConfigRequirementsInfo(from: obj([
            "allowedPermissionProfiles": obj([
                ":read-only": .bool(true),
                ":workspace": .bool(true),
                ":danger-full-access": .bool(false),
            ]),
            "defaultPermissions": .string(":workspace"),
        ]))

        XCTAssertEqual(requirements.allowedPermissionProfiles, [":read-only", ":workspace"])
        XCTAssertEqual(requirements.defaultPermissions, ":workspace")
        XCTAssertEqual(
            requirements.permissionProfileOptions(defaults: [":read-only", ":workspace", ":danger-full-access"]),
            [":read-only", ":workspace"])
    }
}

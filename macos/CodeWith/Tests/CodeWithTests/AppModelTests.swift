import XCTest
@testable import CodeWith

@MainActor
final class AppModelTests: XCTestCase {
    private func obj(_ p: [String: JSONValue]) -> JSONValue { .object(p) }

    func testInitialState() {
        let m = AppModel()
        XCTAssertEqual(m.route, .home)
        XCTAssertEqual(m.sidebarSelection, "New chat")
        XCTAssertFalse(m.showSettings)
        XCTAssertEqual(m.connection, .connecting)
        XCTAssertFalse(m.turnInProgress)
        XCTAssertTrue(m.activeMessages.isEmpty)
        XCTAssertTrue(m.threads.isEmpty)
    }

    func testOpenUpdatesRouteAndClosesMenus() {
        let m = AppModel()
        m.showAddMenu = true; m.showSettings = true
        m.open(.loops, label: "Loops")
        XCTAssertEqual(m.route, .loops)
        XCTAssertEqual(m.sidebarSelection, "Loops")
        XCTAssertFalse(m.showSettings)
        XCTAssertFalse(m.showAddMenu)
    }

    func testOpenSettings() {
        let m = AppModel()
        m.openSettings("Appearance")
        XCTAssertTrue(m.showSettings)
        XCTAssertEqual(m.settingsPage, "Appearance")
    }

    func testNewChatResets() {
        let m = AppModel()
        m.activeThreadId = "x"; m.activeMessages = [ChatMessage(role: .user, text: "hi")]
        m.composerText = "draft"; m.route = .chat("x")
        m.newChat()
        XCTAssertNil(m.activeThreadId)
        XCTAssertTrue(m.activeMessages.isEmpty)
        XCTAssertTrue(m.composerText.isEmpty)
        XCTAssertEqual(m.route, .home)
    }

    // MARK: streaming

    func testAgentDeltaAccumulatesIntoOneMessage() {
        let m = AppModel()
        for d in ["Hel", "lo ", "world"] {
            m.handleNotification(method: "item/agentMessage/delta", params: obj(["delta": .string(d)]))
        }
        XCTAssertEqual(m.activeMessages.count, 1)
        XCTAssertEqual(m.activeMessages.first?.role, .assistant)
        XCTAssertEqual(m.activeMessages.first?.text, "Hello world")
    }

    func testEmptyDeltaIgnored() {
        let m = AppModel()
        m.handleNotification(method: "item/agentMessage/delta", params: obj(["delta": .string("")]))
        XCTAssertTrue(m.activeMessages.isEmpty)
    }

    func testTurnStartedAndCompleted() {
        let m = AppModel()
        m.handleNotification(method: "turn/started", params: .null)
        XCTAssertTrue(m.turnInProgress)
        m.handleNotification(method: "item/agentMessage/delta", params: obj(["delta": .string("A")]))
        m.handleNotification(method: "turn/completed", params: .null)
        XCTAssertFalse(m.turnInProgress)
        // A subsequent delta starts a NEW assistant message (index reset).
        m.handleNotification(method: "item/agentMessage/delta", params: obj(["delta": .string("B")]))
        XCTAssertEqual(m.activeMessages.filter { $0.role == .assistant }.count, 2)
    }

    // Regression: a failed turn (e.g. 401 expired auth) arrives as turn/completed
    // with status:"failed"; the app must surface turn.error.message, not go silent.
    func testTurnCompletedFailedStatusSurfacesError() {
        let m = AppModel()
        m.handleNotification(method: "turn/started", params: .null)
        XCTAssertTrue(m.turnInProgress)
        m.handleNotification(method: "turn/completed", params: obj([
            "turn": obj(["status": .string("failed"),
                         "error": obj(["message": .string("401 Unauthorized: Incorrect API key")])]),
        ]))
        XCTAssertFalse(m.turnInProgress)
        XCTAssertEqual(m.activeMessages.last?.role, .assistant)
        XCTAssertEqual(m.activeMessages.last?.text, "⚠︎ 401 Unauthorized: Incorrect API key")
    }

    func testTurnCompletedSuccessAppendsNoError() {
        let m = AppModel()
        m.handleNotification(method: "turn/started", params: .null)
        m.handleNotification(method: "turn/completed", params: obj(["turn": obj(["status": .string("completed")])]))
        XCTAssertFalse(m.turnInProgress)
        XCTAssertTrue(m.activeMessages.isEmpty)
    }

    // Terminal errors (willRetry != true) surface; retryable ones are suppressed.
    func testTerminalErrorNotificationSurfaced() {
        let m = AppModel()
        m.handleNotification(method: "error", params: obj([
            "willRetry": .bool(false),
            "error": obj(["message": .string("stream disconnected")]),
        ]))
        XCTAssertEqual(m.activeMessages.last?.text, "⚠︎ stream disconnected")
    }

    func testRetryableErrorNotificationSuppressed() {
        let m = AppModel()
        m.handleNotification(method: "error", params: obj([
            "willRetry": .bool(true),
            "error": obj(["message": .string("transient")]),
        ]))
        XCTAssertTrue(m.activeMessages.isEmpty)
    }

    func testTurnFailureMessageHelper() {
        XCTAssertNil(AppModel.turnFailureMessage(.null))
        XCTAssertNil(AppModel.turnFailureMessage(obj(["turn": obj(["status": .string("completed")])])))
        XCTAssertEqual(
            AppModel.turnFailureMessage(obj(["turn": obj(["status": .string("failed"),
                "error": obj(["message": .string("boom")])])])),
            "boom")
        XCTAssertEqual(
            AppModel.turnFailureMessage(obj(["turn": obj(["status": .string("failed")])])),
            "The turn failed.")
    }

    func testItemCompletedAppendsToolAndEndsAssistantBubble() {
        let m = AppModel()
        m.handleNotification(method: "item/agentMessage/delta", params: obj(["delta": .string("A")]))
        m.handleNotification(method: "item/completed",
            params: obj(["item": obj(["type": .string("commandExecution"), "command": .string("ls")])]))
        m.handleNotification(method: "item/agentMessage/delta", params: obj(["delta": .string("B")]))
        // [assistant "A", tool, assistant "B"] — tool ends the bubble, B starts fresh.
        XCTAssertEqual(m.activeMessages.map(\.role), [.assistant, .tool, .assistant])
        XCTAssertEqual(m.activeMessages[0].text, "A")
        XCTAssertEqual(m.activeMessages[2].text, "B")
    }

    func testItemCompletedAgentMessageNotDuplicatedWhenStreamed() {
        let m = AppModel()
        m.handleNotification(method: "item/agentMessage/delta", params: obj(["delta": .string("hi")]))
        m.handleNotification(method: "item/completed",
            params: obj(["item": obj(["type": .string("agentMessage"), "text": .string("hi")])]))
        XCTAssertEqual(m.activeMessages.filter { $0.role == .assistant }.count, 1)
    }

    func testUnknownNotificationNoOp() {
        let m = AppModel()
        m.handleNotification(method: "garbage/method", params: .null)
        XCTAssertTrue(m.activeMessages.isEmpty)
        XCTAssertFalse(m.turnInProgress)
    }

    func testNotificationForDifferentThreadIgnored() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.handleNotification(method: "item/agentMessage/delta",
                             params: obj(["threadId": .string("thread-b"), "delta": .string("wrong")]))
        XCTAssertTrue(m.activeMessages.isEmpty)
        m.handleNotification(method: "item/agentMessage/delta",
                             params: obj(["threadId": .string("thread-a"), "delta": .string("right")]))
        XCTAssertEqual(m.activeMessages.first?.text, "right")
    }

    func testPermissionsApprovalRequestQueuedAndResolved() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.handleServerRequest(AppServerClient.ServerRequest(
            id: .number(3),
            method: "item/permissions/requestApproval",
            params: obj([
                "threadId": .string("thread-a"),
                "reason": .string("Needs write access"),
                "permissions": obj(["fileSystem": obj(["write": .array([.string("/tmp/project")])])]),
            ])
        ))
        XCTAssertEqual(m.pendingServerRequests.count, 1)
        XCTAssertEqual(m.pendingServerRequests.first?.kind, .permissionsApproval)
        XCTAssertEqual(m.pendingServerRequests.first?.requestedPermissions?["fileSystem"]?["write"]?.array?.first?.string,
                       "/tmp/project")
        m.handleNotification(method: "serverRequest/resolved",
                             params: obj(["threadId": .string("thread-a"), "requestId": .number(3)]))
        XCTAssertTrue(m.pendingServerRequests.isEmpty)
    }

    // MARK: add menu

    func testToggleAddMenu() {
        let m = AppModel()
        m.toggleAddMenu(); XCTAssertTrue(m.showAddMenu)
        m.toggleAddMenu(); XCTAssertFalse(m.showAddMenu)
    }
    func testAddActionPlanMode() {
        let m = AppModel(); m.showAddMenu = true
        m.handleAddAction("Plan mode")
        XCTAssertTrue(m.planMode); XCTAssertFalse(m.showAddMenu)
    }
    func testAddActionGoalPrefixesOnce() {
        let m = AppModel(); m.composerText = "ship it"
        m.handleAddAction("Goal"); XCTAssertEqual(m.composerText, "Goal: ship it")
        m.handleAddAction("Goal"); XCTAssertEqual(m.composerText, "Goal: ship it")
    }
    func testGoalObjectiveStripsExistingPrefix() {
        XCTAssertEqual(AppModel.goalObjective(from: "Goal: ship it"), "ship it")
        XCTAssertEqual(AppModel.goalObjective(from: "  goal: ship it  "), "ship it")
        XCTAssertEqual(AppModel.goalObjective(from: "ship it"), "ship it")
        XCTAssertEqual(AppModel.goalObjective(from: "Goal:   "), "")
    }
    func testAddActionAgentMention() {
        let m = AppModel(); m.composerText = "review"
        m.handleAddAction("Apollo")
        XCTAssertEqual(m.composerText, "@Apollo review")
        XCTAssertFalse(m.showAddMenu)
    }
    func testConfigSetters() {
        let m = AppModel()
        m.setModel("o3"); m.setProvider("azure"); m.setEffort("High")
        XCTAssertEqual(m.model, "o3"); XCTAssertEqual(m.provider, "azure"); XCTAssertEqual(m.effort, "High")
    }
    func testProviderIDMapping() {
        XCTAssertEqual(AppModel.providerID(for: "OpenAI"), "openai")
        XCTAssertEqual(AppModel.providerID(for: "OpenRouter"), "openrouter")
        XCTAssertEqual(AppModel.providerID(for: "Anthropic"), "anthropic")
        XCTAssertEqual(AppModel.providerID(for: "Azure"), "azure")
        XCTAssertEqual(AppModel.providerID(for: "Ollama"), "ollama")
    }

    func testSubmitNoOpWhenNotConnected() async {
        let m = AppModel()   // connection == .connecting
        m.composerText = "hello"
        await m.submitComposer()
        XCTAssertTrue(m.activeMessages.isEmpty)
        XCTAssertEqual(m.composerText, "hello")
    }

    func testSampleModelHasData() {
        let m = AppModel.sample()
        XCTAssertEqual(m.connection, .connected)
        XCTAssertFalse(m.threads.isEmpty)
        XCTAssertFalse(m.projects.isEmpty)
        XCTAssertFalse(m.loops.isEmpty)
        XCTAssertEqual(m.account.name, "Andrei Hasna")
    }
}

// MARK: - Integration (gated: needs the real codewith binary)

final class AppServerIntegrationTests: XCTestCase {
    func testSpawnInitializeAndListThreads() async throws {
        try XCTSkipUnless(AppServerClient.binaryPath != nil, "codewith not installed; skipping live test")
        let client = AppServerClient()
        try client.start()
        defer { client.stop() }
        let initResult = try await client.initialize()
        XCTAssertFalse(initResult.isNull, "initialize should return a non-null result")
        let (threads, _) = try await client.listThreads(limit: 3)
        XCTAssertNotNil(threads)   // array (possibly empty) — no RPC error thrown
    }

    // Regression: a candidate binary that exits immediately (e.g. a bundled CLI
    // that lost its node_modules) must surface as a fast initialize FAILURE — not
    // hang — so bootstrap() can fall through to the next candidate instead of
    // leaving the app stuck "connecting" with no app-server.
    func testStartWithImmediatelyExitingBinaryFailsInitialize() async throws {
        let dir = NSTemporaryDirectory() as NSString
        let broken = dir.appendingPathComponent("cw-broken-\(getpid())")
        try "#!/bin/sh\nexit 1\n".write(toFile: broken, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: broken)
        defer { try? FileManager.default.removeItem(atPath: broken) }

        let client = AppServerClient()
        try client.start(binary: broken)
        defer { client.stop() }
        do {
            _ = try await client.initialize()
            XCTFail("initialize should throw when the spawned binary exits immediately")
        } catch {
            // expected: the process died, so the handshake fails fast
        }
    }
}

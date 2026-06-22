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

// MARK: - AgentRunner (still present, pure)

final class AgentRunnerTests: XCTestCase {
    func test401IsNotAuthenticated() {
        if case .notAuthenticated = AgentRunner.classify(exitCode: 1, output: "401 Unauthorized: Missing bearer") {}
        else { XCTFail() }
    }
    func testReconnectingIsNotAuthenticated() {
        if case .notAuthenticated = AgentRunner.classify(exitCode: 1, output: "ERROR: Reconnecting... 5/5") {}
        else { XCTFail() }
    }
    func testCleanReplyTrims() {
        if case .reply(let t) = AgentRunner.classify(exitCode: 0, output: "  done.\n") { XCTAssertEqual(t, "done.") }
        else { XCTFail() }
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
}

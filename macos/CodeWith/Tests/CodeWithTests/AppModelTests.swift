import XCTest
@testable import CodeWith

@MainActor
final class AppModelTests: XCTestCase {
    private func obj(_ p: [String: JSONValue]) -> JSONValue { .object(p) }

    func testInitialState() {
        let m = AppModel()
        XCTAssertEqual(m.route, .home)
        XCTAssertEqual(m.sidebarSelection, "Home")
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

    func testSetShowMenuBarPublishesPreference() {
        var received: Bool?
        let token = NotificationCenter.default.addObserver(
            forName: .codeWithMenuBarPreferenceChanged,
            object: nil,
            queue: nil
        ) { note in
            received = note.object as? Bool
        }
        defer { NotificationCenter.default.removeObserver(token) }

        let m = AppModel()
        m.setShowMenuBar(false)

        XCTAssertEqual(received, false)
        XCTAssertFalse(m.desktopSettings.showMenuBar)
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

    func testTurnCompletedForPreviousThreadClearsStateWithoutAppendingToCurrentThread() {
        let m = AppModel()
        m.activeThreadId = "thread-current"
        m.activeTurnThreadId = "thread-old"
        m.activeTurnId = "turn-old"
        m.turnInProgress = true
        m.handleNotification(method: "turn/completed", params: obj([
            "threadId": .string("thread-old"),
            "turn": obj([
                "status": .string("failed"),
                "error": obj(["message": .string("old failure")]),
            ]),
        ]))
        XCTAssertFalse(m.turnInProgress)
        XCTAssertNil(m.activeTurnThreadId)
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

    func testCommandExecutionOutputDeltaStreamsIntoToolMessage() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.handleNotification(method: "item/commandExecution/outputDelta", params: obj([
            "threadId": .string("thread-a"),
            "turnId": .string("turn-1"),
            "itemId": .string("cmd-1"),
            "delta": .string("hello"),
        ]))
        m.handleNotification(method: "item/commandExecution/outputDelta", params: obj([
            "threadId": .string("thread-a"),
            "turnId": .string("turn-1"),
            "itemId": .string("cmd-1"),
            "delta": .string(" world"),
        ]))
        XCTAssertEqual(m.activeMessages.count, 1)
        XCTAssertEqual(m.activeMessages.first?.role, .tool)
        XCTAssertEqual(m.activeMessages.first?.toolIcon, "terminal")
        XCTAssertEqual(m.activeMessages.first?.text, "Command output:\nhello world")

        m.handleNotification(method: "item/completed", params: obj([
            "threadId": .string("thread-a"),
            "item": obj([
                "type": .string("commandExecution"),
                "id": .string("cmd-1"),
                "command": .string("echo hello"),
                "status": .string("completed"),
            ]),
        ]))
        XCTAssertEqual(m.activeMessages.count, 1)
        XCTAssertEqual(m.activeMessages.first?.text, "Ran echo hello\nCommand output:\nhello world")
    }

    func testCommandExecutionOutputDeltaForOtherThreadIgnored() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.handleNotification(method: "item/commandExecution/outputDelta", params: obj([
            "threadId": .string("thread-b"),
            "turnId": .string("turn-1"),
            "itemId": .string("cmd-1"),
            "delta": .string("wrong"),
        ]))
        XCTAssertTrue(m.activeMessages.isEmpty)
    }

    func testCommandExecOutputDeltaDecodesBase64() {
        let m = AppModel()
        m.handleNotification(method: "command/exec/outputDelta", params: obj([
            "processId": .string("proc-1"),
            "stream": .string("stdout"),
            "deltaBase64": .string("aGVsbG8K"),
            "capReached": .bool(false),
        ]))
        XCTAssertEqual(m.activeMessages.count, 1)
        XCTAssertEqual(m.activeMessages.first?.role, .tool)
        XCTAssertEqual(m.activeMessages.first?.text, "Command output:\nhello\n")
    }

    func testTerminalInteractionUsesCommandOutputRow() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.handleNotification(method: "item/commandExecution/terminalInteraction", params: obj([
            "threadId": .string("thread-a"),
            "turnId": .string("turn-1"),
            "itemId": .string("cmd-1"),
            "processId": .string("proc-1"),
            "stdin": .string("password:"),
        ]))
        XCTAssertEqual(m.activeMessages.first?.text, "Terminal interaction:\nTerminal input requested:\npassword:")
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

    func testOpeningDifferentThreadDetachesVisibleTurnState() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.activeTurnThreadId = "thread-a"
        m.activeTurnId = "turn-a"
        m.turnInProgress = true

        m.activeThreadId = "thread-b"
        m.handleNotification(method: "item/commandExecution/outputDelta",
                             params: obj(["threadId": .string("thread-a"), "delta": .string("wrong")]))

        XCTAssertTrue(m.activeMessages.isEmpty)
    }

    func testCommandOutputForPreviousTurnIgnoredAfterDetach() {
        let m = AppModel()
        m.activeThreadId = "thread-b"
        m.activeTurnThreadId = "thread-a"
        m.turnInProgress = true

        m.handleNotification(method: "item/commandExecution/outputDelta",
                             params: obj(["threadId": .string("thread-a"), "delta": .string("wrong")]))

        XCTAssertTrue(m.activeMessages.isEmpty)
    }

    func testCompletionForPreviousVisibleTurnStillClearsTracking() {
        let m = AppModel()
        m.activeThreadId = "thread-b"
        m.activeTurnThreadId = "thread-a"
        m.activeTurnId = "turn-a"
        m.turnInProgress = true

        m.handleNotification(method: "turn/completed",
                             params: obj(["threadId": .string("thread-a"), "turn": obj(["status": .string("completed")])]))

        XCTAssertFalse(m.turnInProgress)
        XCTAssertNil(m.activeTurnId)
        XCTAssertNil(m.activeTurnThreadId)
        XCTAssertTrue(m.activeMessages.isEmpty)
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
        XCTAssertEqual(m.pendingServerRequests.first?.actions.map(\.title), ["Decline", "Approve"])
        XCTAssertEqual(m.pendingServerRequests.first?.actions.last?.result,
                       obj([
                           "scope": .string("turn"),
                           "permissions": obj(["fileSystem": obj(["write": .array([.string("/tmp/project")])])]),
                       ]))
        m.handleNotification(method: "serverRequest/resolved",
                             params: obj(["threadId": .string("thread-a"), "requestId": .number(3)]))
        XCTAssertTrue(m.pendingServerRequests.isEmpty)
    }

    func testServerRequestForInactiveThreadIsQueuedForThatThread() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.handleServerRequest(AppServerClient.ServerRequest(
            id: .number(4),
            method: "item/commandExecution/requestApproval",
            params: obj([
                "threadId": .string("thread-b"),
                "command": .string("make test"),
            ])
        ))
        XCTAssertEqual(m.pendingServerRequests.count, 1)
        XCTAssertNil(m.pendingServerRequestForActiveThread)
        m.activeThreadId = "thread-b"
        XCTAssertEqual(m.pendingServerRequestForActiveThread?.threadId, "thread-b")
    }

    func testCommandApprovalUsesAvailableDecisions() {
        let m = AppModel()
        m.handleServerRequest(AppServerClient.ServerRequest(
            id: .number(5),
            method: "item/commandExecution/requestApproval",
            params: obj([
                "threadId": .string("thread-a"),
                "command": .string("make test"),
                "availableDecisions": .array([
                    .string("acceptForSession"),
                    .string("cancel"),
                ]),
            ])
        ))

        let actions = m.pendingServerRequests.first?.actions
        XCTAssertEqual(actions?.map(\.title), ["Approve session", "Cancel"])
        XCTAssertEqual(actions?.first?.result,
                       obj(["decision": .string("acceptForSession")]))
        XCTAssertEqual(actions?.last?.result,
                       obj(["decision": .string("cancel")]))
    }

    func testCommandApprovalPreservesPolicyDecisionObject() {
        let decision = obj([
            "acceptWithExecpolicyAmendment": obj([
                "execpolicy_amendment": .array([.string("npm"), .string("test")]),
            ]),
        ])
        let m = AppModel()
        m.handleServerRequest(AppServerClient.ServerRequest(
            id: .number(6),
            method: "item/commandExecution/requestApproval",
            params: obj([
                "threadId": .string("thread-a"),
                "command": .string("npm test"),
                "availableDecisions": .array([decision]),
            ])
        ))

        let action = m.pendingServerRequests.first?.actions.first
        XCTAssertEqual(action?.title, "Trust command")
        XCTAssertEqual(action?.result, obj(["decision": decision]))
    }

    func testFileChangeApprovalOffersSessionActionForGrantRoot() {
        let m = AppModel()
        m.handleServerRequest(AppServerClient.ServerRequest(
            id: .number(7),
            method: "item/fileChange/requestApproval",
            params: obj([
                "threadId": .string("thread-a"),
                "reason": .string("Patch files"),
                "grantRoot": .string("/tmp/project"),
            ])
        ))

        let actions = m.pendingServerRequests.first?.actions
        XCTAssertEqual(actions?.map(\.title), ["Decline", "Approve", "Approve session"])
        XCTAssertEqual(actions?.last?.result,
                       obj(["decision": .string("acceptForSession")]))
    }

    func testToolRequestUserInputQueuedAndResponsePayloadBuilt() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.handleServerRequest(AppServerClient.ServerRequest(
            id: .number(8),
            method: "item/tool/requestUserInput",
            params: obj([
                "threadId": .string("thread-a"),
                "turnId": .string("turn-a"),
                "itemId": .string("item-a"),
                "questions": .array([
                    obj([
                        "id": .string("confirm_path"),
                        "header": .string("Confirm"),
                        "question": .string("Use this path?"),
                        "isOther": .bool(true),
                        "options": .array([
                            obj([
                                "label": .string("Yes"),
                                "description": .string("Use the proposed path."),
                            ]),
                        ]),
                    ]),
                ]),
            ])
        ))

        let prompt = try! XCTUnwrap(m.pendingUserInputForActiveThread)
        XCTAssertEqual(prompt.title, "Input requested")
        XCTAssertEqual(prompt.questions.first?.id, "confirm_path")
        XCTAssertEqual(prompt.questions.first?.options.first?.label, "Yes")

        let response = AppModel.userInputResponse(
            for: prompt,
            answers: ["confirm_path": ["Yes", "user_note: looks right"]])
        XCTAssertEqual(response, obj([
            "answers": obj([
                "confirm_path": obj([
                    "answers": .array([
                        .string("Yes"),
                        .string("user_note: looks right"),
                    ]),
                ]),
            ]),
        ]))
    }

    func testServerRequestResolutionRemovesUserInputRequest() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.handleServerRequest(AppServerClient.ServerRequest(
            id: .number(9),
            method: "item/tool/requestUserInput",
            params: obj([
                "threadId": .string("thread-b"),
                "turnId": .string("turn-b"),
                "itemId": .string("item-b"),
                "questions": .array([
                    obj([
                        "id": .string("q"),
                        "header": .string("Question"),
                        "question": .string("Answer?"),
                    ]),
                ]),
            ])
        ))

        XCTAssertEqual(m.pendingUserInputRequests.count, 1)
        XCTAssertNil(m.pendingUserInputForActiveThread)
        m.handleNotification(
            method: "serverRequest/resolved",
            params: obj(["threadId": .string("thread-b"), "requestId": .number(9)]))
        XCTAssertTrue(m.pendingUserInputRequests.isEmpty)
    }

    func testMcpElicitationFormQueuedAndContentPayloadBuilt() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.handleServerRequest(AppServerClient.ServerRequest(
            id: .number(10),
            method: "mcpServer/elicitation/request",
            params: obj([
                "threadId": .string("thread-a"),
                "turnId": .string("turn-a"),
                "serverName": .string("codex_apps"),
                "mode": .string("form"),
                "message": .string("Configure the connector."),
                "requestedSchema": obj([
                    "type": .string("object"),
                    "required": .array([.string("approved"), .string("env")]),
                    "properties": obj([
                        "approved": obj([
                            "type": .string("boolean"),
                            "title": .string("Approved"),
                            "description": .string("Allow access?"),
                            "default": .bool(true),
                        ]),
                        "count": obj([
                            "type": .string("integer"),
                            "title": .string("Count"),
                        ]),
                        "env": obj([
                            "type": .string("string"),
                            "title": .string("Environment"),
                            "enum": .array([.string("dev"), .string("prod")]),
                            "enumNames": .array([.string("Development"), .string("Production")]),
                            "default": .string("dev"),
                        ]),
                        "notes": obj([
                            "type": .string("string"),
                            "title": .string("Notes"),
                        ]),
                    ]),
                ]),
            ])
        ))

        let prompt = try! XCTUnwrap(m.pendingMcpElicitationForActiveThread)
        XCTAssertEqual(prompt.title, "MCP input requested")
        XCTAssertEqual(prompt.message, "Configure the connector.")
        XCTAssertEqual(prompt.fields.map(\.id), ["approved", "count", "env", "notes"])
        XCTAssertEqual(prompt.fields.first { $0.id == "env" }?.options.map(\.label),
                       ["Development", "Production"])

        let content = AppModel.mcpElicitationContent(
            for: prompt,
            values: [
                "approved": .bool(false),
                "count": .number(3),
                "notes": .string("ok"),
            ])
        XCTAssertEqual(content, obj([
            "approved": .bool(false),
            "count": .number(3),
            "env": .string("dev"),
            "notes": .string("ok"),
        ]))
        XCTAssertEqual(AppModel.mcpElicitationResponse(action: "accept", content: content), obj([
            "action": .string("accept"),
            "content": content,
            "_meta": .null,
        ]))
    }

    func testMcpElicitationUrlQueuedAndResolved() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.handleServerRequest(AppServerClient.ServerRequest(
            id: .number(11),
            method: "mcpServer/elicitation/request",
            params: obj([
                "threadId": .string("thread-b"),
                "serverName": .string("remote"),
                "mode": .string("url"),
                "message": .string("Complete this in the browser."),
                "url": .string("https://example.com/connect"),
                "elicitationId": .string("connect"),
            ])
        ))

        XCTAssertEqual(m.pendingMcpElicitationRequests.count, 1)
        XCTAssertNil(m.pendingMcpElicitationForActiveThread)
        m.activeThreadId = "thread-b"
        let prompt = try! XCTUnwrap(m.pendingMcpElicitationForActiveThread)
        if case .url(let url) = prompt.mode {
            XCTAssertEqual(url, "https://example.com/connect")
        } else {
            XCTFail("expected URL elicitation")
        }
        m.handleNotification(
            method: "serverRequest/resolved",
            params: obj(["threadId": .string("thread-b"), "requestId": .number(11)]))
        XCTAssertTrue(m.pendingMcpElicitationRequests.isEmpty)
    }

    func testChatgptAuthRefreshRequestUsesManagedAuthError() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.loginInProgress = true
        m.handleServerRequest(AppServerClient.ServerRequest(
            id: .number(12),
            method: "account/chatgptAuthTokens/refresh",
            params: obj([
                "reason": .string("unauthorized"),
                "previousAccountId": .string("org-123"),
            ])
        ))

        XCTAssertFalse(m.loginInProgress)
        XCTAssertEqual(
            m.loginError,
            "CodeWith.app cannot provide external ChatGPT auth tokens for account org-123. It uses app-server managed login; sign in again if authentication expired.")
        XCTAssertTrue(m.activeMessages.isEmpty)
        XCTAssertTrue(m.pendingServerRequests.isEmpty)
        XCTAssertTrue(m.pendingUserInputRequests.isEmpty)
        XCTAssertTrue(m.pendingMcpElicitationRequests.isEmpty)
    }

    func testDynamicToolCallReturnsFailedContentResponse() {
        let response = AppModel.dynamicToolCallUnsupportedResponse(params: obj([
            "namespace": .string("codewith"),
            "tool": .string("lookup_ticket"),
        ]))

        XCTAssertEqual(response, obj([
            "contentItems": .array([
                obj([
                    "type": .string("inputText"),
                    "text": .string("CodeWith.app did not register codewith/lookup_ticket, so it cannot run this dynamic tool call."),
                ]),
            ]),
            "success": .bool(false),
        ]))
    }

    func testAttestationGenerateRequestDoesNotAppendUnsupportedChatMessage() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.handleServerRequest(AppServerClient.ServerRequest(
            id: .number(13),
            method: "attestation/generate",
            params: obj([:])
        ))

        XCTAssertTrue(m.activeMessages.isEmpty)
        XCTAssertTrue(m.pendingServerRequests.isEmpty)
        XCTAssertTrue(m.pendingUserInputRequests.isEmpty)
        XCTAssertTrue(m.pendingMcpElicitationRequests.isEmpty)
    }

    func testServerRequestResolutionRemovesInactiveThreadRequest() {
        let m = AppModel()
        m.activeThreadId = "thread-a"
        m.handleServerRequest(AppServerClient.ServerRequest(
            id: .number(4),
            method: "item/commandExecution/requestApproval",
            params: obj([
                "threadId": .string("thread-b"),
                "command": .string("make test"),
            ])
        ))
        m.handleNotification(
            method: "serverRequest/resolved",
            params: obj(["threadId": .string("thread-b"), "requestId": .number(4)]))
        XCTAssertTrue(m.pendingServerRequests.isEmpty)
    }

    func testClearPendingServerInteractionsClearsAllPromptQueues() {
        let m = AppModel()
        m.pendingServerRequests = [
            PendingServerRequest(
                requestId: .number(1),
                threadId: "thread-a",
                method: "item/commandExecution/requestApproval",
                kind: .commandApproval,
                title: "Command",
                detail: "make test"),
        ]
        m.pendingUserInputRequests = [
            PendingUserInputRequest(
                requestId: .number(2),
                threadId: "thread-a",
                title: "Input",
                questions: []),
        ]
        m.pendingMcpElicitationRequests = [
            PendingMcpElicitationRequest(
                requestId: .number(3),
                threadId: "thread-a",
                serverName: "mcp",
                title: "MCP",
                message: "Choose",
                mode: .form,
                fields: []),
        ]

        m.clearPendingServerInteractions()

        XCTAssertTrue(m.pendingServerRequests.isEmpty)
        XCTAssertTrue(m.pendingUserInputRequests.isEmpty)
        XCTAssertTrue(m.pendingMcpElicitationRequests.isEmpty)
    }

    func testGoalPlanUpdatedNotificationUpsertsActiveThreadPlan() {
        let m = AppModel()
        m.activeThreadId = "thread-a"

        m.handleNotification(method: "thread/goalPlan/updated", params: obj([
            "threadId": .string("thread-a"),
            "plan": obj([
                "planId": .string("plan-1"),
                "threadId": .string("thread-a"),
                "nodeCount": .number(1),
                "nodes": .array([
                    obj([
                        "nodeId": .string("node-1"),
                        "planId": .string("plan-1"),
                        "threadId": .string("thread-a"),
                        "key": .string("next"),
                        "objective": .string("Fix the next thing"),
                        "status": .string("pending"),
                        "ready": .bool(true),
                    ]),
                ]),
            ]),
        ]))

        XCTAssertEqual(m.activeGoalPlans.count, 1)
        XCTAssertEqual(m.activeGoalPlans.first?.planId, "plan-1")
        XCTAssertEqual(m.activeGoalPlans.first?.nodes.first?.nodeId, "node-1")
        XCTAssertEqual(m.activeGoalPlans.first?.nodes.first?.canActivate, true)
    }

    func testGoalPlanUpdatedNotificationIgnoresOtherThreads() {
        let m = AppModel()
        m.activeThreadId = "thread-a"

        m.handleNotification(method: "thread/goalPlan/updated", params: obj([
            "threadId": .string("thread-b"),
            "plan": obj([
                "planId": .string("plan-1"),
                "threadId": .string("thread-b"),
            ]),
        ]))

        XCTAssertTrue(m.activeGoalPlans.isEmpty)
    }

    // MARK: add menu

    func testToggleAddMenu() {
        let m = AppModel()
        m.toggleAddMenu(); XCTAssertTrue(m.showAddMenu)
        m.toggleAddMenu(); XCTAssertFalse(m.showAddMenu)
    }
    func testAddActionPlanMode() {
        let m = AppModel(); m.showAddMenu = true
        m.handleAddAction(.planMode)
        XCTAssertTrue(m.planMode); XCTAssertFalse(m.showAddMenu)
    }
    func testAddActionGoalPrefixesOnce() {
        let m = AppModel(); m.composerText = "ship it"
        m.handleAddAction(.goal); XCTAssertEqual(m.composerText, "Goal: ship it")
        m.handleAddAction(.goal); XCTAssertEqual(m.composerText, "Goal: ship it")
    }
    func testGoalObjectiveStripsExistingPrefix() {
        XCTAssertEqual(AppModel.goalObjective(from: "Goal: ship it"), "ship it")
        XCTAssertEqual(AppModel.goalObjective(from: "  goal: ship it  "), "ship it")
        XCTAssertEqual(AppModel.goalObjective(from: "ship it"), "ship it")
        XCTAssertEqual(AppModel.goalObjective(from: "Goal:   "), "")
    }
    func testIsGoalCommandIgnoresLeadingWhitespaceAndCase() {
        XCTAssertTrue(AppModel.isGoalCommand("Goal: ship it"))
        XCTAssertTrue(AppModel.isGoalCommand("  goal: ship it"))
        XCTAssertFalse(AppModel.isGoalCommand("ship it"))
    }
    func testLoopPromptStripsExistingPrefix() {
        XCTAssertEqual(AppModel.loopPrompt(from: "Loop: check CI"), "check CI")
        XCTAssertEqual(AppModel.loopPrompt(from: "  loop: check CI  "), "check CI")
        XCTAssertEqual(AppModel.loopPrompt(from: "check CI"), "check CI")
        XCTAssertEqual(AppModel.loopPrompt(from: "Loop:   "), "")
    }
    func testIsLoopCommandIgnoresLeadingWhitespaceAndCase() {
        XCTAssertTrue(AppModel.isLoopCommand("Loop: check CI"))
        XCTAssertTrue(AppModel.isLoopCommand("  loop: check CI"))
        XCTAssertFalse(AppModel.isLoopCommand("check CI"))
    }
    func testEmptyGoalCommandDoesNotStartTurn() async {
        let m = AppModel()
        m.connection = .connected
        m.composerText = "Goal:   "
        await m.submitComposer()
        XCTAssertEqual(m.composerText, "Goal: ")
        XCTAssertFalse(m.turnInProgress)
        XCTAssertTrue(m.activeMessages.isEmpty)
        XCTAssertNil(m.activeThreadId)
    }
    func testEmptyLoopCommandDoesNotStartTurn() async {
        let m = AppModel()
        m.connection = .connected
        m.composerText = "Loop:   "
        await m.submitComposer()
        XCTAssertEqual(m.composerText, "Loop: ")
        XCTAssertFalse(m.turnInProgress)
        XCTAssertTrue(m.activeMessages.isEmpty)
        XCTAssertNil(m.activeThreadId)
    }
    func testActivePeerMessageStripsMentionPrefix() {
        let peer = ActiveSessionPeerInfo(from: obj([
            "peerId": .string("peer-1"),
            "threadId": .string("thread-1"),
            "displayName": .string("Reviewer"),
            "capabilities": .array([.string("receiveMessage")]),
        ]))
        XCTAssertEqual(AppModel.activePeerMessage(from: "@Reviewer check this", peer: peer), "check this")
        XCTAssertEqual(AppModel.activePeerMessage(from: "@Reviewer   ", peer: peer), "")
        XCTAssertEqual(AppModel.activePeerMessage(from: "check this", peer: peer), "check this")
    }
    func testEmptyPendingActivePeerDoesNotStartNormalTurn() async {
        let m = AppModel()
        m.connection = .connected
        let peer = ActiveSessionPeerInfo(from: obj([
            "peerId": .string("peer-1"),
            "threadId": .string("thread-1"),
            "displayName": .string("Reviewer"),
            "capabilities": .array([.string("receiveMessage")]),
        ]))
        await m.sendComposerToActivePeer(peer)
        XCTAssertEqual(m.composerText, "@Reviewer ")
        await m.submitComposer()
        XCTAssertEqual(m.composerText, "@Reviewer ")
        XCTAssertFalse(m.turnInProgress)
        XCTAssertTrue(m.activeMessages.isEmpty)
        XCTAssertNil(m.activeThreadId)
    }
    func testCreateDefaultLoopWithoutActiveThreadDoesNotCreateThread() async {
        let m = AppModel()
        m.connection = .connected
        await m.createDefaultLoop()
        XCTAssertNil(m.activeThreadId)
        XCTAssertEqual(m.composerText, "Loop: Continue this thread and report anything that needs attention.")
    }
    func testAttachGhosttyDoesNotCreateFakeAgentMention() {
        let m = AppModel(); m.composerText = "review"
        m.handleAddAction(.attachGhostty)
        XCTAssertEqual(m.composerText, "review")
        XCTAssertFalse(m.showAddMenu)
    }
    func testAddMenuAgentRunsExcludesDeletedRunsAndActivePeerThreads() {
        let m = AppModel()
        m.activePeers = [
            ActiveSessionPeerInfo(from: obj([
                "peerId": .string("peer-1"),
                "threadId": .string("thread-active"),
                "capabilities": .array([.string("receiveMessage")]),
            ])),
        ]
        m.agentRuns = [
            AgentRunInfo(from: obj([
                "agentId": .string("agent-visible"),
                "threadId": .string("thread-visible"),
                "status": .string("running"),
            ])),
            AgentRunInfo(from: obj([
                "agentId": .string("agent-duplicate"),
                "threadId": .string("thread-active"),
                "status": .string("running"),
            ])),
            AgentRunInfo(from: obj([
                "agentId": .string("agent-deleted"),
                "threadId": .string("thread-deleted"),
                "desiredState": .string("deleted"),
            ])),
            AgentRunInfo(from: obj([
                "agentId": .string("agent-detached"),
                "status": .string("running"),
            ])),
        ]
        XCTAssertEqual(m.addMenuAgentRuns.map(\.agentId), ["agent-visible"])
    }
    func testConfigSetters() {
        let m = AppModel()
        m.setModel("o3"); m.setProvider("azure"); m.setEffort("High")
        XCTAssertEqual(m.model, "o3"); XCTAssertEqual(m.provider, "azure"); XCTAssertEqual(m.effort, "High")
    }
    func testThreadSettingsUpdatedNotificationUpdatesSessionSelectors() {
        let m = AppModel()
        m.activeThreadId = "thread-1"
        m.model = "old-model"
        m.provider = "openai"
        m.effort = "Low"

        m.handleNotification(
            method: "thread/settings/updated",
            params: obj([
                "threadId": .string("thread-1"),
                "threadSettings": obj([
                    "model": .string("gpt-5.5"),
                    "modelProvider": .string("openrouter"),
                    "effort": .string("medium"),
                ]),
            ]))

        XCTAssertEqual(m.model, "gpt-5.5")
        XCTAssertEqual(m.provider, "openrouter")
        XCTAssertEqual(m.effort, "Medium")
    }
    func testThreadSettingsUpdatedNotificationIgnoresOtherThreads() {
        let m = AppModel()
        m.activeThreadId = "thread-1"
        m.model = "old-model"

        m.handleNotification(
            method: "thread/settings/updated",
            params: obj([
                "threadId": .string("thread-2"),
                "threadSettings": obj(["model": .string("gpt-5.5")]),
            ]))

        XCTAssertEqual(m.model, "old-model")
    }
    func testConfigTomlPathPrefersServerCodewithHome() {
        XCTAssertEqual(
            AppModel.configTomlPath(
                serverCodewithHome: "/server/home",
                environmentCodewithHome: "/env/home",
                homeDirectory: "/Users/me"),
            "/server/home/config.toml")
        XCTAssertEqual(
            AppModel.configTomlPath(
                serverCodewithHome: "",
                environmentCodewithHome: "/env/home",
                homeDirectory: "/Users/me"),
            "/env/home/config.toml")
        XCTAssertEqual(
            AppModel.configTomlPath(
                serverCodewithHome: nil,
                environmentCodewithHome: "",
                homeDirectory: "/Users/me"),
            "/Users/me/.codewith/config.toml")
    }
    func testFullAccessToggleRestoresPreviousNonFullSandbox() {
        let m = AppModel()
        m.configSandbox = "read-only"
        m.setFullAccess(true)
        XCTAssertEqual(m.configSandbox, "danger-full-access")
        m.setFullAccess(false)
        XCTAssertEqual(m.configSandbox, "read-only")
    }
    func testManagedRequirementsBlockDisallowedSandboxAndApprovalWrites() {
        let m = AppModel()
        m.configRequirements = ConfigRequirementsInfo(from: obj([
            "allowedApprovalPolicies": .array([.string("on-request")]),
            "allowedSandboxModes": .array([.string("read-only")]),
        ]))
        m.configSandbox = "read-only"
        m.fullAccess = false

        m.setApproval("never")
        XCTAssertEqual(m.configApproval, nil)
        XCTAssertEqual(m.configError, "Approval policy never is blocked by managed requirements.")

        m.setSandbox("danger-full-access")
        XCTAssertEqual(m.configSandbox, "read-only")
        XCTAssertEqual(m.configError, "Sandbox mode danger-full-access is blocked by managed requirements.")

        m.setFullAccess(true)
        XCTAssertFalse(m.fullAccess)
        XCTAssertEqual(m.configSandbox, "read-only")
        XCTAssertEqual(m.configError, "Full access is blocked by managed requirements.")
    }
    func testProviderIDMapping() {
        XCTAssertEqual(AppModel.providerID(for: "OpenAI"), "openai")
        XCTAssertEqual(AppModel.providerID(for: "OpenRouter"), "openrouter")
        XCTAssertEqual(AppModel.providerID(for: "Anthropic"), "anthropic")
        XCTAssertEqual(AppModel.providerID(for: "Azure"), "azure")
        XCTAssertEqual(AppModel.providerID(for: "Ollama"), "ollama")
    }
    func testLoginURLPrefersAuthURLAndFallsBackToVerificationURL() {
        XCTAssertEqual(
            AppModel.loginURL(from: obj([
                "authUrl": .string("https://example.com/auth"),
                "verificationUrl": .string("https://example.com/verify"),
            ]))?.absoluteString,
            "https://example.com/auth")
        XCTAssertEqual(
            AppModel.loginURL(from: obj([
                "verificationUrl": .string("https://example.com/verify"),
            ]))?.absoluteString,
            "https://example.com/verify")
        XCTAssertNil(AppModel.loginURL(from: obj([:])))
    }

    func testPermissionProfileMappingFromSandbox() {
        XCTAssertEqual(AppModel.permissionProfileId(forSandbox: "danger-full-access"), ":danger-full-access")
        XCTAssertEqual(AppModel.permissionProfileId(forSandbox: "read-only"), ":read-only")
        XCTAssertEqual(AppModel.permissionProfileId(forSandbox: "workspace-write"), ":workspace")
        XCTAssertEqual(AppModel.permissionProfileId(forSandbox: nil), ":workspace")
    }

    func testMachineScopedThreadsFallsBackWhenThreadMetadataHasNoMachineIds() {
        let m = AppModel()
        m.threads = [
            ThreadInfo(from: obj(["id": .string("t1"), "cwd": .string("/a")])),
            ThreadInfo(from: obj(["id": .string("t2"), "cwd": .string("/b")])),
        ]
        m.selectedMachineId = "machine-a"
        XCTAssertEqual(m.machineScopedThreads.map(\.id), ["t1", "t2"])
    }

    func testMachineScopedThreadsFilterWhenMetadataHasMachineIds() {
        let m = AppModel()
        m.threads = [
            ThreadInfo(from: obj(["id": .string("t1"), "cwd": .string("/a"), "machineId": .string("machine-a")])),
            ThreadInfo(from: obj(["id": .string("t2"), "cwd": .string("/b"), "machineId": .string("machine-b")])),
        ]
        m.selectedMachineId = "machine-a"
        XCTAssertEqual(m.machineScopedThreads.map(\.id), ["t1"])
    }

    func testLocalMachineScopeIncludesLegacyThreadsWhenMetadataIsMixed() {
        let m = AppModel()
        m.machines = [
            MachineInfo(id: "local", os: "macos", status: "online", role: "local", isLocal: true),
            MachineInfo(id: "remote", os: "linux", status: "online", role: "remote", isLocal: false),
        ]
        m.threads = [
            ThreadInfo(from: obj(["id": .string("legacy"), "cwd": .string("/legacy")])),
            ThreadInfo(from: obj(["id": .string("local-thread"), "cwd": .string("/local"), "machineId": .string("local")])),
            ThreadInfo(from: obj(["id": .string("remote-thread"), "cwd": .string("/remote"), "machineId": .string("remote")])),
        ]
        m.selectedMachineId = "local"
        XCTAssertEqual(m.machineScopedThreads.map(\.id), ["legacy", "local-thread"])
    }

    func testSelectingDifferentMachineClearsActiveSessionAndSearch() {
        let m = AppModel()
        m.machines = [
            MachineInfo(id: "a", os: "macos", status: "online", role: "local", isLocal: true),
            MachineInfo(id: "b", os: "linux", status: "online", role: "remote", isLocal: false),
        ]
        m.selectedMachineId = "a"
        m.activeThreadId = "thread-a"
        m.activeMessages = [ChatMessage(role: .assistant, text: "old")]
        m.route = .chat("thread-a")
        m.remoteSearchThreads = [ThreadInfo(from: obj(["id": .string("thread-a"), "machineId": .string("a")]))]

        m.selectMachine(m.machines[1])

        XCTAssertEqual(m.selectedMachineId, "b")
        XCTAssertNil(m.activeThreadId)
        XCTAssertTrue(m.activeMessages.isEmpty)
        XCTAssertTrue(m.remoteSearchThreads.isEmpty)
        XCTAssertEqual(m.route, .home)
    }

    func testSelectingDifferentMachineBlockedWithPendingPrompt() {
        let m = AppModel()
        m.machines = [
            MachineInfo(id: "a", os: "macos", status: "online", role: "local", isLocal: true),
            MachineInfo(id: "b", os: "linux", status: "online", role: "remote", isLocal: false),
        ]
        m.selectedMachineId = "a"
        m.activeThreadId = "thread-a"
        m.pendingServerRequests = [
            PendingServerRequest(
                requestId: .number(1),
                threadId: "thread-a",
                method: "serverRequest/permissions",
                kind: .permissionsApproval,
                title: "Approve?",
                detail: "Approve?")
        ]

        m.selectMachine(m.machines[1])

        XCTAssertEqual(m.selectedMachineId, "a")
        XCTAssertEqual(m.activeThreadId, "thread-a")
        XCTAssertEqual(m.pendingServerRequests.count, 1)
        XCTAssertEqual(m.pendingMachineSwitchWarning, "Resolve the pending request before switching machines.")
    }

    func testSelectingMachineClearsProjectContext() {
        let m = AppModel()
        m.currentProjectPath = "/old/project"
        m.selectMachine(MachineInfo(id: "machine-a", os: "macos", status: "online", role: "local", isLocal: true))
        XCTAssertNil(m.currentProjectPath)
        XCTAssertEqual(m.selectedMachineId, "machine-a")
    }

    func testSubmitNoOpWhenNotConnected() async {
        let m = AppModel()   // connection == .connecting
        m.composerText = "hello"
        await m.submitComposer()
        XCTAssertTrue(m.activeMessages.isEmpty)
        XCTAssertEqual(m.composerText, "hello")
    }

    func testPrepareLoopComposerDoesNotCreateLoopImmediately() {
        let m = AppModel()
        m.prepareLoopComposer()
        XCTAssertEqual(m.route, .home)
        XCTAssertEqual(m.sidebarSelection, "New loop")
        XCTAssertTrue(m.composerText.hasPrefix("Loop: "))
        XCTAssertTrue(m.loops.isEmpty)
    }

    func testMcpElicitationValueParsesTypedFields() {
        let integerField = PendingMcpElicitationField(
            id: "count",
            label: "Count",
            prompt: "Count",
            required: true,
            kind: .integer)
        let numberField = PendingMcpElicitationField(
            id: "ratio",
            label: "Ratio",
            prompt: "Ratio",
            required: true,
            kind: .number)
        let textField = PendingMcpElicitationField(
            id: "name",
            label: "Name",
            prompt: "Name",
            required: true,
            kind: .text)

        XCTAssertEqual(AppModel.mcpElicitationValue(field: integerField, rawValue: "3"), .number(3))
        XCTAssertEqual(AppModel.mcpElicitationValue(field: numberField, rawValue: "3.5"), .number(3.5))
        XCTAssertEqual(AppModel.mcpElicitationValue(field: textField, rawValue: "3"), .string("3"))
        XCTAssertNil(AppModel.mcpElicitationValue(field: integerField, rawValue: "3.5"))
    }

    func testSampleModelHasData() {
        let m = AppModel.sample()
        XCTAssertEqual(m.connection, .connected)
        XCTAssertFalse(m.threads.isEmpty)
        XCTAssertFalse(m.projects.isEmpty)
        XCTAssertFalse(m.loops.isEmpty)
        XCTAssertFalse(m.goalStates.isEmpty)
        XCTAssertFalse(m.workflows.isEmpty)
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

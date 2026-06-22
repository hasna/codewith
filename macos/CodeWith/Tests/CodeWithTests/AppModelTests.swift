import XCTest
@testable import CodeWith

@MainActor
final class AppModelTests: XCTestCase {
    func testStartsAtHome() {
        let m = AppModel()
        XCTAssertEqual(m.route, .home)
        XCTAssertEqual(m.sidebarSelection, "New chat")
        XCTAssertFalse(m.showSettings)
    }

    func testSeedData() {
        let m = AppModel()
        XCTAssertEqual(m.projects.first?.name, "scaffold-api")
        XCTAssertEqual(m.projects.first?.tasks.count, 5)
        XCTAssertEqual(m.chats.count, 2)
        XCTAssertEqual(m.profiles.count, 3)
        XCTAssertEqual(m.currentProfile.initials, "AH")
    }

    func testOpenRouteUpdatesSelection() {
        let m = AppModel()
        m.open(.machines, label: "Machines")
        XCTAssertEqual(m.route, .machines)
        XCTAssertEqual(m.sidebarSelection, "Machines")
        XCTAssertFalse(m.showSettings)
    }

    func testOpenSettingsAndBack() {
        let m = AppModel()
        m.openSettings("Appearance")
        XCTAssertTrue(m.showSettings)
        XCTAssertEqual(m.settingsPage, "Appearance")
        m.showSettings = false
        XCTAssertFalse(m.showSettings)
    }

    func testSubmitComposerCreatesChatAndNavigates() {
        let m = AppModel()
        let before = m.chats.count
        m.composerText = "Refactor the auth module"
        m.submitComposer()
        XCTAssertEqual(m.chats.count, before + 1)
        XCTAssertEqual(m.chats.first?.messages.first?.text, "Refactor the auth module")
        XCTAssertEqual(m.chats.first?.messages.first?.role, .user)
        if case .chat(let id) = m.route {
            XCTAssertEqual(id, m.chats.first?.id)
        } else {
            XCTFail("expected chat route after submit")
        }
        XCTAssertTrue(m.composerText.isEmpty, "composer should clear after submit")
    }

    func testSubmitEmptyComposerIsNoOp() {
        let m = AppModel()
        let before = m.chats.count
        m.composerText = "   "
        m.submitComposer()
        XCTAssertEqual(m.chats.count, before)
    }

    func testNewChatResetsToHome() {
        let m = AppModel()
        m.open(.apps, label: "Apps")
        m.composerText = "leftover"
        m.newChat()
        XCTAssertEqual(m.route, .home)
        XCTAssertEqual(m.sidebarSelection, "New chat")
        XCTAssertTrue(m.composerText.isEmpty)
    }

    func testSwitchProfile() {
        let m = AppModel()
        let work = m.profiles[1]
        m.switchProfile(work.id)
        XCTAssertEqual(m.currentProfile.id, work.id)
        XCTAssertEqual(m.currentProfile.name, "Work")
    }

    func testSubmitComposerAddsSimulatedReply() {
        let m = AppModel()
        m.composerText = "Add a feature"
        m.submitComposer()
        let msgs = m.chats.first?.messages ?? []
        XCTAssertGreaterThan(msgs.count, 1, "chat should include a simulated assistant reply")
        XCTAssertTrue(msgs.contains { $0.role == .assistant })
        XCTAssertTrue(msgs.contains { $0.role == .tool })
    }

    func testAddProject() {
        let m = AppModel()
        let before = m.projects.count
        m.addProject(name: "  my-new-repo  ")
        XCTAssertEqual(m.projects.count, before + 1)
        XCTAssertEqual(m.projects.first?.name, "my-new-repo")
        XCTAssertEqual(m.sidebarSelection, "my-new-repo")
    }

    func testAddProjectIgnoresEmpty() {
        let m = AppModel()
        let before = m.projects.count
        m.addProject(name: "   ")
        XCTAssertEqual(m.projects.count, before)
    }

    func testAddTaskToProject() {
        let m = AppModel()
        m.addTask("Wire up OAuth", toProjectNamed: "scaffold-api")
        XCTAssertEqual(m.projects.first { $0.name == "scaffold-api" }?.tasks.first?.title, "Wire up OAuth")
    }

    func testToggleAddMenu() {
        let m = AppModel()
        XCTAssertFalse(m.showAddMenu)
        m.toggleAddMenu()
        XCTAssertTrue(m.showAddMenu)
        m.toggleAddMenu()
        XCTAssertFalse(m.showAddMenu)
    }

    func testSetPlanModeClosesMenu() {
        let m = AppModel()
        m.showAddMenu = true
        m.setPlanMode(true)
        XCTAssertTrue(m.planMode)
        XCTAssertFalse(m.showAddMenu)
    }

    func testGoalPrefixesComposer() {
        let m = AppModel()
        m.composerText = "ship the release"
        m.setGoalFromComposer()
        XCTAssertEqual(m.composerText, "Goal: ship the release")
        XCTAssertFalse(m.showAddMenu)
    }

    func testHandleAddActionPlanMode() {
        let m = AppModel()
        m.showAddMenu = true
        m.handleAddAction("Plan mode")
        XCTAssertTrue(m.planMode)
        XCTAssertFalse(m.showAddMenu)
    }

    func testHandleAddActionGoal() {
        let m = AppModel()
        m.composerText = "fix the bug"
        m.handleAddAction("Goal")
        XCTAssertEqual(m.composerText, "Goal: fix the bug")
    }

    func testHandleAddActionAgentMention() {
        let m = AppModel()
        m.showAddMenu = true
        m.composerText = "review this"
        m.handleAddAction("Apollo")
        XCTAssertEqual(m.composerText, "@Apollo review this")
        XCTAssertFalse(m.showAddMenu)
    }

    func testAgentRunnerClassifyAuthError() {
        let out = "ERROR: Reconnecting... 5/5\n401 Unauthorized: Missing bearer or basic authentication"
        if case .notAuthenticated = AgentRunner.classify(exitCode: 1, output: out) {} else {
            XCTFail("401 output should classify as notAuthenticated")
        }
    }

    func testAgentRunnerClassifyReply() {
        if case .reply(let t) = AgentRunner.classify(exitCode: 0, output: "  Done refactoring the module.\n") {
            XCTAssertEqual(t, "Done refactoring the module.")
        } else {
            XCTFail("clean exit with output should classify as reply")
        }
    }

    func testOpenChatNavigates() {
        let m = AppModel()
        let chat = m.chats[0]
        m.openChat(chat)
        XCTAssertEqual(m.route, .chat(chat.id))
        XCTAssertEqual(m.sidebarSelection, chat.title)
    }
}

use super::*;
use crate::pull_request_summary::PullRequestCheckSummary;
use crate::pull_request_summary::PullRequestOverview;
use crate::pull_request_summary::PullRequestQuery;
use crate::pull_request_summary::PullRequestQueryStatus;
use crate::pull_request_summary::PullRequestSummary;

#[tokio::test]
async fn pull_request_overview_current_and_open_tabs_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    show_test_overview(&mut chat, loaded_overview());

    assert_chatwidget_snapshot!(
        "pull_request_overview_current",
        render_bottom_popup(&chat, /*width*/ 120)
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Right));
    assert_chatwidget_snapshot!(
        "pull_request_overview_open",
        render_bottom_popup(&chat, /*width*/ 120)
    );
}

#[tokio::test]
async fn pull_request_overview_missing_gh_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    show_test_overview(
        &mut chat,
        PullRequestOverview {
            cwd: PathBuf::from("/tmp/project"),
            current: status(PullRequestQueryStatus::GhUnavailable(
                "program not found".to_string(),
            )),
            open: status(PullRequestQueryStatus::GhUnavailable(
                "program not found".to_string(),
            )),
        },
    );

    assert_chatwidget_snapshot!(
        "pull_request_overview_missing_gh",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn pull_request_overview_auth_required_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    show_test_overview(
        &mut chat,
        PullRequestOverview {
            cwd: PathBuf::from("/tmp/project"),
            current: status(PullRequestQueryStatus::AuthRequired(
                "To get started with GitHub CLI, run: gh auth login".to_string(),
            )),
            open: status(PullRequestQueryStatus::AuthRequired(
                "To get started with GitHub CLI, run: gh auth login".to_string(),
            )),
        },
    );

    assert_chatwidget_snapshot!(
        "pull_request_overview_auth_required",
        render_bottom_popup(&chat, /*width*/ 100)
    );
}

#[tokio::test]
async fn pull_request_overview_no_current_narrow_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    show_test_overview(
        &mut chat,
        PullRequestOverview {
            cwd: PathBuf::from("/tmp/project"),
            current: status(PullRequestQueryStatus::NoCurrentPullRequest),
            open: ready(Vec::new()),
        },
    );

    assert_chatwidget_snapshot!(
        "pull_request_overview_no_current_narrow",
        render_bottom_popup(&chat, /*width*/ 60)
    );
}

#[tokio::test]
async fn pull_request_overview_enter_opens_browser_only() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    show_test_overview(&mut chat, loaded_overview());
    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenUrlInBrowser { url }) if url == "https://github.com/hasna/codewith/pull/42"
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn pull_request_overview_loaded_replaces_loading_popup() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.open_pull_request_overview();
    let loading = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        loading.contains("Loading GitHub pull requests"),
        "expected loading popup, got:\n{loading}"
    );

    let (request_id, overview) = next_pull_request_overview_loaded(&mut rx).await;
    chat.show_pull_request_overview(request_id, overview);
    let loaded = render_bottom_popup(&chat, /*width*/ 100);
    assert!(
        loaded.contains("Workspace commands unavailable"),
        "expected loaded popup, got:\n{loaded}"
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Esc));
    assert!(chat.no_modal_or_popup_active());
}

#[tokio::test]
async fn pull_request_overview_dismissed_before_load_does_not_reopen() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.open_pull_request_overview();
    chat.handle_key_event(KeyEvent::from(KeyCode::Esc));
    assert!(chat.no_modal_or_popup_active());

    let (request_id, overview) = next_pull_request_overview_loaded(&mut rx).await;
    chat.show_pull_request_overview(request_id, overview);

    assert!(chat.no_modal_or_popup_active());
}

async fn next_pull_request_overview_loaded(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) -> (u64, PullRequestOverview) {
    loop {
        if let AppEvent::PullRequestOverviewLoaded {
            request_id,
            overview,
        } = rx.recv().await.expect("expected app event")
        {
            return (request_id, overview);
        }
    }
}

fn show_test_overview(chat: &mut ChatWidget, overview: PullRequestOverview) {
    chat.pull_request_overview_request_id = 1;
    seed_pull_request_loading_view(chat);
    chat.show_pull_request_overview(/*request_id*/ 1, overview);
}

fn seed_pull_request_loading_view(chat: &mut ChatWidget) {
    chat.show_selection_view(SelectionViewParams {
        view_id: Some("pull_request_overview"),
        title: Some("Pull Requests".to_string()),
        subtitle: Some("Loading GitHub pull requests".to_string()),
        items: vec![SelectionItem {
            name: "Loading".to_string(),
            is_disabled: true,
            ..Default::default()
        }],
        ..Default::default()
    });
}

fn loaded_overview() -> PullRequestOverview {
    PullRequestOverview {
        cwd: PathBuf::from("/tmp/project"),
        current: ready(vec![test_pull_request(
            42,
            "Add native pull request overview",
            "open-pr-overview",
            checks(
                /*total*/ 4, /*success*/ 3, /*failing*/ 0, /*pending*/ 1,
            ),
        )]),
        open: ready(vec![
            test_pull_request(
                42,
                "Add native pull request overview",
                "open-pr-overview",
                checks(
                    /*total*/ 4, /*success*/ 3, /*failing*/ 0, /*pending*/ 1,
                ),
            ),
            PullRequestSummary {
                number: 51,
                title: "Harden PR follow-up actions".to_string(),
                url: "https://github.com/hasna/codewith/pull/51".to_string(),
                state: "OPEN".to_string(),
                is_draft: true,
                author: Some("reviewer".to_string()),
                head_ref: "approval-gated-pr-actions".to_string(),
                base_ref: "main".to_string(),
                review_decision: Some("CHANGES_REQUESTED".to_string()),
                merge_state: Some("DIRTY".to_string()),
                checks: checks(
                    /*total*/ 3, /*success*/ 1, /*failing*/ 1, /*pending*/ 1,
                ),
            },
        ]),
    }
}

fn test_pull_request(
    number: u64,
    title: &str,
    head_ref: &str,
    checks: PullRequestCheckSummary,
) -> PullRequestSummary {
    PullRequestSummary {
        number,
        title: title.to_string(),
        url: format!("https://github.com/hasna/codewith/pull/{number}"),
        state: "OPEN".to_string(),
        is_draft: false,
        author: Some("hasna".to_string()),
        head_ref: head_ref.to_string(),
        base_ref: "main".to_string(),
        review_decision: Some("APPROVED".to_string()),
        merge_state: Some("CLEAN".to_string()),
        checks,
    }
}

fn ready(items: Vec<PullRequestSummary>) -> PullRequestQuery {
    PullRequestQuery {
        status: PullRequestQueryStatus::Ready,
        items,
    }
}

fn status(status: PullRequestQueryStatus) -> PullRequestQuery {
    PullRequestQuery {
        status,
        items: Vec::new(),
    }
}

fn checks(total: u64, success: u64, failing: u64, pending: u64) -> PullRequestCheckSummary {
    PullRequestCheckSummary {
        total,
        success,
        failing,
        pending,
        skipped: 0,
        unknown: 0,
    }
}

use super::*;
use pretty_assertions::assert_eq;

fn next_override_turn_context(op_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Op>) -> Op {
    loop {
        match op_rx.try_recv() {
            Ok(op @ Op::OverrideTurnContext { .. }) => return op,
            Ok(_) => continue,
            Err(TryRecvError::Empty) => panic!("expected override op but queue was empty"),
            Err(TryRecvError::Disconnected) => panic!("expected override op but channel closed"),
        }
    }
}

#[tokio::test]
async fn prompt_slash_sets_and_clears_session_prompt() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());

    chat.bottom_pane
        .set_composer_text("/prompt  be concise  ".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_eq!(chat.current_session_prompt(), Some("be concise"));
    match next_override_turn_context(&mut op_rx) {
        Op::OverrideTurnContext {
            session_prompt: Some(Some(prompt)),
            ..
        } => assert_eq!(prompt, "be concise"),
        other => panic!("expected session prompt set override, got {other:?}"),
    }

    chat.bottom_pane
        .set_composer_text("/prompt clear".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_eq!(chat.current_session_prompt(), None);
    match next_override_turn_context(&mut op_rx) {
        Op::OverrideTurnContext {
            session_prompt: Some(None),
            ..
        } => {}
        other => panic!("expected session prompt clear override, got {other:?}"),
    }
}

#[tokio::test]
async fn prompt_slash_show_does_not_submit_override() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.set_session_prompt_from_settings(Some("Prefer direct answers.".to_string()));

    chat.bottom_pane
        .set_composer_text("/prompt show".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert!(op_rx.try_recv().is_err());
    assert_eq!(
        chat.current_session_prompt(),
        Some("Prefer direct answers.")
    );
}

#[tokio::test]
async fn prompt_editor_snapshot_prefills_existing_prompt() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.set_session_prompt_from_settings(Some(
        "Prefer direct answers.\nCall out risky assumptions.".to_string(),
    ));

    chat.dispatch_command(SlashCommand::Prompt);

    let popup = render_bottom_popup(&chat, /*width*/ 84);
    assert_chatwidget_snapshot!("session_prompt_editor", popup);
}

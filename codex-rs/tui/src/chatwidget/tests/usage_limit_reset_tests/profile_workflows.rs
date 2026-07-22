use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn profile_login_continuations_preserve_originating_reset_generation() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let reset_generation = 17;

    chat.open_auth_profile_login_prompt(reset_generation);
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    // ChatGPT supports multiple login methods, so selecting it opens the method
    // chooser rather than jumping straight to the name prompt.
    let subscription_provider = match rx.try_recv() {
        Ok(AppEvent::OpenAuthProfileMethodPrompt {
            subscription_provider,
            reset_generation: event_generation,
        }) => {
            assert_eq!(event_generation, reset_generation);
            subscription_provider
        }
        event => panic!("expected profile method prompt, got {event:?}"),
    };

    chat.open_auth_profile_method_prompt(subscription_provider, reset_generation);
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let login_method = match rx.try_recv() {
        Ok(AppEvent::OpenAuthProfileNamePrompt {
            subscription_provider: event_provider,
            login_method,
            reset_generation: event_generation,
        }) => {
            assert_eq!(event_generation, reset_generation);
            assert_eq!(event_provider, subscription_provider);
            login_method
        }
        event => panic!("expected profile name prompt, got {event:?}"),
    };

    chat.open_auth_profile_name_prompt(subscription_provider, login_method, reset_generation);
    chat.bottom_pane.set_disable_paste_burst(/*disabled*/ true);
    for ch in "work".chars() {
        chat.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::LoginNewAuthProfile {
            profile,
            subscription_provider: event_provider,
            login_method: event_method,
            reset_generation: event_generation,
        }) if profile == "work"
            && event_provider == subscription_provider
            && event_method == login_method
            && event_generation == reset_generation
    );

    chat.start_auth_profile_login(
        "external-work".to_string(),
        codex_login::AuthProfileSubscriptionProvider::Cursor,
        codex_login::AuthProfileLoginMethod::SubscriptionLogin,
        reset_generation,
    );
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::AuthProfileLoginCompleted {
            profile,
            success: true,
            error: None,
            reset_generation: event_generation,
        }) if profile == "external-work" && event_generation == reset_generation
    );
}

#[tokio::test]
async fn profile_rename_and_delete_continuations_preserve_originating_reset_generation() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let reset_generation = 23;

    chat.open_auth_profile_rename_prompt("work".to_string(), reset_generation);
    chat.bottom_pane.set_disable_paste_burst(/*disabled*/ true);
    for ch in "-renamed".chars() {
        chat.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::RenameAuthProfile {
            old_name,
            new_name,
            reset_generation: event_generation,
        }) if old_name == "work"
            && new_name == "work-renamed"
            && event_generation == reset_generation
    );

    chat.open_auth_profile_delete_confirm("work".to_string(), reset_generation);
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::DeleteAuthProfile {
            profile,
            reset_generation: event_generation,
        }) if profile == "work" && event_generation == reset_generation
    );
}

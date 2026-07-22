//! Paste-burst integration tests for [`ChatComposer`].
//!
//! Extracted from `chat_composer.rs` to keep the composer orchestration module
//! focused. These exercise the paste-burst state-machine integration surface
//! (`paste_burst.rs` owns the pure state machine) via the full composer.

use super::*;
use crate::app_event::AppEvent;
use crate::bottom_pane::AppEventSender;
use crate::bottom_pane::ChatComposer;
use crate::bottom_pane::InputResult;
use crate::bottom_pane::chat_composer::LARGE_PASTE_CHAR_THRESHOLD;
use pretty_assertions::assert_eq;
use std::path::PathBuf;
use tokio::sync::mpsc::unbounded_channel;

fn flush_after_paste_burst(composer: &mut ChatComposer) -> bool {
    std::thread::sleep(PasteBurst::recommended_active_flush_delay());
    composer.flush_paste_burst_if_due()
}

// Test helper: simulate human typing with a brief delay and flush the paste-burst buffer
fn type_chars_humanlike(composer: &mut ChatComposer, chars: &[char]) {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyEventKind;
    use crossterm::event::KeyModifiers;
    for &ch in chars {
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        std::thread::sleep(ChatComposer::recommended_paste_flush_delay());
        let _ = composer.flush_paste_burst_if_due();
        if ch == ' ' {
            let _ = composer.handle_key_event(KeyEvent::new_with_kind(
                KeyCode::Char(' '),
                KeyModifiers::NONE,
                KeyEventKind::Release,
            ));
        }
    }
}

#[test]
fn esc_keeps_shell_mode_when_paste_burst_flushes_pending_text() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    type_chars_humanlike(&mut composer, &['!']);
    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
    assert!(composer.is_in_paste_burst());
    assert_eq!(composer.current_text(), "!");

    let (result, needs_redraw) =
        composer.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    assert!(matches!(result, InputResult::None));
    assert!(needs_redraw);
    assert!(composer.draft.is_bash_mode);
    assert_eq!(composer.current_text(), "!g");
}

#[test]
fn clear_for_ctrl_c_preserves_pending_paste_history_entry() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let large = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5);
    composer.handle_paste(large.clone());
    let char_count = large.chars().count();
    let placeholder = format!("[Pasted Content {char_count} chars]");
    assert_eq!(composer.draft.textarea.text(), placeholder);
    assert_eq!(
        composer.draft.pending_pastes,
        vec![(placeholder.clone(), large.clone())]
    );

    composer.clear_for_ctrl_c();
    assert!(composer.is_empty());

    let history_entry = composer
        .history
        .navigate_up(&composer.app_event_tx)
        .expect("expected history entry");
    let text_elements = vec![TextElement::new(
        (0..placeholder.len()).into(),
        Some(placeholder.clone()),
    )];
    assert_eq!(
        history_entry,
        HistoryEntry::with_pending(
            placeholder.clone(),
            text_elements,
            Vec::new(),
            vec![(placeholder.clone(), large.clone())]
        )
    );

    composer.apply_history_entry(history_entry);
    assert_eq!(composer.draft.textarea.text(), placeholder);
    assert_eq!(
        composer.draft.pending_pastes,
        vec![(placeholder.clone(), large)]
    );
    assert_eq!(
        composer.draft.textarea.element_payloads(),
        vec![placeholder]
    );

    let (result, _needs_redraw) =
        composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    match result {
        InputResult::Submitted {
            text,
            text_elements,
        } => {
            assert_eq!(text, "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5));
            assert!(text_elements.is_empty());
        }
        _ => panic!("expected Submitted"),
    }
}

#[test]
fn large_paste_numbering_reuses_after_ctrl_c_clear() {
    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let paste = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 4);
    let base = format!("[Pasted Content {} chars]", paste.chars().count());

    composer.handle_paste(paste.clone());
    assert_eq!(composer.draft.textarea.text(), base);
    assert_eq!(composer.draft.pending_pastes.len(), 1);

    assert_eq!(composer.clear_for_ctrl_c(), Some(base.clone()));
    assert!(composer.draft.textarea.text().is_empty());
    assert!(composer.draft.pending_pastes.is_empty());

    composer.handle_paste(paste);
    assert_eq!(composer.draft.textarea.text(), base);
    assert_eq!(composer.draft.pending_pastes.len(), 1);
    assert_eq!(composer.draft.pending_pastes[0].0, base);
}

/// Behavior: while a paste-like burst is being captured, `?` must not toggle the shortcut
/// overlay; it should be treated as part of the pasted content.
#[test]
fn question_mark_does_not_toggle_during_paste_burst() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    // Force an active paste burst so this test doesn't depend on tight timing.
    composer
        .draft
        .paste_burst
        .begin_with_retro_grabbed(String::new(), Instant::now());

    for ch in ['h', 'i', '?', 't', 'h', 'e', 'r', 'e'] {
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert!(composer.is_in_paste_burst());
    assert_eq!(composer.draft.textarea.text(), "");

    let _ = flush_after_paste_burst(&mut composer);

    assert_eq!(composer.draft.textarea.text(), "hi?there");
    assert_ne!(composer.footer.mode, FooterMode::ShortcutOverlay);
}

/// Behavior: a single non-ASCII char should be inserted immediately (IME-friendly) and should
/// not create any paste-burst state.
#[test]
fn non_ascii_char_inserts_immediately_without_burst_state() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('あ'), KeyModifiers::NONE));

    assert_eq!(composer.draft.textarea.text(), "あ");
    assert!(!composer.is_in_paste_burst());
}

/// Behavior: while we're capturing a paste-like burst, Enter should be treated as a newline
/// within the burst (not as "submit"), and the whole payload should flush as one paste.
#[test]
fn non_ascii_burst_buffers_enter_and_flushes_multiline() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    composer
        .draft
        .paste_burst
        .begin_with_retro_grabbed(String::new(), Instant::now());

    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('你'), KeyModifiers::NONE));
    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('好'), KeyModifiers::NONE));
    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));

    assert!(composer.draft.textarea.text().is_empty());
    let _ = flush_after_paste_burst(&mut composer);
    assert_eq!(composer.draft.textarea.text(), "你好\nhi");
}

/// Behavior: a paste-like burst may include a full-width/ideographic space (U+3000). It should
/// still be captured as a single paste payload and preserve the exact Unicode content.
#[test]
fn non_ascii_burst_preserves_ideographic_space_and_ascii() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    composer
        .draft
        .paste_burst
        .begin_with_retro_grabbed(String::new(), Instant::now());

    for ch in ['你', '　', '好'] {
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for ch in ['h', 'i'] {
        let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }

    assert!(composer.draft.textarea.text().is_empty());
    let _ = flush_after_paste_burst(&mut composer);
    assert_eq!(composer.draft.textarea.text(), "你　好\nhi");
}

/// Behavior: a large multi-line payload containing both non-ASCII and ASCII (e.g. "UTF-8",
/// "Unicode") should be captured as a single paste-like burst, and Enter key events should
/// become `\n` within the buffered content.
#[test]
fn non_ascii_burst_buffers_large_multiline_mixed_ascii_and_unicode() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    const LARGE_MIXED_PAYLOAD: &str = "天地玄黄 宇宙洪荒\n\
日月盈昃 辰宿列张\n\
寒来暑往 秋收冬藏\n\
\n\
你好世界 编码测试\n\
汉字处理 UTF-8\n\
终端显示 正确无误\n\
\n\
风吹竹林 月照大江\n\
白云千载 青山依旧\n\
程序员 与 Unicode 同行";

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    // Force an active burst so the test doesn't depend on timing heuristics.
    composer
        .draft
        .paste_burst
        .begin_with_retro_grabbed(String::new(), Instant::now());

    for ch in LARGE_MIXED_PAYLOAD.chars() {
        let code = if ch == '\n' {
            KeyCode::Enter
        } else {
            KeyCode::Char(ch)
        };
        let _ = composer.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
    }

    assert!(composer.draft.textarea.text().is_empty());
    let _ = flush_after_paste_burst(&mut composer);
    assert_eq!(composer.draft.textarea.text(), LARGE_MIXED_PAYLOAD);
}

/// Behavior: while a paste-like burst is active, Enter should not submit; it should insert a
/// newline into the buffered payload and flush as a single paste later.
#[test]
fn ascii_burst_treats_enter_as_newline() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let mut now = Instant::now();
    let step = Duration::from_millis(1);

    let _ = composer
        .handle_input_basic_with_time(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE), now);
    now += step;
    let _ = composer
        .handle_input_basic_with_time(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE), now);
    now += step;

    let (result, _) = composer.handle_submission_with_time(/*should_queue*/ false, now);
    assert!(
        matches!(result, InputResult::None),
        "Enter during a burst should insert newline, not submit"
    );

    for ch in ['t', 'h', 'e', 'r', 'e'] {
        now += step;
        let _ = composer.handle_input_basic_with_time(
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            now,
        );
    }

    assert!(composer.draft.textarea.text().is_empty());
    let flush_time = now + PasteBurst::recommended_active_flush_delay() + step;
    let flushed = composer.handle_paste_burst_flush(flush_time);
    assert!(flushed, "expected paste burst to flush");
    assert_eq!(composer.draft.textarea.text(), "hi\nthere");
}

/// Behavior: startup-pending submissions are queued immediately, so Enter should flush any
/// buffered burst text into that queued message instead of turning into a draft newline.
#[test]
fn queued_submission_flushes_ascii_burst_instead_of_inserting_newline() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let mut now = Instant::now();
    let step = Duration::from_millis(1);
    for ch in ['h', 'i'] {
        let _ = composer.handle_input_basic_with_time(
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
            now,
        );
        now += step;
    }
    assert!(composer.is_in_paste_burst());

    let (result, _) = composer.handle_submission_with_time(/*should_queue*/ true, now);

    assert_eq!(
        result,
        InputResult::Queued {
            text: "hi".to_string(),
            text_elements: Vec::new(),
            action: QueuedInputAction::Plain,
        }
    );
    assert!(composer.draft.textarea.text().is_empty());
    assert!(!composer.is_in_paste_burst());
}

/// Behavior: even if Enter suppression would normally be active for a burst, Enter should
/// still dispatch a built-in slash command when the first line begins with `/`.
#[test]
fn slash_context_enter_ignores_paste_burst_enter_suppression() {
    use crate::slash_command::SlashCommand;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    composer.draft.textarea.set_text_clearing_elements("/diff");
    composer.draft.textarea.set_cursor("/diff".len());
    composer
        .draft
        .paste_burst
        .begin_with_retro_grabbed(String::new(), Instant::now());

    let (result, _) = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(result, InputResult::Command(SlashCommand::Diff)));
}

/// Behavior: if a burst is buffering text and the user presses a non-char key, flush the
/// buffered burst *before* applying that key so the buffer cannot get stuck.
#[test]
fn non_char_key_flushes_active_burst_before_input() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    // Force an active burst so we can deterministically buffer characters without relying on
    // timing.
    composer
        .draft
        .paste_burst
        .begin_with_retro_grabbed(String::new(), Instant::now());

    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
    assert!(composer.draft.textarea.text().is_empty());
    assert!(composer.is_in_paste_burst());

    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(composer.draft.textarea.text(), "hi");
    assert_eq!(composer.draft.textarea.cursor(), 1);
    assert!(!composer.is_in_paste_burst());
}

/// Behavior: enabling `disable_paste_burst` flushes any held first character (flicker
/// suppression) and then inserts subsequent chars immediately without creating burst state.
#[test]
fn disable_paste_burst_flushes_pending_first_char_and_inserts_immediately() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    // First ASCII char is normally held briefly. Flip the config mid-stream and ensure the
    // held char is not dropped.
    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    assert!(composer.is_in_paste_burst());
    assert!(composer.draft.textarea.text().is_empty());

    composer.set_disable_paste_burst(/*disabled*/ true);
    assert_eq!(composer.draft.textarea.text(), "a");
    assert!(!composer.is_in_paste_burst());

    let _ = composer.handle_key_event(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE));
    assert_eq!(composer.draft.textarea.text(), "ab");
    assert!(!composer.is_in_paste_burst());
}

/// Behavior: a small explicit paste inserts text directly (no placeholder), and the submitted
/// text matches what is visible in the textarea.
#[test]
fn handle_paste_small_inserts_text() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let needs_redraw = composer.handle_paste("hello".to_string());
    assert!(needs_redraw);
    assert_eq!(composer.draft.textarea.text(), "hello");
    assert!(composer.draft.pending_pastes.is_empty());

    let (result, _) = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    match result {
        InputResult::Submitted { text, .. } => assert_eq!(text, "hello"),
        _ => panic!("expected Submitted"),
    }
}

/// Behavior: a large explicit paste inserts a placeholder into the textarea, stores the full
/// content in `pending_pastes`, and expands the placeholder to the full content on submit.
#[test]
fn handle_paste_large_uses_placeholder_and_replaces_on_submit() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let large = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 10);
    let needs_redraw = composer.handle_paste(large.clone());
    assert!(needs_redraw);
    let placeholder = format!("[Pasted Content {} chars]", large.chars().count());
    assert_eq!(composer.draft.textarea.text(), placeholder);
    assert_eq!(composer.draft.pending_pastes.len(), 1);
    assert_eq!(composer.draft.pending_pastes[0].0, placeholder);
    assert_eq!(composer.draft.pending_pastes[0].1, large);

    let (result, _) = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    match result {
        InputResult::Submitted { text, .. } => assert_eq!(text, large),
        _ => panic!("expected Submitted"),
    }
    assert!(composer.draft.pending_pastes.is_empty());
}

/// Behavior: editing that removes a paste placeholder should also clear the associated
/// `pending_pastes` entry so it cannot be submitted accidentally.
#[test]
fn edit_clears_pending_paste() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let large = "y".repeat(LARGE_PASTE_CHAR_THRESHOLD + 1);
    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    composer.handle_paste(large);
    assert_eq!(composer.draft.pending_pastes.len(), 1);

    // Any edit that removes the placeholder should clear pending_paste
    composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert!(composer.draft.pending_pastes.is_empty());
}

#[test]
fn file_completion_preserves_large_paste_placeholder_elements() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let large = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5);
    let placeholder = format!("[Pasted Content {} chars]", large.chars().count());

    composer.handle_paste(large.clone());
    composer.insert_str(" @ma");
    composer.on_file_search_result(
        "ma".to_string(),
        vec![FileMatch {
            score: 1,
            path: PathBuf::from("src/main.rs"),
            match_type: codex_file_search::MatchType::File,
            root: PathBuf::from("/tmp"),
            indices: None,
        }],
    );

    let (_result, _needs_redraw) =
        composer.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));

    let text = composer.draft.textarea.text().to_string();
    assert_eq!(text, format!("{placeholder} src/main.rs "));
    let elements = composer.draft.textarea.text_elements();
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0].placeholder(&text), Some(placeholder.as_str()));

    let (result, _needs_redraw) =
        composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    match result {
        InputResult::Submitted {
            text,
            text_elements,
        } => {
            assert_eq!(text, format!("{large} src/main.rs"));
            assert!(text_elements.is_empty());
        }
        _ => panic!("expected Submitted"),
    }
}

/// Behavior: multiple paste operations can coexist; placeholders should be expanded to their
/// original content on submission.
#[test]
fn test_multiple_pastes_submission() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    // Define test cases: (paste content, is_large)
    let test_cases = [
        ("x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 3), true),
        (" and ".to_string(), false),
        ("y".repeat(LARGE_PASTE_CHAR_THRESHOLD + 7), true),
    ];

    // Expected states after each paste
    let mut expected_text = String::new();
    let mut expected_pending_count = 0;

    // Apply all pastes and build expected state
    let states: Vec<_> = test_cases
        .iter()
        .map(|(content, is_large)| {
            composer.handle_paste(content.clone());
            if *is_large {
                let placeholder = format!("[Pasted Content {} chars]", content.chars().count());
                expected_text.push_str(&placeholder);
                expected_pending_count += 1;
            } else {
                expected_text.push_str(content);
            }
            (expected_text.clone(), expected_pending_count)
        })
        .collect();

    // Verify all intermediate states were correct
    assert_eq!(
        states,
        vec![
            (
                format!("[Pasted Content {} chars]", test_cases[0].0.chars().count()),
                1
            ),
            (
                format!(
                    "[Pasted Content {} chars] and ",
                    test_cases[0].0.chars().count()
                ),
                1
            ),
            (
                format!(
                    "[Pasted Content {} chars] and [Pasted Content {} chars]",
                    test_cases[0].0.chars().count(),
                    test_cases[2].0.chars().count()
                ),
                2
            ),
        ]
    );

    // Submit and verify final expansion
    let (result, _) = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    if let InputResult::Submitted { text, .. } = result {
        assert_eq!(text, format!("{} and {}", test_cases[0].0, test_cases[2].0));
    } else {
        panic!("expected Submitted");
    }
}

/// Behavior: if multiple large pastes share the same placeholder label (same char count),
/// deleting one placeholder removes only its corresponding `pending_pastes` entry.
#[test]
fn deleting_duplicate_length_pastes_removes_only_target() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let paste = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 4);
    let placeholder_base = format!("[Pasted Content {} chars]", paste.chars().count());
    let placeholder_second = format!("{placeholder_base} #2");

    composer.handle_paste(paste.clone());
    composer.handle_paste(paste.clone());
    assert_eq!(
        composer.draft.textarea.text(),
        format!("{placeholder_base}{placeholder_second}")
    );
    assert_eq!(composer.draft.pending_pastes.len(), 2);

    composer
        .draft
        .textarea
        .set_cursor(composer.draft.textarea.text().len());
    composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

    assert_eq!(composer.draft.textarea.text(), placeholder_base);
    assert_eq!(composer.draft.pending_pastes.len(), 1);
    assert_eq!(composer.draft.pending_pastes[0].0, placeholder_base);
    assert_eq!(composer.draft.pending_pastes[0].1, paste);
}

/// Behavior: large-paste placeholder numbering continues when another placeholder of the
/// same length still exists, so a new paste gets a new unique placeholder label.
#[test]
fn large_paste_numbering_continues_with_same_length_placeholder() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let paste = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 4);
    let base = format!("[Pasted Content {} chars]", paste.chars().count());
    let second = format!("{base} #2");
    let third = format!("{base} #3");

    composer.handle_paste(paste.clone());
    composer.handle_paste(paste.clone());
    assert_eq!(composer.draft.textarea.text(), format!("{base}{second}"));

    composer.draft.textarea.set_cursor(base.len());
    composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(composer.draft.textarea.text(), second);
    assert_eq!(composer.draft.pending_pastes.len(), 1);
    assert_eq!(composer.draft.pending_pastes[0].0, second);

    composer
        .draft
        .textarea
        .set_cursor(composer.draft.textarea.text().len());
    composer.handle_paste(paste);

    assert_eq!(composer.draft.textarea.text(), format!("{second}{third}"));
    assert_eq!(composer.draft.pending_pastes.len(), 2);
    assert_eq!(composer.draft.pending_pastes[0].0, second);
    assert_eq!(composer.draft.pending_pastes[1].0, third);
}

/// Behavior: if all placeholders of a given length are removed, numbering resets to the
/// base placeholder on the next paste.
#[test]
fn large_paste_numbering_reuses_after_all_deleted() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let paste = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 4);
    let base = format!("[Pasted Content {} chars]", paste.chars().count());

    composer.handle_paste(paste.clone());
    assert_eq!(composer.draft.textarea.text(), base);
    assert_eq!(composer.draft.pending_pastes.len(), 1);

    composer
        .draft
        .textarea
        .set_cursor(composer.draft.textarea.text().len());
    composer.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert!(composer.draft.textarea.text().is_empty());
    assert!(composer.draft.pending_pastes.is_empty());

    composer.handle_paste(paste);
    assert_eq!(composer.draft.textarea.text(), base);
    assert_eq!(composer.draft.pending_pastes.len(), 1);
    assert_eq!(composer.draft.pending_pastes[0].0, base);
}

#[test]
fn large_paste_preserves_image_text_elements_on_submit() {
    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let large_content = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5);
    composer.handle_paste(large_content.clone());
    composer.handle_paste(" ".into());
    let path = PathBuf::from("/tmp/image_with_paste.png");
    composer.attach_image(path.clone());

    let (result, _) = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    match result {
        InputResult::Submitted {
            text,
            text_elements,
        } => {
            let expected = format!("{large_content} [Image #1]");
            assert_eq!(text, expected);
            assert_eq!(text_elements.len(), 1);
            assert_eq!(text_elements[0].placeholder(&text), Some("[Image #1]"));
            assert_eq!(
                text_elements[0].byte_range,
                ByteRange {
                    start: large_content.len() + 1,
                    end: large_content.len() + 1 + "[Image #1]".len(),
                }
            );
        }
        _ => panic!("expected Submitted"),
    }
    let imgs = composer.take_recent_submission_images();
    assert_eq!(vec![path], imgs);
}

#[test]
fn large_paste_with_leading_whitespace_trims_and_shifts_elements() {
    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let large_content = format!("  {}", "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5));
    composer.handle_paste(large_content.clone());
    composer.handle_paste(" ".into());
    let path = PathBuf::from("/tmp/image_with_trim.png");
    composer.attach_image(path.clone());

    let (result, _) = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    match result {
        InputResult::Submitted {
            text,
            text_elements,
        } => {
            let trimmed = large_content.trim().to_string();
            assert_eq!(text, format!("{trimmed} [Image #1]"));
            assert_eq!(text_elements.len(), 1);
            assert_eq!(text_elements[0].placeholder(&text), Some("[Image #1]"));
            assert_eq!(
                text_elements[0].byte_range,
                ByteRange {
                    start: trimmed.len() + 1,
                    end: trimmed.len() + 1 + "[Image #1]".len(),
                }
            );
        }
        _ => panic!("expected Submitted"),
    }
    let imgs = composer.take_recent_submission_images();
    assert_eq!(vec![path], imgs);
}

#[test]
fn pasted_crlf_normalizes_newlines_for_elements() {
    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let pasted = "line1\r\nline2\r\n".to_string();
    composer.handle_paste(pasted);
    composer.handle_paste(" ".into());
    let path = PathBuf::from("/tmp/image_crlf.png");
    composer.attach_image(path.clone());

    let (result, _) = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    match result {
        InputResult::Submitted {
            text,
            text_elements,
        } => {
            assert_eq!(text, "line1\nline2\n [Image #1]");
            assert!(!text.contains('\r'));
            assert_eq!(text_elements.len(), 1);
            assert_eq!(text_elements[0].placeholder(&text), Some("[Image #1]"));
            assert_eq!(
                text_elements[0].byte_range,
                ByteRange {
                    start: "line1\nline2\n ".len(),
                    end: "line1\nline2\n [Image #1]".len(),
                }
            );
        }
        _ => panic!("expected Submitted"),
    }
    let imgs = composer.take_recent_submission_images();
    assert_eq!(vec![path], imgs);
}

/// Submitting an unrecognized slash command clears the composer — including any pending
/// paste payload — just like a valid command submission, rather than restoring the mistyped
/// draft. This keeps the input empty so the user can immediately retype.
#[test]
fn unrecognized_slash_command_submission_clears_pending_paste() {
    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    composer
        .draft
        .textarea
        .set_text_clearing_elements("/unknown ");
    composer.draft.textarea.set_cursor("/unknown ".len());
    let large_content = "x".repeat(LARGE_PASTE_CHAR_THRESHOLD + 5);
    composer.handle_paste(large_content);
    assert_eq!(composer.draft.pending_pastes.len(), 1);

    let (result, _) = composer.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(result, InputResult::None));
    // The unrecognized command is dropped from the composer along with its pending paste
    // rather than restored, so nothing lingers in the input.
    assert!(composer.draft.pending_pastes.is_empty());
    assert!(composer.draft.textarea.text().is_empty());
}

/// Behavior: fast "paste-like" ASCII input should buffer and then flush as a single paste. If
/// the payload is small, it should insert directly (no placeholder).
#[test]
fn burst_paste_fast_small_buffers_and_flushes_on_stop() {
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;

    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let count = 32;
    let mut now = Instant::now();
    let step = Duration::from_millis(1);
    for _ in 0..count {
        let _ = composer.handle_input_basic_with_time(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
            now,
        );
        assert!(
            composer.is_in_paste_burst(),
            "expected active paste burst during fast typing"
        );
        assert!(
            composer.draft.textarea.text().is_empty(),
            "text should not appear during burst"
        );
        now += step;
    }

    assert!(
        composer.draft.textarea.text().is_empty(),
        "text should remain empty until flush"
    );
    let flush_time = now + PasteBurst::recommended_active_flush_delay() + step;
    let flushed = composer.handle_paste_burst_flush(flush_time);
    assert!(flushed, "expected buffered text to flush after stop");
    assert_eq!(composer.draft.textarea.text(), "a".repeat(count));
    assert!(
        composer.draft.pending_pastes.is_empty(),
        "no placeholder for small burst"
    );
}

/// Behavior: fast "paste-like" ASCII input should buffer and then flush as a single paste. If
/// the payload is large, it should insert a placeholder and defer the full text until submit.
#[test]
fn burst_paste_fast_large_inserts_placeholder_on_flush() {
    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let sender = AppEventSender::new(tx);
    let mut composer = ChatComposer::new(
        /*has_input_focus*/ true,
        sender,
        /*enhanced_keys_supported*/ false,
        "Ask Codewith to do anything".to_string(),
        /*disable_paste_burst*/ false,
    );

    let count = LARGE_PASTE_CHAR_THRESHOLD + 1; // > threshold to trigger placeholder
    let mut now = Instant::now();
    let step = Duration::from_millis(1);
    for _ in 0..count {
        let _ = composer.handle_input_basic_with_time(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            now,
        );
        now += step;
    }

    // Nothing should appear until we stop and flush
    assert!(composer.draft.textarea.text().is_empty());
    let flush_time = now + PasteBurst::recommended_active_flush_delay() + step;
    let flushed = composer.handle_paste_burst_flush(flush_time);
    assert!(flushed, "expected flush after stopping fast input");

    let expected_placeholder = format!("[Pasted Content {count} chars]");
    assert_eq!(composer.draft.textarea.text(), expected_placeholder);
    assert_eq!(composer.draft.pending_pastes.len(), 1);
    assert_eq!(composer.draft.pending_pastes[0].0, expected_placeholder);
    assert_eq!(composer.draft.pending_pastes[0].1.len(), count);
    assert!(composer.draft.pending_pastes[0].1.chars().all(|c| c == 'x'));
}

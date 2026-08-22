use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Composer;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn edits_unicode_text_without_splitting_characters() {
    let mut composer = Composer::default();
    composer.input(key(KeyCode::Char('你')));
    composer.input(key(KeyCode::Char('好')));
    composer.input(key(KeyCode::Left));
    composer.input(key(KeyCode::Backspace));
    assert_eq!(composer.text(), "好");
}

#[test]
fn command_replacement_moves_cursor_to_end() {
    let mut composer = Composer::default();
    composer.replace_command("compact");
    composer.input(key(KeyCode::Char(' ')));
    assert_eq!(composer.text(), "/compact ");
}

#[test]
fn cjk_cursor_uses_display_width() {
    let mut composer = Composer::default();
    composer.input(key(KeyCode::Char('你')));
    composer.input(key(KeyCode::Char('a')));
    assert_eq!(composer.visual_cursor(20), (3, 0));
}

#[test]
fn shift_enter_inserts_a_newline() {
    let mut composer = Composer::default();
    composer.input(key(KeyCode::Char('a')));
    composer.input(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    composer.input(key(KeyCode::Char('b')));
    assert_eq!(composer.text(), "a\nb");
}

#[test]
fn control_j_is_a_newline_fallback() {
    let mut composer = Composer::default();
    composer.input(key(KeyCode::Char('a')));
    composer.input(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
    composer.input(key(KeyCode::Char('b')));
    assert_eq!(composer.text(), "a\nb");
}

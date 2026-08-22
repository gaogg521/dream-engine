use super::{SessionPicker, TuiSession};

fn session(id: &str) -> TuiSession {
    TuiSession::new(
        id.to_string(),
        "model".to_string(),
        "summary".to_string(),
        "2026-08-13 12:00 UTC".to_string(),
        2,
    )
}

#[test]
fn selection_wraps_in_both_directions() {
    let mut picker = SessionPicker::default();
    picker.set_sessions(vec![session("first"), session("second")]);
    assert!(picker.open());

    picker.move_previous();
    assert_eq!(picker.selected_id().as_deref(), Some("second"));
    picker.move_next();
    assert_eq!(picker.selected_id().as_deref(), Some("first"));
}

#[test]
fn empty_catalog_cannot_open() {
    let mut picker = SessionPicker::default();
    assert!(!picker.open());
    assert!(!picker.is_visible());
}

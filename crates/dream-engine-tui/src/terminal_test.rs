use super::finish_terminal_output;

#[test]
fn terminal_cleanup_ends_output_with_a_flushed_newline() {
    let mut output = Vec::new();

    finish_terminal_output(&mut output).expect("terminal output should finish cleanly");

    assert_eq!(output, b"\r\n");
}

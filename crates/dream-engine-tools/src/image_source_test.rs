use serde_json::json;

use super::{image_path_argument, strip_verbatim_prefix};

#[test]
fn strips_windows_extended_length_prefix() {
    // The Dream UI host injects attachment paths in this verbatim form.
    assert_eq!(
        strip_verbatim_prefix(r"\\?\C:\Users\me\image-1(4).png"),
        r"C:\Users\me\image-1(4).png"
    );
}

#[test]
fn rewrites_verbatim_unc_prefix_to_a_real_unc_path() {
    // Dropping `\\?\UNC\` outright would leave `server\share\...`, a *relative*
    // path pointing at a different location than the caller asked for.
    assert_eq!(
        strip_verbatim_prefix(r"\\?\UNC\server\share\image.png"),
        r"\\server\share\image.png"
    );
}

#[test]
fn leaves_ordinary_paths_untouched() {
    assert_eq!(
        strip_verbatim_prefix(r"C:\Users\me\image.png"),
        r"C:\Users\me\image.png"
    );
    assert_eq!(strip_verbatim_prefix("/home/me/image.png"), "/home/me/image.png");
}

#[test]
fn path_argument_normalizes_verbatim_paths_from_tool_input() {
    let input = json!({ "file_path": r"\\?\C:\Users\me\image.png" });

    assert_eq!(image_path_argument(&input).unwrap(), r"C:\Users\me\image.png");
}

#[test]
fn path_argument_rejects_missing_and_blank_values() {
    assert!(image_path_argument(&json!({})).is_err());
    assert!(image_path_argument(&json!({ "file_path": "   " })).is_err());
    assert!(image_path_argument(&json!({ "file_path": 42 })).is_err());
}

#[cfg(windows)]
#[tokio::test]
async fn loads_an_image_addressed_through_a_verbatim_path() {
    use std::fs;

    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use tempfile::TempDir;

    use super::load_image_url;

    let directory = TempDir::new().expect("temp dir");
    let path = directory.path().join("sample.png");
    let png = STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScL3WQAAAABJRU5ErkJggg==")
        .expect("decode PNG fixture");
    fs::write(&path, png).expect("write image fixture");

    let verbatim = json!({ "file_path": format!(r"\\?\{}", path.display()) });
    let resolved = image_path_argument(&verbatim).expect("verbatim path accepted");
    let image_url = load_image_url(&resolved).await.expect("image loads");

    assert!(image_url.url.starts_with("data:image/png;base64,"));
}

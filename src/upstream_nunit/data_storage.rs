use crate::data_storage::{ImageError, open_image};
use std::path::Path;

#[test]
fn should_throw_on_no_file() {
    assert!(matches!(
        open_image(Some(Path::new("/doesntexist"))),
        Err(ImageError::MissingFile)
    ));
}

#[test]
fn should_throw_on_empty_path() {
    assert!(matches!(open_image(None), Err(ImageError::MissingPath)));
    assert!(matches!(
        open_image(Some(Path::new(""))),
        Err(ImageError::MissingPath)
    ));
}

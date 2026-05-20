use std::io::{self, Read as _};
use std::path::Path;

pub fn read_bounded_text(path: &Path, preview_limit_bytes: usize) -> io::Result<(String, bool)> {
    let mut file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    let read_limit = preview_limit_bytes.saturating_add(1) as u64;
    file.by_ref().take(read_limit).read_to_end(&mut bytes)?;
    let truncated = bytes.len() > preview_limit_bytes;
    if truncated {
        bytes.truncate(preview_limit_bytes);
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), truncated))
}

pub fn is_regular_file_no_symlink(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_file()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn is_directory_no_symlink(path: &Path) -> io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_dir()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub fn truncate_to_char_boundary(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_to_char_boundary_short_input_is_noop() {
        let mut value = String::from("hello");
        truncate_to_char_boundary(&mut value, 64);
        assert_eq!(value, "hello");
    }

    #[test]
    fn truncate_to_char_boundary_exact_boundary_cut() {
        let mut value = String::from("hello world");
        truncate_to_char_boundary(&mut value, 5);
        assert_eq!(value, "hello");
    }

    #[test]
    fn truncate_to_char_boundary_walks_back_inside_multibyte_char() {
        // "é" is 2 bytes (0xC3 0xA9). Cutting at byte 6 lands inside it
        // ("aaaaa" + first byte of "é"); the helper must walk back to 5.
        let mut value = String::from("aaaaaé");
        assert_eq!(value.len(), 7);
        truncate_to_char_boundary(&mut value, 6);
        assert_eq!(value, "aaaaa");
        assert_eq!(value.len(), 5);
    }
}

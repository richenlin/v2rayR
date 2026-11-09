//! Log tailing without a logging framework dependency.
//!
//! Both the app and each kernel append to a file; the UI subscribes to new lines
//! by remembering a byte offset. Reading the tail is also how a failed start
//! explains itself.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Last `count` lines of a file, cheap enough for error paths.
pub fn tail(path: &Path, count: usize) -> std::io::Result<Vec<String>> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    // Read at most the last 64 KiB: kernel logs can be chatty.
    let window = length.min(64 * 1024);
    file.seek(SeekFrom::Start(length - window))?;
    let mut buffer = String::new();
    file.take(window).read_to_string(&mut buffer)?;
    let mut lines: Vec<String> = buffer
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect();
    if lines.len() > count {
        lines = lines.split_off(lines.len() - count);
    }
    Ok(lines)
}

/// Read everything appended after `offset`, returning the new offset.
pub fn read_from(path: &Path, offset: u64) -> std::io::Result<(Vec<String>, u64)> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    if length < offset {
        // Truncated or rotated: start over rather than seeking past the end.
        return Ok((Vec::new(), 0));
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = String::new();
    file.read_to_string(&mut buffer)?;
    let lines = buffer
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect();
    Ok((lines, length))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_lines(path: &Path, lines: &[&str]) {
        let mut file = std::fs::File::create(path).expect("create");
        for line in lines {
            writeln!(file, "{line}").expect("write");
        }
    }

    #[test]
    fn tail_returns_the_last_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("k.log");
        write_lines(&path, &["a", "b", "c", "d"]);
        assert_eq!(tail(&path, 2).expect("tail"), vec!["c", "d"]);
        assert_eq!(tail(&path, 99).expect("tail").len(), 4);
    }

    #[test]
    fn read_from_resumes_at_the_offset() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("k.log");
        write_lines(&path, &["one", "two"]);
        let (lines, offset) = read_from(&path, 0).expect("read");
        assert_eq!(lines.len(), 2);
        write_lines(&path, &["one", "two", "three"]);
        let (lines, _) = read_from(&path, offset).expect("read");
        assert_eq!(lines, vec!["three"]);
    }

    #[test]
    fn a_shrunk_file_restarts_from_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("k.log");
        write_lines(&path, &["one", "two"]);
        let (_, offset) = read_from(&path, 0).expect("read");
        write_lines(&path, &["x"]);
        let (lines, new_offset) = read_from(&path, offset).expect("read");
        assert!(lines.is_empty());
        assert_eq!(new_offset, 0);
    }
}

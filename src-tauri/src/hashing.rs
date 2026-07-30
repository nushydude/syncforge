use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Returns a lowercase hex BLAKE3 digest of the file at `path`.
pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut noop = |_: u64, _: u64| true;
    hash_file_with_progress(path, &mut noop)
}

pub fn hash_file_with_progress(
    path: &Path,
    mut on_progress: impl FnMut(u64, u64) -> bool,
) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    let total = file.metadata()?.len();
    let mut processed = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        processed += read as u64;
        if !on_progress(processed, total) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Returns a lowercase hex BLAKE3 digest of `data` (used in unit tests).
#[cfg(test)]
pub fn hash_bytes(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn hash_bytes_is_deterministic() {
        let a = hash_bytes(b"hello");
        let b = hash_bytes(b"hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn hash_file_matches_hash_bytes() {
        let mut file = NamedTempFile::new().expect("temp file");
        file.write_all(b"syncforge").expect("write");
        file.flush().expect("flush");
        let path = file.path();
        assert_eq!(hash_file(path).expect("hash file"), hash_bytes(b"syncforge"));
    }
}

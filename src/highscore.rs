use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

const HIGHSCORE_RELATIVE_PATH: &[&str] = &[".local", "share", "asteroids", "highscore"];
const MAX_HIGHSCORE_BYTES: u64 = 32;

pub fn default_path() -> io::Result<PathBuf> {
    let home = env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    Ok(path_for_home(home))
}

pub fn path_for_home(home: impl AsRef<Path>) -> PathBuf {
    let mut path = home.as_ref().to_path_buf();
    for component in HIGHSCORE_RELATIVE_PATH {
        path.push(component);
    }
    path
}

pub fn read_default() -> io::Result<u32> {
    read(&default_path()?)
}

pub fn write_default(score: u32) -> io::Result<()> {
    write(&default_path()?, score)
}

pub fn read(path: &Path) -> io::Result<u32> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.len() > MAX_HIGHSCORE_BYTES {
        return Ok(0);
    }

    let content = fs::read(path)?;
    Ok(parse_highscore(&content).unwrap_or(0))
}

pub fn write(path: &Path, score: u32) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, score.to_string())
}

fn parse_highscore(content: &[u8]) -> Option<u32> {
    let trimmed = trim_ascii_whitespace(content);
    if trimmed.is_empty() || !trimmed.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let text = std::str::from_utf8(trimmed).ok()?;
    let value = text.parse::<u64>().ok()?;
    u32::try_from(value).ok()
}

fn trim_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_home(stem: &str) -> PathBuf {
        let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!(
            "asteroids-highscore-{stem}-{}-{id}",
            std::process::id()
        ))
    }

    #[test]
    fn reads_valid_highscore_file() {
        let home = temp_home("valid");
        let path = path_for_home(&home);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "12345").unwrap();

        assert_eq!(read(&path).unwrap(), 12_345);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn missing_highscore_file_returns_zero() {
        let home = temp_home("missing");
        let path = path_for_home(&home);

        assert_eq!(read(&path).unwrap(), 0);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn corrupt_highscore_file_returns_zero() {
        for content in [b"abc".as_slice(), b"-1", b"4294967296", b"12x34", b"\xff"] {
            let home = temp_home("corrupt");
            let path = path_for_home(&home);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();

            assert_eq!(read(&path).unwrap(), 0, "content={content:?}");
            let _ = fs::remove_dir_all(home);
        }
    }

    #[test]
    fn write_then_read_round_trips() {
        let home = temp_home("roundtrip");
        let path = path_for_home(&home);

        write(&path, 98_765).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "98765");
        assert_eq!(read(&path).unwrap(), 98_765);
        let _ = fs::remove_dir_all(home);
    }
}

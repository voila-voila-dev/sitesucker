use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub struct State {
    path: PathBuf,
    pub visited: HashSet<String>,
}

impl State {
    pub fn load(output_dir: &Path) -> Self {
        let path = output_dir.join(".sitegrabber-state");
        let mut visited = HashSet::new();

        if path.exists() {
            if let Ok(file) = File::open(&path) {
                for line in BufReader::new(file).lines() {
                    if let Ok(url) = line {
                        let url = url.trim().to_string();
                        if !url.is_empty() {
                            visited.insert(url);
                        }
                    }
                }
            }
        }

        State { path, visited }
    }

    pub fn empty(output_dir: &Path) -> Self {
        State {
            path: output_dir.join(".sitegrabber-state"),
            visited: HashSet::new(),
        }
    }

    pub fn mark_done(&mut self, url: &str) {
        if self.visited.insert(url.to_string()) {
            if let Ok(mut file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                let _ = writeln!(file, "{}", url);
            }
        }
    }

    pub fn is_done(&self, url: &str) -> bool {
        self.visited.contains(url)
    }

    pub fn count(&self) -> usize {
        self.visited.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "sitegrabber_test_{}_{}", std::process::id(), id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn empty_state() {
        let dir = tmp_dir();
        let state = State::empty(&dir);
        assert_eq!(state.count(), 0);
        assert!(!state.is_done("https://example.com"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_done_and_check() {
        let dir = tmp_dir();
        let mut state = State::empty(&dir);
        state.mark_done("https://example.com/page1");
        state.mark_done("https://example.com/page2");

        assert!(state.is_done("https://example.com/page1"));
        assert!(state.is_done("https://example.com/page2"));
        assert!(!state.is_done("https://example.com/page3"));
        assert_eq!(state.count(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn mark_done_idempotent() {
        let dir = tmp_dir();
        let mut state = State::empty(&dir);
        state.mark_done("https://example.com/page1");
        state.mark_done("https://example.com/page1");
        assert_eq!(state.count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_persists_to_disk() {
        let dir = tmp_dir();

        {
            let mut state = State::empty(&dir);
            state.mark_done("https://example.com/a");
            state.mark_done("https://example.com/b");
        }

        // Load from disk
        let state = State::load(&dir);
        assert_eq!(state.count(), 2);
        assert!(state.is_done("https://example.com/a"));
        assert!(state.is_done("https://example.com/b"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_nonexistent_state() {
        let dir = tmp_dir();
        let state = State::load(&dir);
        assert_eq!(state.count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_file_is_appendable() {
        let dir = tmp_dir();

        {
            let mut state = State::empty(&dir);
            state.mark_done("https://example.com/1");
        }
        {
            let mut state = State::load(&dir);
            state.mark_done("https://example.com/2");
        }

        let state = State::load(&dir);
        assert_eq!(state.count(), 2);
        let _ = fs::remove_dir_all(&dir);
    }
}

use std::process::Command;
use std::fs;
use std::path::PathBuf;

fn binary_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("target");
    // Use debug for test builds
    path.push("debug");
    path.push("sitegrabber");
    path
}

fn tmp_output(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sitegrabber_integ_{}", name));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn help_flag() {
    let output = Command::new(binary_path())
        .arg("--help")
        .output()
        .expect("failed to run");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Download an entire website"));
    assert!(stdout.contains("--output"));
    assert!(stdout.contains("--depth"));
    assert!(stdout.contains("--concurrency"));
    assert!(stdout.contains("--warc"));
    assert!(stdout.contains("--respect-robots"));
    assert!(stdout.contains("--exclude"));
    assert!(stdout.contains("--max-size"));
    assert!(stdout.contains("--verbose"));
    assert!(stdout.contains("--quiet"));
    assert!(stdout.contains("--no-resume"));
    assert!(stdout.contains("--delay"));
}

#[test]
fn no_args_shows_error() {
    let output = Command::new(binary_path())
        .output()
        .expect("failed to run");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("required"));
}

#[test]
fn invalid_url_shows_error() {
    let out = tmp_output("invalid_url");
    let output = Command::new(binary_path())
        .args(["not-a-url", "-o", out.to_str().unwrap(), "--no-resume", "--delay", "0"])
        .output()
        .expect("failed to run");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Invalid URL") || stderr.contains("error") || stderr.contains("Error"));
    cleanup(&out);
}

#[test]
fn verbose_and_quiet_conflict() {
    let output = Command::new(binary_path())
        .args(["https://example.com", "-v", "-q"])
        .output()
        .expect("failed to run");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Cannot use --verbose and --quiet together"));
}

#[test]
fn invalid_max_size() {
    let out = tmp_output("bad_size");
    let output = Command::new(binary_path())
        .args(["https://example.com", "-o", out.to_str().unwrap(), "--max-size", "abc", "--no-resume", "--delay", "0"])
        .output()
        .expect("failed to run");
    assert!(!output.status.success());
    cleanup(&out);
}

#[test]
fn invalid_exclude_pattern() {
    let out = tmp_output("bad_exclude");
    let output = Command::new(binary_path())
        .args(["https://example.com", "-o", out.to_str().unwrap(), "--exclude", "[invalid", "--no-resume", "--delay", "0"])
        .output()
        .expect("failed to run");
    assert!(!output.status.success());
    cleanup(&out);
}

#[test]
fn download_example_com() {
    let out = tmp_output("example_com");
    let output = Command::new(binary_path())
        .args([
            "https://example.com",
            "-o", out.to_str().unwrap(),
            "--no-resume",
            "--delay", "0",
            "-q",
        ])
        .output()
        .expect("failed to run");

    if !output.status.success() {
        // Network might not be available in CI
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Failed to fetch") || stderr.contains("error sending request") {
            eprintln!("Skipping network test: {}", stderr);
            cleanup(&out);
            return;
        }
    }

    assert!(output.status.success());

    // Check index.html was created
    let index = out.join("index.html");
    assert!(index.exists(), "index.html should exist");
    let content = fs::read_to_string(&index).unwrap();
    assert!(content.contains("Example Domain"));

    cleanup(&out);
}

#[test]
fn download_with_warc() {
    let out = tmp_output("warc_test");
    let output = Command::new(binary_path())
        .args([
            "https://example.com",
            "-o", out.to_str().unwrap(),
            "--no-resume",
            "--delay", "0",
            "--warc",
            "-q",
        ])
        .output()
        .expect("failed to run");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Failed to fetch") || stderr.contains("error sending request") {
            eprintln!("Skipping network test: {}", stderr);
            cleanup(&out);
            return;
        }
    }

    // Check WARC file exists
    let warc_files: Vec<_> = fs::read_dir(&out)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|x| x == "warc")
                .unwrap_or(false)
        })
        .collect();

    assert!(!warc_files.is_empty(), "WARC file should be created");

    let warc_content = fs::read_to_string(warc_files[0].path()).unwrap();
    assert!(warc_content.contains("WARC/1.0"));
    assert!(warc_content.contains("WARC-Type: response"));

    cleanup(&out);
}

#[test]
fn resume_skips_already_downloaded() {
    let out = tmp_output("resume_test");

    // First run
    let output1 = Command::new(binary_path())
        .args([
            "https://example.com",
            "-o", out.to_str().unwrap(),
            "--delay", "0",
            "-q",
        ])
        .output()
        .expect("failed to run");

    if !output1.status.success() {
        let stderr = String::from_utf8_lossy(&output1.stderr);
        if stderr.contains("Failed to fetch") || stderr.contains("error sending request") {
            cleanup(&out);
            return;
        }
    }

    // Check state file exists
    let state_file = out.join(".sitegrabber-state");
    assert!(state_file.exists(), "State file should exist after first run");
    let state_content = fs::read_to_string(&state_file).unwrap();
    assert!(!state_content.is_empty());

    // Second run should show "Skipped"
    let output2 = Command::new(binary_path())
        .args([
            "https://example.com",
            "-o", out.to_str().unwrap(),
            "--delay", "0",
        ])
        .output()
        .expect("failed to run");

    assert!(output2.status.success());
    let stdout2 = String::from_utf8_lossy(&output2.stdout);
    assert!(
        stdout2.contains("Skipped") || stdout2.contains("skipped"),
        "Second run should mention skipped URLs: {}",
        stdout2
    );

    cleanup(&out);
}

#[test]
fn quiet_mode_no_output() {
    let out = tmp_output("quiet_test");
    let output = Command::new(binary_path())
        .args([
            "https://example.com",
            "-o", out.to_str().unwrap(),
            "--no-resume",
            "--delay", "0",
            "-q",
        ])
        .output()
        .expect("failed to run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.is_empty() || stdout.trim().is_empty(), "Quiet mode should produce no stdout");

    cleanup(&out);
}

#[test]
fn custom_output_directory() {
    let out = tmp_output("custom_out");
    let output = Command::new(binary_path())
        .args([
            "https://example.com",
            "-o", out.to_str().unwrap(),
            "--no-resume",
            "--delay", "0",
            "-q",
        ])
        .output()
        .expect("failed to run");

    if output.status.success() {
        assert!(out.exists());
        assert!(out.join("index.html").exists());
    }

    cleanup(&out);
}

#[test]
fn no_resume_flag_skips_state() {
    let out = tmp_output("no_resume");

    // Run with --no-resume
    Command::new(binary_path())
        .args([
            "https://example.com",
            "-o", out.to_str().unwrap(),
            "--no-resume",
            "--delay", "0",
            "-q",
        ])
        .output()
        .expect("failed to run");

    // State file should still be created by mark_done
    // But if we run again with --no-resume, it should re-download
    let output2 = Command::new(binary_path())
        .args([
            "https://example.com",
            "-o", out.to_str().unwrap(),
            "--no-resume",
            "--delay", "0",
        ])
        .output()
        .expect("failed to run");

    if output2.status.success() {
        let stdout = String::from_utf8_lossy(&output2.stdout);
        // Should NOT mention skipping since --no-resume
        assert!(
            !stdout.contains("Skipped"),
            "--no-resume should not skip: {}",
            stdout
        );
    }

    cleanup(&out);
}

mod support;

use std::fs;

use support::{TestRepo, git};

#[test]
fn target_commands_accept_dot_and_repeated_separator_paths() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.dir.path().join("src")).expect("create source directory");
    fs::write(repo.dir.path().join("src/lib.rs"), "pub fn target() {}\n")
        .expect("write target file");
    git(repo.dir.path(), ["add", "src/lib.rs"]);
    git(repo.dir.path(), ["commit", "-m", "Create target file"]);
    repo.index();

    for path in ["./src/lib.rs", "./src//lib.rs"] {
        let why = repo.run(["why", path, "--line", "1"]);
        assert_eq!(
            why.status.code(),
            Some(0),
            "why {path}: {}",
            String::from_utf8_lossy(&why.stderr)
        );

        let regression = repo.run(["regression", "target path", "--path", path]);
        assert_eq!(
            regression.status.code(),
            Some(0),
            "regression {path}: {}",
            String::from_utf8_lossy(&regression.stderr)
        );

        let timeline = repo.run(["timeline", path]);
        assert_eq!(
            timeline.status.code(),
            Some(0),
            "timeline {path}: {}",
            String::from_utf8_lossy(&timeline.stderr)
        );
    }
}

#[cfg(windows)]
#[test]
fn target_commands_reject_drive_relative_paths() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.dir.path().join("src")).expect("create source directory");
    fs::write(repo.dir.path().join("src/lib.rs"), "pub fn target() {}\n")
        .expect("write target file");
    git(repo.dir.path(), ["add", "src/lib.rs"]);
    git(repo.dir.path(), ["commit", "-m", "Create target file"]);
    repo.index();

    let output = repo.run(["timeline", r"C:src\lib.rs"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("path must be repository-relative"),
        "drive-relative path should be rejected during validation: {stderr}"
    );
}

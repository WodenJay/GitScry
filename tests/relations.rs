use std::{
    ffi::OsStr,
    fs,
    path::Path,
    process::{Command, Output},
};

use tempfile::TempDir;

struct TestRepo {
    dir: TempDir,
}

impl TestRepo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temporary repository");
        git(dir.path(), ["init", "--initial-branch=main"]);
        git(dir.path(), ["config", "user.name", "GitScry Test"]);
        git(
            dir.path(),
            ["config", "user.email", "gitscry@example.invalid"],
        );
        Self { dir }
    }

    fn commit_files(&self, files: &[(&str, &[u8])], message: &str) {
        for (path, contents) in files {
            let path = Path::new(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(self.dir.path().join(parent)).expect("create parent directory");
            }
            fs::write(self.dir.path().join(path), contents).expect("write tracked file");
        }
        git(self.dir.path(), ["add", "--all"]);
        git(self.dir.path(), ["commit", "-m", message]);
    }

    fn remove(&self, path: &str, message: &str) {
        fs::remove_file(self.dir.path().join(path)).expect("remove tracked file");
        git(self.dir.path(), ["add", "--all"]);
        git(self.dir.path(), ["commit", "-m", message]);
    }

    fn rename(&self, old: &str, new: &str, message: &str) {
        if let Some(parent) = Path::new(new).parent() {
            fs::create_dir_all(self.dir.path().join(parent)).expect("create rename directory");
        }
        git(self.dir.path(), ["mv", old, new]);
        git(self.dir.path(), ["commit", "-m", message]);
    }

    fn commit_mass_change(&self, seed: &str, candidate: &str) {
        fs::write(self.dir.path().join(seed), b"seed changed\n").expect("write seed file");
        fs::write(self.dir.path().join(candidate), b"candidate changed\n")
            .expect("write candidate file");
        fs::create_dir_all(self.dir.path().join("generated")).expect("create generated directory");
        for index in 0..51 {
            let path = format!("generated/{index}.txt");
            fs::write(self.dir.path().join(path), b"generated\n").expect("write generated file");
        }
        git(self.dir.path(), ["add", "--all"]);
        git(self.dir.path(), ["commit", "-m", "mass change"]);
    }

    fn run<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new(env!("CARGO_BIN_EXE_gitscry"))
            .args(args)
            .current_dir(self.dir.path())
            .output()
            .expect("run gitscry")
    }
}

fn git<I, S>(cwd: &Path, args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn related_merges_multiple_seeds_and_cites_one_candidate_once() {
    let repo = TestRepo::new();
    repo.commit_files(&[("src/alpha.rs", b"alpha\n")], "alpha");
    repo.commit_files(&[("src/beta.rs", b"beta\n")], "beta");
    repo.commit_files(
        &[
            ("src/alpha.rs", b"alpha one\n"),
            ("src/shared.rs", b"one\n"),
        ],
        "alpha shared",
    );
    repo.commit_files(
        &[("src/beta.rs", b"beta one\n"), ("src/shared.rs", b"two\n")],
        "beta shared",
    );
    repo.commit_files(
        &[
            ("src/alpha.rs", b"alpha two\n"),
            ("src/beta.rs", b"beta two\n"),
            ("src/shared.rs", b"three\n"),
        ],
        "both shared",
    );
    repo.remove("src/alpha.rs", "remove alpha");

    let output = repo.run(["related", "src/alpha.rs", "src/beta.rs"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert_eq!(
        text.matches("candidate path: src/shared.rs").count(),
        1,
        "{text}"
    );
    assert!(text.contains("co-change count: 3"), "{text}");
    assert!(text.contains("proportion: 100.0%"), "{text}");
    assert!(text.contains("supporting commits: 3"), "{text}");
    assert!(text.contains("confidence:"), "{text}");
    assert!(text.contains("basis:"), "{text}");
}

#[test]
fn related_keeps_historical_seeds_and_ignores_mass_change_noise() {
    let repo = TestRepo::new();
    repo.commit_files(&[("src/legacy.rs", b"legacy\n")], "legacy");
    repo.commit_files(
        &[
            ("src/legacy.rs", b"legacy relation\n"),
            ("src/shared.rs", b"shared\n"),
        ],
        "ordinary relation",
    );
    repo.commit_mass_change("src/legacy.rs", "src/shared.rs");
    repo.remove("src/legacy.rs", "remove legacy");

    let output = repo.run(["related", "src/legacy.rs"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("candidate path: src/shared.rs"), "{text}");
    assert!(text.contains("co-change count: 1"), "{text}");
    assert!(text.contains("proportion: 100.0%"), "{text}");
}

#[test]
fn tests_filter_deleted_candidates_and_warn_about_rename_continuity() {
    let repo = TestRepo::new();
    repo.commit_files(
        &[
            ("src/widget.py", b"widget\n"),
            ("tests/test_widget.py", b"test\n"),
            ("tests/test_widget_satellite.py", b"satellite\n"),
            ("tests/test_old_widget.py", b"old\n"),
            ("tests/test_deleted_widget.py", b"deleted\n"),
        ],
        "seed test history",
    );
    repo.commit_files(
        &[
            ("src/widget.py", b"widget one\n"),
            ("tests/test_widget.py", b"test one\n"),
            ("tests/test_widget_satellite.py", b"satellite one\n"),
            ("tests/test_old_widget.py", b"old one\n"),
            ("tests/test_deleted_widget.py", b"deleted one\n"),
        ],
        "update widget tests",
    );
    repo.remove("tests/test_deleted_widget.py", "delete obsolete test");
    repo.rename(
        "tests/test_old_widget.py",
        "tests/test_renamed_widget.py",
        "rename widget test",
    );

    let output = repo.run(["tests", "src/widget.py"]);

    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("candidate path: tests/test_widget.py"),
        "{text}"
    );
    assert!(
        text.contains("candidate path: tests/test_widget_satellite.py"),
        "{text}"
    );
    assert!(
        text.contains("candidate path: tests/test_renamed_widget.py"),
        "{text}"
    );
    assert!(!text.contains("tests/test_deleted_widget.py"), "{text}");
    assert!(!text.contains("tests/test_old_widget.py"), "{text}");
    assert!(text.contains("beyond mirrored test name"), "{text}");
    assert!(stderr(&output).contains("warning:"), "{}", stderr(&output));
    assert!(stderr(&output).contains("rename"), "{}", stderr(&output));
}

#[test]
fn relation_commands_have_fixed_empty_results() {
    let repo = TestRepo::new();
    repo.commit_files(&[("src/only.rs", b"only\n")], "only");

    let related = repo.run(["related", "src/only.rs"]);
    assert_eq!(related.status.code(), Some(0));
    assert_eq!(stdout(&related), "No historical relations found.\n");

    let tests = repo.run(["tests", "src/only.rs"]);
    assert_eq!(tests.status.code(), Some(0));
    assert_eq!(stdout(&tests), "No historically related tests found.\n");
}

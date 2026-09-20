#![allow(dead_code)]

use std::{ffi::OsStr, path::Path, process::Command};

use tempfile::TempDir;

pub(crate) struct TestRepo {
    pub(crate) dir: TempDir,
}

impl TestRepo {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temporary repository");
        git(dir.path(), ["init", "--initial-branch=main"]);
        git(dir.path(), ["config", "user.name", "GitScry Test"]);
        git(
            dir.path(),
            ["config", "user.email", "gitscry@example.invalid"],
        );
        Self { dir }
    }

    pub(crate) fn head(&self) -> String {
        git_stdout(self.dir.path(), ["rev-parse", "HEAD"])
    }

    pub(crate) fn run<I, S>(&self, args: I) -> std::process::Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Self::run_at(self.dir.path(), args)
    }

    pub(crate) fn run_at<I, S>(cwd: &Path, args: I) -> std::process::Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new(env!("CARGO_BIN_EXE_gitscry"))
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", cwd.join("global-config"))
            .output()
            .expect("run gitscry")
    }
}

pub(crate) fn git<I, S>(cwd: &Path, args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", cwd.join("global-config"))
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn git_stdout<I, S>(cwd: &Path, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", cwd.join("global-config"))
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git output is utf-8")
        .trim()
        .to_owned()
}

#![allow(dead_code)]

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

use tempfile::TempDir;

pub(crate) struct TestRepo {
    pub(crate) dir: TempDir,
    user_data: TempDir,
}

impl TestRepo {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().expect("create temporary repository");
        git(dir.path(), ["init", "--initial-branch=main"]);
        // Keep Git's generated repository configuration, appending only fixture identity.
        std::io::Write::write_all(
            &mut std::fs::OpenOptions::new()
                .append(true)
                .open(dir.path().join(".git/config"))
                .expect("open repository config"),
            b"\n[user]\n\tname = GitScry Test\n\temail = gitscry@example.invalid\n",
        )
        .expect("configure fixture identity");
        let user_data = tempfile::tempdir().expect("create temporary user data directory");
        Self { dir, user_data }
    }

    pub(crate) fn head(&self) -> String {
        git_stdout(self.dir.path(), ["rev-parse", "HEAD"])
    }

    pub(crate) fn head_oid(&self, revision: &str) -> String {
        git_stdout(self.dir.path(), ["rev-parse", revision])
    }

    pub(crate) fn common_dir(&self) -> PathBuf {
        git_common_dir(self.dir.path())
    }
    pub(crate) fn cache_dir(&self) -> PathBuf {
        self.common_dir().join("gitscry")
    }

    pub(crate) fn run<I, S>(&self, args: I) -> std::process::Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Self::run_at_with_user_data_dir(self.dir.path(), args, self.user_data.path())
    }

    pub(crate) fn run_with_user_data_dir<I, S>(
        &self,
        args: I,
        user_data_dir: &Path,
    ) -> std::process::Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Self::run_at_with_user_data_dir(self.dir.path(), args, user_data_dir)
    }

    pub(crate) fn user_data_dir(&self) -> &Path {
        self.user_data.path()
    }

    pub(crate) fn index(&self) {
        let output = self.run(["index"]);
        assert!(
            output.status.success(),
            "index failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub(crate) fn run_at<I, S>(cwd: &Path, args: I) -> std::process::Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let user_data = tempfile::tempdir().expect("create temporary user data directory");
        Self::run_at_with_user_data_dir(cwd, args, user_data.path())
    }

    pub(crate) fn run_at_with_user_data_dir<I, S>(
        cwd: &Path,
        args: I,
        user_data_dir: &Path,
    ) -> std::process::Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Self::command_at(cwd, user_data_dir)
            .args(args)
            .output()
            .expect("run gitscry")
    }

    pub(crate) fn command_at(cwd: &Path, user_data_dir: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_gitscry"));
        command
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", cwd.join("global-config"))
            .env("LOCALAPPDATA", user_data_dir)
            .env("XDG_DATA_HOME", user_data_dir.join("xdg"))
            .env("HOME", user_data_dir.join("home"));
        command
    }
}

pub(crate) fn isolated_gitscry_command(user_data_dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gitscry"));
    command
        .env("LOCALAPPDATA", user_data_dir)
        .env("XDG_DATA_HOME", user_data_dir.join("xdg"))
        .env("HOME", user_data_dir.join("home"));
    command
}

pub(crate) fn git_command(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", cwd.join("global-config"));
    command
}

pub(crate) fn git<I, S>(cwd: &Path, args: I)
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = git_command(cwd).args(args).output().expect("run git");
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
    let output = git_command(cwd).args(args).output().expect("run git");
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

pub(crate) fn git_common_dir(cwd: &Path) -> PathBuf {
    PathBuf::from(git_stdout(
        cwd,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ))
}

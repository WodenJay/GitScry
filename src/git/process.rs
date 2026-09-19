use std::{
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use crate::app::AppError;

pub(super) struct Git {
    root: PathBuf,
}

impl Git {
    pub(super) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(super) fn text<I, S>(&self, args: I) -> Result<String, AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let bytes = self.output(args, &[])?;
        String::from_utf8(bytes).map_err(|error| {
            AppError::operational(format!(
                "error: parsing Git machine output as UTF-8: {error}"
            ))
        })
    }

    pub(super) fn output<I, S>(&self, args: I, input: &[u8]) -> Result<Vec<u8>, AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.run(args, input)?;
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::operational(format!(
                "error: Git command failed: {}",
                detail.trim()
            )));
        }
        Ok(output.stdout)
    }

    pub(super) fn success<I, S>(&self, args: I) -> Result<bool, AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.run(args, &[])?;
        if output.status.success() {
            return Ok(true);
        }
        if output.status.code() == Some(1) {
            return Ok(false);
        }
        let detail = String::from_utf8_lossy(&output.stderr);
        Err(AppError::operational(format!(
            "error: Git command failed: {}",
            detail.trim()
        )))
    }

    pub(super) fn missing_objects(&self, object_ids: &[String]) -> Result<Vec<String>, AppError> {
        if object_ids.is_empty() {
            return Ok(Vec::new());
        }
        let input = object_ids
            .iter()
            .map(|oid| format!("{oid}\n"))
            .collect::<String>();
        let output = self.output(["cat-file", "--batch-check"], input.as_bytes())?;
        let mut missing = Vec::new();
        for (expected, line) in object_ids.iter().zip(output.split(|byte| *byte == b'\n')) {
            if line.is_empty() {
                return Err(AppError::operational(
                    "error: Git object check returned a truncated response",
                ));
            }
            let fields = line.split(|byte| *byte == b' ').collect::<Vec<_>>();
            if fields.first().copied() != Some(expected.as_bytes()) {
                return Err(AppError::operational(
                    "error: Git object check returned an unexpected object",
                ));
            }
            if fields.get(1).copied() == Some(b"missing") {
                missing.push(expected.clone());
            } else if fields.len() < 3 {
                return Err(AppError::operational(
                    "error: Git object check returned an invalid response",
                ));
            }
        }
        Ok(missing)
    }

    fn run<I, S>(&self, args: I, input: &[u8]) -> Result<Output, AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("git");
        command
            .arg("--no-pager")
            .args(args)
            .current_dir(&self.root)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().map_err(|error| {
            AppError::operational(format!(
                "error: starting Git: {error}; ensure Git is installed"
            ))
        })?;
        let writer = if input.is_empty() {
            drop(child.stdin.take());
            None
        } else {
            let mut stdin = child.stdin.take().expect("piped Git stdin");
            let input = input.to_vec();
            Some(std::thread::spawn(move || stdin.write_all(&input)))
        };
        let output = child
            .wait_with_output()
            .map_err(|error| AppError::operational(format!("error: waiting for Git: {error}")))?;
        if let Some(writer) = writer {
            writer
                .join()
                .map_err(|_| AppError::operational("error: Git input writer panicked"))?
                .map_err(|error| {
                    AppError::operational(format!("error: sending input to Git: {error}"))
                })?;
        }
        Ok(output)
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }
}

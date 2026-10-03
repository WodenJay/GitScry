use std::{
    ffi::OsStr,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
};

use crate::app::AppError;

#[derive(Clone)]
pub(super) struct Git {
    root: PathBuf,
    index: Option<PathBuf>,
}

impl Git {
    pub(super) fn new(root: PathBuf) -> Self {
        Self { root, index: None }
    }

    pub(super) fn with_index(&self, index: PathBuf) -> Self {
        Self {
            root: self.root.clone(),
            index: Some(index),
        }
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

    /// Stop the child once its output exceeds a caller's content budget.
    pub(super) fn limited_output<I, S>(
        &self,
        args: I,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut bytes = Vec::new();
        let mut exceeded = false;
        let result = self.stream(args, &[], |chunk| {
            if bytes.len().saturating_add(chunk.len()) > limit {
                exceeded = true;
                return Err(AppError::operational("content output limit reached"));
            }
            bytes.extend_from_slice(chunk);
            Ok(())
        });
        if exceeded {
            Ok(None)
        } else {
            result?;
            Ok(Some(bytes))
        }
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
        let command = self.run(["cat-file", "--batch-check"], input.as_bytes())?;
        if !command.status.success() || !command.stderr.is_empty() {
            let detail = String::from_utf8_lossy(&command.stderr);
            let reason = if detail.trim().is_empty() {
                format!("exit status {}", command.status)
            } else {
                detail.trim().to_owned()
            };
            return Err(AppError::operational(format!(
                "error: Git object check failed: {reason}"
            )));
        }
        parse_missing_objects(object_ids, &command.stdout)
    }

    pub(super) fn stream<I, S, F>(
        &self,
        args: I,
        input: &[String],
        mut consume: F,
    ) -> Result<(), AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
        F: FnMut(&[u8]) -> Result<(), AppError>,
    {
        let mut child = self.spawn(args)?;
        let mut stdin = child.stdin.take().expect("piped Git stdin");
        let mut stdout = child.stdout.take().expect("piped Git stdout");
        let mut stderr = child.stderr.take().expect("piped Git stderr");

        std::thread::scope(|scope| {
            let writer = scope.spawn(move || {
                for line in input {
                    stdin.write_all(line.as_bytes())?;
                }
                Ok::<(), std::io::Error>(())
            });
            let error_reader = scope.spawn(move || {
                let mut bytes = Vec::new();
                stderr.read_to_end(&mut bytes).map(|_| bytes)
            });

            let mut buffer = [0_u8; 64 * 1024];
            let streamed = loop {
                match stdout.read(&mut buffer) {
                    Ok(0) => break Ok(()),
                    Ok(length) => {
                        if let Err(error) = consume(&buffer[..length]) {
                            break Err(error);
                        }
                    }
                    Err(error) => {
                        break Err(AppError::operational(format!(
                            "error: reading Git output: {error}"
                        )));
                    }
                }
            };
            if streamed.is_err() {
                let _ = child.kill();
            }
            let status = match child.wait() {
                Ok(status) => status,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AppError::operational(format!(
                        "error: waiting for Git: {error}"
                    )));
                }
            };
            let written = writer
                .join()
                .map_err(|_| AppError::operational("error: Git input writer panicked"))?;
            let stderr = error_reader
                .join()
                .map_err(|_| AppError::operational("error: Git stderr reader panicked"))?
                .map_err(|error| {
                    AppError::operational(format!("error: reading Git error output: {error}"))
                })?;

            streamed?;
            if !status.success() {
                return Err(git_failure(&stderr));
            }
            check_input_result(written)?;
            Ok(())
        })
    }

    fn run<I, S>(&self, args: I, input: &[u8]) -> Result<Output, AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut child = self.spawn(args)?;
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
            let written = writer
                .join()
                .map_err(|_| AppError::operational("error: Git input writer panicked"))?;
            check_input_result(written)?;
        }
        Ok(output)
    }

    fn spawn<I, S>(&self, args: I) -> Result<Child, AppError>
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
        if let Some(index) = &self.index {
            command.env("GIT_INDEX_FILE", index);
        }
        command.spawn().map_err(|error| {
            AppError::operational(format!(
                "error: starting Git: {error}; ensure Git is installed"
            ))
        })
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }
}
fn parse_missing_objects(object_ids: &[String], output: &[u8]) -> Result<Vec<String>, AppError> {
    let response = output.strip_suffix(b"\n").ok_or_else(|| {
        AppError::operational("error: Git object check returned a truncated response")
    })?;
    let lines = response.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    if lines.len() < object_ids.len() {
        return Err(AppError::operational(
            "error: Git object check returned a truncated response",
        ));
    }
    if lines.len() > object_ids.len() {
        return Err(AppError::operational(
            "error: Git object check returned an unexpected number of responses",
        ));
    }

    let mut missing = Vec::new();
    for (expected, line) in object_ids.iter().zip(lines) {
        let fields = line.split(|byte| *byte == b' ').collect::<Vec<_>>();
        if fields.first().copied() != Some(expected.as_bytes()) {
            return Err(AppError::operational(
                "error: Git object check returned an unexpected object",
            ));
        }
        if fields.len() == 2 && fields[1] == b"missing" {
            missing.push(expected.clone());
            continue;
        }
        let valid_object = fields.len() == 3
            && ["blob", "tree", "commit", "tag"]
                .iter()
                .any(|object_type| fields[1] == object_type.as_bytes());
        let valid_size = fields.len() == 3
            && fields[2].iter().all(|byte| byte.is_ascii_digit())
            && std::str::from_utf8(fields[2])
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .is_some();
        if !valid_object || !valid_size {
            return Err(AppError::operational(
                "error: Git object check returned an invalid response",
            ));
        }
    }
    Ok(missing)
}

fn check_input_result(written: std::io::Result<()>) -> Result<(), AppError> {
    match written {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(AppError::operational(format!(
            "error: sending input to Git: {error}"
        ))),
    }
}

fn git_failure(stderr: &[u8]) -> AppError {
    let detail = String::from_utf8_lossy(stderr);
    AppError::operational(format!("error: Git command failed: {}", detail.trim()))
}

#[cfg(test)]
mod tests {
    use super::{Git, parse_missing_objects};

    #[test]
    fn missing_object_parser_accepts_exact_responses() {
        let object_ids = vec!["present".to_owned(), "absent".to_owned()];
        let missing =
            parse_missing_objects(&object_ids, b"present commit 12\nabsent missing\n").unwrap();
        assert_eq!(missing, vec!["absent".to_owned()]);
    }

    #[test]
    fn missing_object_parser_rejects_malformed_and_extra_responses() {
        let one_object = vec!["a".to_owned()];
        for output in [
            &b"a missing extra\n"[..],
            b"a commit 1 extra\n",
            b"a commit not-a-size\n",
            b"a unknown 1\n",
            b"a commit 1",
            b"a commit 1\nb missing\n",
        ] {
            assert!(
                parse_missing_objects(&one_object, output).is_err(),
                "accepted malformed response: {output:?}"
            );
        }

        let two_objects = vec!["a".to_owned(), "b".to_owned()];
        assert!(
            parse_missing_objects(&two_objects, b"a missing\n").is_err(),
            "accepted a truncated response"
        );
    }

    #[test]
    fn stream_surfaces_an_early_child_failure() {
        let git = Git::new(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        let input = vec!["HEAD\n".to_owned()];
        let error = git
            .stream(["definitely-not-a-git-command"], &input, |_| Ok(()))
            .unwrap_err();

        assert!(error.to_string().contains("Git command failed"));
    }

    #[test]
    fn broken_input_pipe_defers_to_the_child_result() {
        let broken_pipe = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
        super::check_input_result(Err(broken_pipe)).unwrap();
    }
}

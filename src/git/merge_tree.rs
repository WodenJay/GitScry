use std::{
    ffi::OsStr,
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::app::AppError;

use super::Repository;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const MAX_TEXT_BLOB_BYTES: usize = 1024 * 1024;

pub(crate) struct MergeTree {
    scratch: ScratchRepository,
    version: String,
    supported: bool,
}

pub(crate) struct Replay {
    pub(crate) tree: String,
    pub(crate) stages: Vec<StageEntry>,
    pub(crate) conflicted: bool,
}

#[derive(Clone)]
pub(crate) struct StageEntry {
    pub(crate) path: Vec<u8>,
    pub(crate) mode: Vec<u8>,
    pub(crate) oid: String,
    pub(crate) stage: u8,
}

pub(crate) struct TreeEntry {
    pub(crate) path: Vec<u8>,
    pub(crate) mode: Vec<u8>,
    pub(crate) oid: String,
}

impl MergeTree {
    pub(crate) fn new(repository: &Repository) -> Result<Self, AppError> {
        let version = repository.git.text(["--version"])?;
        let version = version.trim().to_owned();
        let supported = git_version_at_least_240(&version);
        let object_format = repository.object_format()?;
        let source_objects = PathBuf::from(
            repository
                .git
                .text([
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    "objects",
                ])?
                .trim(),
        );
        let scratch = ScratchRepository::new(&object_format, source_objects)?;
        Ok(Self {
            scratch,
            version,
            supported,
        })
    }

    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    pub(crate) fn supported(&self) -> bool {
        self.supported
    }

    pub(crate) fn merge_bases(&self, left: &str, right: &str) -> Result<Vec<String>, AppError> {
        let output = self.run(["merge-base", "--all", left, right].map(str::to_owned), &[])?;
        match output.status.code() {
            Some(0) => output
                .stdout
                .split(|byte| *byte == b'\n')
                .filter(|oid| !oid.is_empty())
                .map(|oid| {
                    if valid_oid(oid) {
                        Ok(String::from_utf8_lossy(oid).into_owned())
                    } else {
                        Err(AppError::operational(
                            "error: Git returned an invalid merge base",
                        ))
                    }
                })
                .collect(),
            Some(1) => Ok(Vec::new()),
            _ => Err(git_error(
                "finding merge bases",
                &output.stderr,
                output.status,
            )),
        }
    }

    pub(crate) fn has_unsupported_attributes(&self, revisions: &[&str]) -> Result<bool, AppError> {
        for revision in revisions {
            let entries = self.tree_entries(revision)?;
            if entries.is_empty() {
                continue;
            }
            let mut input = Vec::new();
            for entry in entries {
                input.extend_from_slice(&entry.path);
                input.push(0);
            }
            let output = self.run(
                [
                    "check-attr".to_owned(),
                    "-z".to_owned(),
                    format!("--source={revision}"),
                    "--stdin".to_owned(),
                    "merge".to_owned(),
                ],
                &input,
            )?;
            ensure_success(output.status, &output.stderr, "checking merge attributes")?;
            let fields = output.stdout.split(|byte| *byte == 0).collect::<Vec<_>>();
            if fields.len() % 3 != 1 {
                return Err(AppError::operational(
                    "error: Git returned malformed merge-attribute output",
                ));
            }
            for record in fields[..fields.len() - 1].as_chunks::<3>().0 {
                if record[2].is_empty() {
                    continue;
                }
                let value = record[2];
                if !matches!(
                    value,
                    b"unspecified" | b"set" | b"unset" | b"text" | b"binary" | b"union"
                ) {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub(crate) fn replay(
        &self,
        base: &str,
        parent1: &str,
        parent2: &str,
    ) -> Result<Replay, AppError> {
        let output = self.run(
            [
                "merge-tree".to_owned(),
                "--write-tree".to_owned(),
                "-z".to_owned(),
                format!("--merge-base={base}"),
                "-Xfind-renames".to_owned(),
                parent1.to_owned(),
                parent2.to_owned(),
            ],
            &[],
        )?;
        let conflicted = match output.status.code() {
            Some(0) => false,
            Some(1) => true,
            _ => {
                return Err(git_error(
                    "reconstructing merge",
                    &output.stderr,
                    output.status,
                ));
            }
        };
        let mut records = output.stdout.split(|byte| *byte == 0);
        let tree = records
            .next()
            .filter(|oid| valid_oid(oid))
            .map(|oid| String::from_utf8_lossy(oid).into_owned())
            .ok_or_else(|| AppError::operational("error: Git returned a malformed merge tree"))?;
        let mut stages = Vec::new();
        for record in records {
            if record.is_empty() {
                break;
            }
            stages.push(parse_stage_entry(record)?);
        }
        Ok(Replay {
            tree,
            stages,
            conflicted,
        })
    }

    pub(crate) fn tree_entries(&self, revision: &str) -> Result<Vec<TreeEntry>, AppError> {
        let output = self.run(["ls-tree", "-r", "-z", revision].map(str::to_owned), &[])?;
        ensure_success(output.status, &output.stderr, "reading a Git tree")?;
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .map(parse_tree_entry)
            .collect()
    }
    pub(crate) fn tree_entry(
        &self,
        revision: &str,
        path: &str,
    ) -> Result<Option<TreeEntry>, AppError> {
        let pathspec = format!(":(literal){path}");
        let output = self.run(
            [
                "ls-tree".to_owned(),
                "-r".to_owned(),
                "-z".to_owned(),
                revision.to_owned(),
                "--".to_owned(),
                pathspec,
            ],
            &[],
        )?;
        ensure_success(output.status, &output.stderr, "reading a Git tree path")?;
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|record| !record.is_empty())
            .map(parse_tree_entry)
            .collect::<Result<Vec<_>, _>>()
            .map(|entries| {
                entries
                    .into_iter()
                    .find(|entry| entry.path == path.as_bytes())
            })
    }

    pub(crate) fn blob(&self, oid: &str) -> Result<Option<Vec<u8>>, AppError> {
        let output = self.run(["cat-file", "blob", oid].map(str::to_owned), &[])?;
        ensure_success(output.status, &output.stderr, "reading a Git blob")?;
        if output.stdout.len() > MAX_TEXT_BLOB_BYTES
            || output.stdout.contains(&0)
            || std::str::from_utf8(&output.stdout).is_err()
        {
            return Ok(None);
        }
        Ok(Some(output.stdout))
    }

    fn run<I, S>(&self, args: I, input: &[u8]) -> Result<Output, AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.scratch.run(args, input)
    }
}

struct ScratchRepository {
    root: PathBuf,
    git_dir: PathBuf,
    object_dir: PathBuf,
    source_objects: PathBuf,
    empty_global_config: PathBuf,
}

impl ScratchRepository {
    fn new(object_format: &str, source_objects: PathBuf) -> Result<Self, AppError> {
        if !matches!(object_format, "sha1" | "sha256") {
            return Err(AppError::operational(format!(
                "error: unsupported Git object format for isolated merge replay: {object_format}"
            )));
        }
        let root = create_scratch_root()?;
        let git_dir = root.join("git");
        let object_dir = root.join("objects");
        let empty_global_config = root.join("empty-global-config");
        fs::create_dir_all(git_dir.join("objects/info"))
            .map_err(|error| scratch_io_error("creating isolated object metadata", error))?;
        fs::create_dir_all(git_dir.join("info"))
            .map_err(|error| scratch_io_error("creating isolated Git info directory", error))?;
        fs::create_dir_all(git_dir.join("refs/heads"))
            .map_err(|error| scratch_io_error("creating isolated Git refs", error))?;
        fs::create_dir_all(&object_dir)
            .map_err(|error| scratch_io_error("creating isolated object directory", error))?;
        fs::write(&empty_global_config, b"")
            .map_err(|error| scratch_io_error("creating isolated Git config", error))?;
        fs::write(git_dir.join("HEAD"), b"ref: refs/heads/main\n")
            .map_err(|error| scratch_io_error("creating isolated Git HEAD", error))?;
        fs::write(git_dir.join("info/attributes"), b"")
            .map_err(|error| scratch_io_error("creating isolated Git attributes", error))?;
        let repository_format = if object_format == "sha256" { 1 } else { 0 };
        let extensions = if object_format == "sha256" {
            "[extensions]\n\tobjectformat = sha256\n"
        } else {
            ""
        };
        fs::write(
            git_dir.join("config"),
            format!(
                "[core]\n\trepositoryformatversion = {repository_format}\n\tbare = true\n\tfilemode = true\n[merge]\n\tdefault = text\n\trenames = true\n\tconflictstyle = merge\n{extensions}"
            ),
        )
        .map_err(|error| scratch_io_error("writing isolated Git config", error))?;
        Ok(Self {
            root,
            git_dir,
            object_dir,
            source_objects,
            empty_global_config,
        })
    }

    fn run<I, S>(&self, args: I, input: &[u8]) -> Result<Output, AppError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new("git");
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        command
            .current_dir(&self.root)
            .env("GIT_DIR", &self.git_dir)
            .env("GIT_OBJECT_DIRECTORY", &self.object_dir)
            .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &self.source_objects)
            .env("GIT_CONFIG_GLOBAL", &self.empty_global_config)
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "core.attributesFile")
            .env("GIT_CONFIG_VALUE_0", self.git_dir.join("info/attributes"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_ATTR_NOSYSTEM", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_PAGER", "cat")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            AppError::operational(format!("error: starting isolated Git replay: {error}"))
        })?;
        child
            .stdin
            .take()
            .expect("piped isolated Git stdin")
            .write_all(input)
            .map_err(|error| {
                AppError::operational(format!("error: writing isolated Git input: {error}"))
            })?;
        child.wait_with_output().map_err(|error| {
            AppError::operational(format!("error: waiting for isolated Git replay: {error}"))
        })
    }
}

impl Drop for ScratchRepository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn scratch_io_error(operation: &str, error: std::io::Error) -> AppError {
    AppError::operational(format!("error: {operation}: {error}"))
}

fn create_scratch_root() -> Result<PathBuf, AppError> {
    let parent = std::env::temp_dir();
    for _ in 0..100 {
        let id = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!("gitscry-merge-tree-{}-{id}", std::process::id()));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(AppError::operational(format!(
                    "error: creating isolated Git replay directory: {error}"
                )));
            }
        }
    }
    Err(AppError::operational(
        "error: could not allocate an isolated Git replay directory",
    ))
}

fn git_version_at_least_240(version: &str) -> bool {
    let Some(version) = version.strip_prefix("git version ") else {
        return false;
    };
    let mut parts = version.split('.');
    let (Some(major), Some(minor)) = (parts.next(), parts.next()) else {
        return false;
    };
    let (Ok(major), Ok(minor)) = (major.parse::<u32>(), minor.parse::<u32>()) else {
        return false;
    };
    (major, minor) >= (2, 40)
}

fn parse_stage_entry(record: &[u8]) -> Result<StageEntry, AppError> {
    let tab = record
        .iter()
        .position(|byte| *byte == b'\t')
        .ok_or_else(|| AppError::operational("error: Git returned a malformed merge stage"))?;
    let mut fields = record[..tab].split(|byte| *byte == b' ');
    let mode = fields.next().unwrap_or_default();
    let oid = fields.next().unwrap_or_default();
    let stage = fields.next().unwrap_or_default();
    if !matches!(mode, b"100644" | b"100755" | b"120000" | b"160000")
        || !valid_oid(oid)
        || fields.next().is_some()
    {
        return Err(AppError::operational(
            "error: Git returned an unsupported merge stage",
        ));
    }
    let stage = std::str::from_utf8(stage)
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|stage| (1..=3).contains(stage))
        .ok_or_else(|| AppError::operational("error: Git returned an invalid merge stage"))?;
    Ok(StageEntry {
        path: record[tab + 1..].to_vec(),
        mode: mode.to_vec(),
        oid: String::from_utf8_lossy(oid).into_owned(),
        stage,
    })
}

fn parse_tree_entry(record: &[u8]) -> Result<TreeEntry, AppError> {
    let tab = record
        .iter()
        .position(|byte| *byte == b'\t')
        .ok_or_else(|| AppError::operational("error: Git returned a malformed tree entry"))?;
    let mut fields = record[..tab].split(|byte| *byte == b' ');
    let mode = fields.next().unwrap_or_default();
    let kind = fields.next().unwrap_or_default();
    let oid = fields.next().unwrap_or_default();
    if !matches!(kind, b"blob" | b"commit") || !valid_oid(oid) || fields.next().is_some() {
        return Err(AppError::operational(
            "error: Git returned an unsupported tree entry",
        ));
    }
    Ok(TreeEntry {
        path: record[tab + 1..].to_vec(),
        mode: mode.to_vec(),
        oid: String::from_utf8_lossy(oid).into_owned(),
    })
}

fn valid_oid(oid: &[u8]) -> bool {
    matches!(oid.len(), 40 | 64) && oid.iter().all(u8::is_ascii_hexdigit)
}

fn ensure_success(
    status: std::process::ExitStatus,
    stderr: &[u8],
    operation: &str,
) -> Result<(), AppError> {
    if status.success() {
        Ok(())
    } else {
        Err(git_error(operation, stderr, status))
    }
}

fn git_error(operation: &str, stderr: &[u8], status: std::process::ExitStatus) -> AppError {
    let detail = String::from_utf8_lossy(stderr);
    AppError::operational(format!(
        "error: isolated Git failed while {operation}: {}",
        if detail.trim().is_empty() {
            status.to_string()
        } else {
            detail.trim().to_owned()
        }
    ))
}

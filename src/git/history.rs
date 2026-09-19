use std::{collections::HashSet, fs, path::PathBuf};

use crate::app::AppError;

use super::process::Git;

pub(crate) struct Snapshot {
    pub(crate) default_ref: String,
    pub(crate) tip: String,
    pub(crate) object_format: String,
    pub(crate) shallow_boundaries: Vec<String>,
    pub(crate) missing_objects: Vec<String>,
    pub(crate) commits: Vec<Commit>,
    pub(crate) changes: Vec<Change>,
    pub(crate) hunks: Vec<Hunk>,
}

pub(crate) struct Commit {
    pub(crate) oid: String,
    pub(crate) message: Vec<u8>,
    pub(crate) time: i64,
    pub(crate) parents: Vec<String>,
}

pub(crate) struct Change {
    pub(crate) commit_oid: String,
    pub(crate) ordinal: i64,
    pub(crate) status: String,
    pub(crate) old_path: Option<Vec<u8>>,
    pub(crate) new_path: Option<Vec<u8>>,
    pub(crate) old_blob: Option<String>,
    pub(crate) new_blob: Option<String>,
    pub(crate) old_mode: String,
    pub(crate) new_mode: String,
}

pub(crate) struct Hunk {
    pub(crate) commit_oid: String,
    pub(crate) change_ordinal: i64,
    pub(crate) ordinal: i64,
    pub(crate) old_start: i64,
    pub(crate) old_lines: i64,
    pub(crate) new_start: i64,
    pub(crate) new_lines: i64,
    pub(crate) text: Vec<u8>,
}

pub(super) fn read(
    git: &Git,
    default_ref: String,
    tip: String,
    object_format: String,
) -> Result<Snapshot, AppError> {
    let graph = read_graph(git, &tip)?;
    read_selected(
        git,
        default_ref,
        tip,
        object_format,
        &graph,
        &graph,
        Vec::new(),
    )
}

pub(super) fn read_incremental(
    git: &Git,
    default_ref: String,
    tip: String,
    object_format: String,
    cached_commits: &[String],
    refresh_commits: &[String],
    known_missing_objects: Vec<String>,
) -> Result<Snapshot, AppError> {
    let graph = read_graph(git, &tip)?;
    let cached = cached_commits.iter().collect::<HashSet<_>>();
    let refresh = refresh_commits.iter().collect::<HashSet<_>>();
    let selected = graph
        .iter()
        .filter(|oid| !cached.contains(oid) || refresh.contains(oid))
        .cloned()
        .collect::<Vec<_>>();
    read_selected(
        git,
        default_ref,
        tip,
        object_format,
        &graph,
        &selected,
        known_missing_objects,
    )
}

fn read_graph(git: &Git, tip: &str) -> Result<Vec<String>, AppError> {
    let graph = git.output(["rev-list", "--reverse", "--topo-order", tip], &[])?;
    parse_graph(&graph)
}

fn read_selected(
    git: &Git,
    default_ref: String,
    tip: String,
    object_format: String,
    graph: &[String],
    selected: &[String],
    known_missing_objects: Vec<String>,
) -> Result<Snapshot, AppError> {
    let commits = read_commits(git, selected)?;
    let graph_set = graph.iter().map(String::as_str).collect::<HashSet<_>>();
    let selected_set = selected.iter().map(String::as_str).collect::<HashSet<_>>();
    let diff_input = commits
        .iter()
        .filter_map(|commit| match commit.parents.first() {
            Some(parent) if graph_set.contains(parent.as_str()) => {
                Some(format!("{} {parent}\n", commit.oid))
            }
            Some(_) => None,
            None => Some(format!("{}\n", commit.oid)),
        })
        .collect::<String>();
    let raw = git.output(
        [
            "diff-tree",
            "--stdin",
            "--root",
            "-r",
            "-M",
            "--raw",
            "-z",
            "--full-index",
        ],
        diff_input.as_bytes(),
    )?;
    let changes = parse_changes(&raw, &selected_set)?;
    let object_ids = changes
        .iter()
        .flat_map(|change| [&change.old_blob, &change.new_blob])
        .flatten()
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut missing_objects = git.missing_objects(&object_ids)?;
    missing_objects.extend(known_missing_objects);
    missing_objects.sort();
    missing_objects.dedup();

    let hunks = if missing_objects.is_empty() {
        let patch = git.output(
            [
                "diff-tree",
                "--stdin",
                "--root",
                "-r",
                "-M",
                "-p",
                "--full-index",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
            ],
            diff_input.as_bytes(),
        )?;
        parse_hunks(&patch, &selected_set)?
    } else {
        Vec::new()
    };

    Ok(Snapshot {
        default_ref,
        tip,
        object_format,
        shallow_boundaries: read_shallow_boundaries(git)?,
        missing_objects,
        commits,
        changes,
        hunks,
    })
}

pub(super) fn read_shallow_boundaries(git: &Git) -> Result<Vec<String>, AppError> {
    if git.text(["rev-parse", "--is-shallow-repository"])?.trim() != "true" {
        return Ok(Vec::new());
    }
    let value = git.text(["rev-parse", "--git-path", "shallow"])?;
    let mut path = PathBuf::from(value.trim());
    if path.is_relative() {
        path = git.root().join(path);
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| parse_error(format!("cannot read shallow boundaries: {error}")))?;
    contents
        .lines()
        .map(|oid| {
            if is_oid(oid.as_bytes()) {
                Ok(oid.to_owned())
            } else {
                Err(parse_error("shallow file contained an invalid object ID"))
            }
        })
        .collect()
}

fn parse_graph(bytes: &[u8]) -> Result<Vec<String>, AppError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| parse_error(format!("revision graph is not UTF-8: {error}")))?;
    text.lines()
        .map(|oid| {
            if is_oid(oid.as_bytes()) {
                Ok(oid.to_owned())
            } else {
                Err(parse_error("revision graph contained an invalid object ID"))
            }
        })
        .collect()
}

fn read_commits(git: &Git, graph: &[String]) -> Result<Vec<Commit>, AppError> {
    if graph.is_empty() {
        return Ok(Vec::new());
    }
    let input = graph
        .iter()
        .map(|oid| format!("{oid}\n"))
        .collect::<String>();
    let output = git.output(["cat-file", "--batch"], input.as_bytes())?;
    let mut cursor = 0;
    let mut commits = Vec::with_capacity(graph.len());
    for expected_oid in graph {
        let header_end = find_byte(&output, cursor, b'\n')
            .ok_or_else(|| parse_error("truncated cat-file header"))?;
        let header = std::str::from_utf8(&output[cursor..header_end])
            .map_err(|error| parse_error(format!("invalid cat-file header: {error}")))?;
        let mut fields = header.split_ascii_whitespace();
        let oid = fields.next().unwrap_or_default();
        let kind = fields.next().unwrap_or_default();
        let size: usize = fields
            .next()
            .ok_or_else(|| parse_error("cat-file header omitted object size"))?
            .parse()
            .map_err(|_| parse_error("cat-file returned an invalid object size"))?;
        if oid != expected_oid || kind != "commit" {
            return Err(parse_error("cat-file returned an unexpected object"));
        }
        let start = header_end + 1;
        let end = start
            .checked_add(size)
            .filter(|end| *end < output.len())
            .ok_or_else(|| parse_error("truncated commit object"))?;
        let body = &output[start..end];
        let (time, message, parents) = parse_commit(body)?;
        commits.push(Commit {
            oid: oid.to_owned(),
            message: message.to_vec(),
            time,
            parents,
        });
        cursor = end + 1;
    }
    Ok(commits)
}

fn parse_commit(body: &[u8]) -> Result<(i64, &[u8], Vec<String>), AppError> {
    let separator = body
        .windows(2)
        .position(|window| window == b"\n\n")
        .ok_or_else(|| parse_error("commit object omitted its message separator"))?;
    let headers = &body[..separator];
    let committer = headers
        .split(|byte| *byte == b'\n')
        .rev()
        .find(|line| line.starts_with(b"committer "))
        .ok_or_else(|| parse_error("commit object omitted its committer"))?;
    let fields: Vec<&[u8]> = committer.rsplitn(3, |byte| *byte == b' ').collect();
    let timestamp = fields
        .get(1)
        .ok_or_else(|| parse_error("committer omitted its timestamp"))?;
    let time = std::str::from_utf8(timestamp)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| parse_error("committer timestamp is invalid"))?;
    let parents = headers
        .split(|byte| *byte == b'\n')
        .filter_map(|line| line.strip_prefix(b"parent "))
        .map(|oid| {
            let oid = std::str::from_utf8(oid)
                .map_err(|error| parse_error(format!("invalid parent object ID: {error}")))?;
            if is_oid(oid.as_bytes()) {
                Ok(oid.to_owned())
            } else {
                Err(parse_error("commit contained an invalid parent object ID"))
            }
        })
        .collect::<Result<_, _>>()?;
    Ok((time, &body[separator + 2..], parents))
}

fn parse_changes(bytes: &[u8], known: &HashSet<&str>) -> Result<Vec<Change>, AppError> {
    let fields: Vec<&[u8]> = bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect();
    let mut index = 0;
    let mut commit_oid = None;
    let mut ordinal = 0;
    let mut changes = Vec::new();
    while index < fields.len() {
        if let Ok(value) = std::str::from_utf8(fields[index])
            && known.contains(value)
        {
            commit_oid = Some(value.to_owned());
            ordinal = 0;
            index += 1;
            continue;
        }
        let oid = commit_oid
            .as_ref()
            .ok_or_else(|| parse_error("diff-tree change preceded its commit header"))?;
        let header = std::str::from_utf8(fields[index])
            .map_err(|error| parse_error(format!("invalid raw diff header: {error}")))?;
        let parts: Vec<&str> = header.split_ascii_whitespace().collect();
        if parts.len() != 5 || !parts[0].starts_with(':') {
            return Err(parse_error("raw diff header has an unexpected shape"));
        }
        let status = parts[4].to_owned();
        index += 1;
        let first_path = fields
            .get(index)
            .ok_or_else(|| parse_error("raw diff omitted a path"))?
            .to_vec();
        index += 1;
        let (old_path, new_path) = match status.as_bytes().first() {
            Some(b'A') => (None, Some(first_path)),
            Some(b'D') => (Some(first_path), None),
            Some(b'R' | b'C') => {
                let second = fields
                    .get(index)
                    .ok_or_else(|| parse_error("rename/copy diff omitted its new path"))?
                    .to_vec();
                index += 1;
                (Some(first_path), Some(second))
            }
            Some(_) => (Some(first_path.clone()), Some(first_path)),
            None => return Err(parse_error("raw diff omitted status")),
        };
        changes.push(Change {
            commit_oid: oid.clone(),
            ordinal,
            status,
            old_path,
            new_path,
            old_blob: nonzero_oid(parts[2]),
            new_blob: nonzero_oid(parts[3]),
            old_mode: parts[0][1..].to_owned(),
            new_mode: parts[1].to_owned(),
        });
        ordinal += 1;
    }
    Ok(changes)
}

fn parse_hunks(bytes: &[u8], known: &HashSet<&str>) -> Result<Vec<Hunk>, AppError> {
    let mut commit_oid: Option<String> = None;
    let mut change_ordinal = -1;
    let mut hunk_ordinal = 0;
    let mut active: Option<Hunk> = None;
    let mut hunks = Vec::new();

    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
        if let Ok(text) = std::str::from_utf8(trimmed)
            && known.contains(text)
        {
            flush_hunk(&mut active, &mut hunks);
            commit_oid = Some(text.to_owned());
            change_ordinal = -1;
            continue;
        }
        if line.starts_with(b"diff --git ") {
            flush_hunk(&mut active, &mut hunks);
            change_ordinal += 1;
            hunk_ordinal = 0;
            continue;
        }
        if line.starts_with(b"@@ ") {
            flush_hunk(&mut active, &mut hunks);
            let (old_start, old_lines, new_start, new_lines) = parse_hunk_header(line)?;
            active = Some(Hunk {
                commit_oid: commit_oid
                    .clone()
                    .ok_or_else(|| parse_error("patch hunk preceded its commit header"))?,
                change_ordinal,
                ordinal: hunk_ordinal,
                old_start,
                old_lines,
                new_start,
                new_lines,
                text: line.to_vec(),
            });
            hunk_ordinal += 1;
        } else if let Some(hunk) = &mut active {
            hunk.text.extend_from_slice(line);
        }
    }
    flush_hunk(&mut active, &mut hunks);
    Ok(hunks)
}

fn parse_hunk_header(line: &[u8]) -> Result<(i64, i64, i64, i64), AppError> {
    let text = std::str::from_utf8(line)
        .map_err(|error| parse_error(format!("invalid text hunk header: {error}")))?;
    let mut fields = text.split_ascii_whitespace();
    if fields.next() != Some("@@") {
        return Err(parse_error("invalid text hunk marker"));
    }
    let old = fields
        .next()
        .ok_or_else(|| parse_error("hunk omitted old range"))?;
    let new = fields
        .next()
        .ok_or_else(|| parse_error("hunk omitted new range"))?;
    let (old_start, old_lines) = parse_range(old, '-')?;
    let (new_start, new_lines) = parse_range(new, '+')?;
    Ok((old_start, old_lines, new_start, new_lines))
}

fn parse_range(value: &str, prefix: char) -> Result<(i64, i64), AppError> {
    let value = value
        .strip_prefix(prefix)
        .ok_or_else(|| parse_error("hunk range has the wrong prefix"))?;
    let mut fields = value.split(',');
    let start = fields
        .next()
        .and_then(|field| field.parse().ok())
        .ok_or_else(|| parse_error("hunk range start is invalid"))?;
    let lines = fields
        .next()
        .map(str::parse)
        .transpose()
        .map_err(|_| parse_error("hunk range length is invalid"))?
        .unwrap_or(1);
    Ok((start, lines))
}

fn flush_hunk(active: &mut Option<Hunk>, hunks: &mut Vec<Hunk>) {
    if let Some(hunk) = active.take() {
        hunks.push(hunk);
    }
}

fn nonzero_oid(value: &str) -> Option<String> {
    (!value.bytes().all(|byte| byte == b'0')).then(|| value.to_owned())
}

fn is_oid(value: &[u8]) -> bool {
    matches!(value.len(), 40 | 64) && value.iter().all(u8::is_ascii_hexdigit)
}

fn find_byte(bytes: &[u8], start: usize, needle: u8) -> Option<usize> {
    bytes[start..]
        .iter()
        .position(|byte| *byte == needle)
        .map(|position| start + position)
}

fn parse_error(message: impl std::fmt::Display) -> AppError {
    AppError::operational(format!("error: parsing Git history: {message}"))
}

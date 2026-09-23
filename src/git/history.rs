use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};

use crate::app::{AppError, IndexStage};

use super::process::Git;

pub(crate) struct Snapshot {
    pub(crate) default_ref: String,
    pub(crate) tip: String,
    pub(crate) object_format: String,
    pub(crate) shallow_boundaries: Vec<String>,
    pub(crate) missing_objects: Vec<String>,
    pub(crate) commits: Vec<Commit>,
    pub(crate) changes: Vec<Change>,
    pub(crate) patches: PatchStream,
}

pub(crate) struct HistoryTarget {
    pub(crate) default_ref: String,
    pub(crate) tip: String,
    pub(crate) object_format: String,
    pub(crate) shallow_boundaries: Vec<String>,
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

pub(crate) struct PatchStream {
    git: Git,
    specs: Vec<String>,
    known: HashSet<String>,
    type_change_ordinals: HashMap<String, HashSet<i64>>,
}

impl PatchStream {
    #[cfg(test)]
    pub(crate) fn empty_for_test() -> Self {
        Self {
            git: Git::new(PathBuf::new()),
            specs: Vec::new(),
            known: HashSet::new(),
            type_change_ordinals: HashMap::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    pub(crate) fn for_each_hunk(
        &self,
        emit: &mut dyn FnMut(Hunk) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        if self.specs.is_empty() {
            return Ok(());
        }
        let mut parser = PatchParser::new(&self.known, &self.type_change_ordinals, emit);
        self.git.stream(
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
            &self.specs,
            |chunk| parser.feed(chunk),
        )?;
        parser.finish()
    }
}
pub(super) fn read(
    git: &Git,
    target: HistoryTarget,
    report: &mut dyn FnMut(IndexStage),
) -> Result<Snapshot, AppError> {
    let graph = read_graph(git, &target.tip)?;
    read_selected(git, target, &graph, &graph, Vec::new(), report)
}

pub(super) fn read_incremental(
    git: &Git,
    target: HistoryTarget,
    cached_commits: &[String],
    refresh_commits: &[String],
    known_missing_objects: Vec<String>,
    report: &mut dyn FnMut(IndexStage),
) -> Result<Snapshot, AppError> {
    let graph = read_graph(git, &target.tip)?;
    let cached = cached_commits.iter().collect::<HashSet<_>>();
    let refresh = refresh_commits.iter().collect::<HashSet<_>>();
    let selected = graph
        .iter()
        .filter(|oid| !cached.contains(oid) || refresh.contains(oid))
        .cloned()
        .collect::<Vec<_>>();
    read_selected(
        git,
        target,
        &graph,
        &selected,
        known_missing_objects,
        report,
    )
}

type TraceFixData = (Commit, Option<Vec<Change>>, Option<Vec<Hunk>>);

pub(super) fn read_trace_fix(git: &Git, fix_oid: &str) -> Result<TraceFixData, AppError> {
    let commits = read_commits(git, &[fix_oid.to_owned()])?;
    let commit = commits
        .into_iter()
        .next()
        .ok_or_else(|| parse_error("fix commit was not returned"))?;
    let input = match commit.parents.first() {
        Some(parent) => format!("{fix_oid} {parent}\n"),
        None => format!("{fix_oid}\n"),
    };
    let known = [fix_oid].into_iter().collect::<HashSet<_>>();
    let changes = match git.output(
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
        input.as_bytes(),
    ) {
        Ok(raw) => Some(parse_changes(&raw, &known)?),
        Err(_) => None,
    };
    let type_change_ordinals = changes
        .as_ref()
        .map(|changes| collect_type_change_ordinals(changes.iter()))
        .unwrap_or_default();
    let hunks = match git.output(
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
        input.as_bytes(),
    ) {
        Ok(patch) => Some(parse_hunks(&patch, &known, &type_change_ordinals)?),
        Err(_) => None,
    };
    Ok((commit, changes, hunks))
}

fn collect_type_change_ordinals<'a>(
    changes: impl IntoIterator<Item = &'a Change>,
) -> HashMap<String, HashSet<i64>> {
    let mut type_change_ordinals = HashMap::<String, HashSet<i64>>::new();
    for change in changes.into_iter().filter(|change| change.status == "T") {
        type_change_ordinals
            .entry(change.commit_oid.clone())
            .or_default()
            .insert(change.ordinal);
    }
    type_change_ordinals
}

fn read_graph(git: &Git, tip: &str) -> Result<Vec<String>, AppError> {
    let graph = git.output(["rev-list", "--reverse", "--topo-order", tip], &[])?;
    parse_graph(&graph)
}

fn read_selected(
    git: &Git,
    target: HistoryTarget,
    graph: &[String],
    selected: &[String],
    known_missing_objects: Vec<String>,
    report: &mut dyn FnMut(IndexStage),
) -> Result<Snapshot, AppError> {
    let commits = read_commits(git, selected)?;
    report(IndexStage::ReadingChanges);
    let graph_set = graph.iter().map(String::as_str).collect::<HashSet<_>>();
    let selected_set = selected.iter().map(String::as_str).collect::<HashSet<_>>();
    let diff_specs = commits
        .iter()
        .filter_map(|commit| match commit.parents.first() {
            Some(parent) if graph_set.contains(parent.as_str()) => {
                Some((commit.oid.clone(), format!("{} {parent}\n", commit.oid)))
            }
            Some(_) => None,
            None => Some((commit.oid.clone(), format!("{}\n", commit.oid))),
        })
        .collect::<Vec<_>>();
    let diff_input = diff_specs
        .iter()
        .map(|(_, input)| input.as_str())
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
    let selected_missing = git.missing_objects(&object_ids)?;
    let mut missing_objects = selected_missing.clone();
    missing_objects.extend(known_missing_objects);
    missing_objects.sort();
    missing_objects.dedup();

    report(IndexStage::ReadingPatches);
    let missing_set = selected_missing
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let blocked_commits = changes
        .iter()
        .filter(|change| {
            [&change.old_blob, &change.new_blob]
                .into_iter()
                .flatten()
                .any(|blob| missing_set.contains(blob.as_str()))
        })
        .map(|change| change.commit_oid.as_str())
        .collect::<HashSet<_>>();
    let (known, specs) = diff_specs
        .into_iter()
        .filter(|(oid, _)| !blocked_commits.contains(oid.as_str()))
        .unzip();
    let type_change_ordinals = collect_type_change_ordinals(
        changes
            .iter()
            .filter(|change| !blocked_commits.contains(change.commit_oid.as_str())),
    );
    let patches = PatchStream {
        git: git.clone(),
        specs,
        known,
        type_change_ordinals,
    };
    Ok(Snapshot {
        default_ref: target.default_ref,
        tip: target.tip,
        object_format: target.object_format,
        shallow_boundaries: target.shallow_boundaries,
        missing_objects,
        commits,
        changes,
        patches,
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
        let old_mode = parts[0][1..].to_owned();
        let new_mode = parts[1].to_owned();
        changes.push(Change {
            commit_oid: oid.clone(),
            ordinal,
            status,
            old_path,
            new_path,
            old_blob: blob_oid(parts[2], &old_mode),
            new_blob: blob_oid(parts[3], &new_mode),
            old_mode,
            new_mode,
        });
        ordinal += 1;
    }
    Ok(changes)
}

fn parse_hunks(
    bytes: &[u8],
    known: &HashSet<&str>,
    type_change_ordinals: &HashMap<String, HashSet<i64>>,
) -> Result<Vec<Hunk>, AppError> {
    let known = known.iter().map(|oid| (*oid).to_owned()).collect();
    let mut hunks = Vec::new();
    let mut emit = |hunk| {
        hunks.push(hunk);
        Ok(())
    };
    let mut parser = PatchParser::new(&known, type_change_ordinals, &mut emit);
    parser.feed(bytes)?;
    parser.finish()?;
    Ok(hunks)
}

struct PatchParser<'a, F>
where
    F: FnMut(Hunk) -> Result<(), AppError>,
{
    known: &'a HashSet<String>,
    type_change_ordinals: &'a HashMap<String, HashSet<i64>>,
    emit: F,
    line: Vec<u8>,
    commit_oid: Option<String>,
    change_ordinal: i64,
    repeat_type_change_block: bool,
    hunk_ordinal: i64,
    active: Option<Hunk>,
}

impl<'a, F> PatchParser<'a, F>
where
    F: FnMut(Hunk) -> Result<(), AppError>,
{
    fn new(
        known: &'a HashSet<String>,
        type_change_ordinals: &'a HashMap<String, HashSet<i64>>,
        emit: F,
    ) -> Self {
        Self {
            known,
            type_change_ordinals,
            emit,
            line: Vec::new(),
            commit_oid: None,
            change_ordinal: -1,
            repeat_type_change_block: false,
            hunk_ordinal: 0,
            active: None,
        }
    }

    fn feed(&mut self, chunk: &[u8]) -> Result<(), AppError> {
        for fragment in chunk.split_inclusive(|byte| *byte == b'\n') {
            self.line.extend_from_slice(fragment);
            if self.line.ends_with(b"\n") {
                self.process_complete_line()?;
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(), AppError> {
        if !self.line.is_empty() {
            self.process_complete_line()?;
        }
        self.flush_hunk()
    }

    fn process_complete_line(&mut self) -> Result<(), AppError> {
        let mut line = std::mem::take(&mut self.line);
        let result = self.process_line(&line);
        line.clear();
        self.line = line;
        result
    }

    fn process_line(&mut self, line: &[u8]) -> Result<(), AppError> {
        let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
        if let Ok(text) = std::str::from_utf8(trimmed)
            && self.known.contains(text)
        {
            self.flush_hunk()?;
            self.commit_oid = Some(text.to_owned());
            self.change_ordinal = -1;
            self.repeat_type_change_block = false;
            return Ok(());
        }
        if line.starts_with(b"diff --git ") {
            self.flush_hunk()?;
            if self.commit_oid.is_none() {
                return Err(parse_error("patch diff preceded its commit header"));
            }
            // Git's patch format splits a raw type change into delete/add blocks.
            if self.repeat_type_change_block {
                self.repeat_type_change_block = false;
            } else {
                self.change_ordinal += 1;
                self.hunk_ordinal = 0;
                self.repeat_type_change_block = self
                    .commit_oid
                    .as_ref()
                    .and_then(|oid| self.type_change_ordinals.get(oid))
                    .is_some_and(|ordinals| ordinals.contains(&self.change_ordinal));
            }
            return Ok(());
        }
        if line.starts_with(b"@@ ") {
            self.flush_hunk()?;
            let commit_oid = self
                .commit_oid
                .clone()
                .ok_or_else(|| parse_error("patch hunk preceded its commit header"))?;
            if self.change_ordinal < 0 {
                return Err(parse_error("patch hunk preceded its diff boundary"));
            }
            let (old_start, old_lines, new_start, new_lines) = parse_hunk_header(line)?;
            self.active = Some(Hunk {
                commit_oid,
                change_ordinal: self.change_ordinal,
                ordinal: self.hunk_ordinal,
                old_start,
                old_lines,
                new_start,
                new_lines,
                text: line.to_vec(),
            });
            self.hunk_ordinal += 1;
        } else if let Some(hunk) = &mut self.active {
            hunk.text.extend_from_slice(line);
        }
        Ok(())
    }

    fn flush_hunk(&mut self) -> Result<(), AppError> {
        if let Some(hunk) = self.active.take() {
            (self.emit)(hunk)?;
        }
        Ok(())
    }
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

fn blob_oid(value: &str, mode: &str) -> Option<String> {
    if mode == "160000" {
        None
    } else {
        nonzero_oid(value)
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

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        fs,
        process::Command,
    };

    use super::{Git, Hunk, PatchParser, PatchStream};
    use crate::app::AppError;

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    fn parse_fragmented(bytes: &[u8], chunk_size: usize) -> Result<Vec<Hunk>, String> {
        let known = HashSet::from([OID.to_owned()]);
        let mut hunks = Vec::new();
        let mut emit = |hunk| {
            hunks.push(hunk);
            Ok(())
        };
        let type_change_ordinals = HashMap::new();
        let mut parser = PatchParser::new(&known, &type_change_ordinals, &mut emit);
        for chunk in bytes.chunks(chunk_size) {
            parser.feed(chunk).map_err(|error| error.to_string())?;
        }
        parser.finish().map_err(|error| error.to_string())?;
        Ok(hunks)
    }

    #[test]
    fn patch_parser_handles_every_token_split_across_chunks() {
        let patch = format!(
            "{OID}\ndiff --git a/old b/new\nindex 111..222 100644\n--- a/old\n+++ b/new\n@@ -1,2 +1,2 @@ section\n-old\n+new\n context\n@@ -8 +8,0 @@\n-gone"
        );
        let hunks = parse_fragmented(patch.as_bytes(), 1).unwrap();

        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].commit_oid, OID);
        assert_eq!(hunks[0].change_ordinal, 0);
        assert_eq!(hunks[0].ordinal, 0);
        assert_eq!((hunks[0].old_start, hunks[0].old_lines), (1, 2));
        assert_eq!((hunks[0].new_start, hunks[0].new_lines), (1, 2));
        assert_eq!(
            hunks[0].text,
            b"@@ -1,2 +1,2 @@ section\n-old\n+new\n context\n"
        );
        assert_eq!(hunks[1].ordinal, 1);
        assert_eq!(hunks[1].text, b"@@ -8 +8,0 @@\n-gone");
    }

    #[test]
    fn patch_parser_keeps_a_large_line_without_buffering_the_patch() {
        let mut patch = format!("{OID}\ndiff --git a/a b/a\n@@ -1 +1 @@\n-removed\n+").into_bytes();
        patch.extend(std::iter::repeat_n(b'x', 1024 * 1024));
        patch.push(b'\n');
        let hunks = parse_fragmented(&patch, 4093).unwrap();

        assert_eq!(hunks.len(), 1);
        assert_eq!(
            hunks[0].text.len(),
            "@@ -1 +1 @@\n-removed\n+\n".len() + 1024 * 1024
        );
        assert!(hunks[0].text.ends_with(b"xxx\n"));
    }

    #[test]
    fn patch_parser_accepts_hunk_free_input() {
        let patch = format!(
            "{OID}\ndiff --git a/empty b/empty\nsimilarity index 100%\nrename from empty\nrename to empty\n"
        );
        assert!(parse_fragmented(patch.as_bytes(), 3).unwrap().is_empty());
    }

    #[test]
    fn patch_parser_rejects_malformed_ordering() {
        let without_commit = b"diff --git a/a b/a\n@@ -1 +1 @@\n-a\n+b\n";
        let error = parse_fragmented(without_commit, 2).err().unwrap();
        assert!(error.contains("patch diff preceded its commit header"));

        let without_diff = format!("{OID}\n@@ -1 +1 @@\n-a\n+b\n");
        let error = parse_fragmented(without_diff.as_bytes(), 5).err().unwrap();
        assert!(error.contains("patch hunk preceded its diff boundary"));
    }

    #[test]
    fn patch_stream_propagates_cache_write_failure() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        run_git(root, &["init", "-q"]);
        run_git(root, &["config", "user.name", "Test"]);
        run_git(root, &["config", "user.email", "test@example.com"]);
        let mut lines = (0..30)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>();
        fs::write(root.join("file.txt"), lines.join("\n")).unwrap();
        run_git(root, &["add", "file.txt"]);
        run_git(root, &["commit", "-qm", "initial"]);
        lines[0] = "changed first".to_owned();
        lines[29] = "changed last".to_owned();
        fs::write(root.join("file.txt"), lines.join("\n")).unwrap();
        run_git(root, &["add", "file.txt"]);
        run_git(root, &["commit", "-qm", "change"]);
        let oid = git_output(root, &["rev-parse", "HEAD"]);
        let patches = PatchStream {
            git: Git::new(root.to_owned()),
            specs: vec![format!("{oid}\n")],
            known: HashSet::from([oid]),
            type_change_ordinals: HashMap::new(),
        };

        let error = patches
            .for_each_hunk(&mut |_| Err(AppError::operational("simulated SQLite write failure")))
            .unwrap_err();

        assert_eq!(error.to_string(), "simulated SQLite write failure");
    }

    fn run_git(root: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn git_output(root: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }
}

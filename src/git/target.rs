use std::{collections::HashSet, path::Path};

use crate::app::AppError;

use super::{history, process::Git};

#[derive(Clone)]
pub(crate) enum WhyAnchor {
    Line { number: usize },
    Symbol { name: String, number: usize },
}

pub(crate) struct WhyTarget {
    pub(crate) revision: String,
    pub(crate) path: Vec<u8>,
    pub(crate) anchor: WhyAnchor,
    pub(crate) blame: Option<Blame>,
    pub(crate) reachable: HashSet<String>,
    pub(crate) warnings: Vec<String>,
    pub(crate) anchor_valid: bool,
}

pub(crate) struct Blame {
    pub(crate) oid: String,
    pub(crate) subject: String,
    pub(crate) boundary: bool,
}

pub(crate) struct TraceFixTarget {
    pub(crate) revision: String,
    pub(crate) fix: history::Commit,
    pub(crate) parent: Option<String>,
    pub(crate) shallow: bool,
    pub(crate) deleted_lines: Vec<DeletedLine>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) struct DeletedLine {
    pub(crate) path: Vec<u8>,
    pub(crate) line: usize,
    pub(crate) blame: Option<Blame>,
}

pub(super) fn pin_trace_fix(
    git: &Git,
    requested_revision: &str,
    paths: &[String],
) -> Result<TraceFixTarget, AppError> {
    for path in paths {
        validate_path(path)?;
    }
    if requested_revision.contains('\0') {
        return Err(AppError::input("revision must not contain a NUL byte"));
    }
    let revision = resolve_revision(git, requested_revision)?;
    let (fix, all_changes, hunks) = history::read_trace_fix(git, &revision)?;
    let parent = fix.parents.first().cloned();
    let shallow = !read_shallow_boundaries(git)?.is_empty();
    let mut warnings = Vec::new();
    let parent_missing = parent
        .as_ref()
        .map(|parent| git.missing_objects(std::slice::from_ref(parent)))
        .transpose()?
        .is_some_and(|missing| !missing.is_empty());
    if all_changes.is_none() {
        warnings.push(
            "warning: fix changed-path metadata is unavailable locally; lineage is degraded (confidence: low)."
                .to_owned(),
        );
    }
    if shallow {
        warnings.push(
            "warning: local history is shallow; trace-fix lineage may be incomplete.".to_owned(),
        );
    }
    if parent_missing {
        warnings.push(
            "warning: fix parent is missing locally; deleted-line lineage is unavailable (confidence: low)."
                .to_owned(),
        );
    }
    if fix.parents.len() > 1 {
        warnings.push(
            "warning: fix is a merge; first-parent deleted lines are used and merge lineage is ambiguous."
                .to_owned(),
        );
    }
    if parent.is_none() {
        warnings.push(
            "warning: fix has no parent; introducing lineage is unavailable (confidence: low)."
                .to_owned(),
        );
    }
    let selected_ordinals = match all_changes.as_ref() {
        None => HashSet::new(),
        Some(all_changes) if paths.is_empty() => all_changes
            .iter()
            .map(|change| change.ordinal)
            .collect::<HashSet<_>>(),
        Some(all_changes) => {
            let mut selected = HashSet::new();
            for path in paths {
                let normalized = normalize_path(path.as_bytes());
                let matches = all_changes.iter().filter(|change| {
                    change.old_path.as_deref().map(normalize_path).as_deref()
                        == Some(normalized.as_slice())
                        || change.new_path.as_deref().map(normalize_path).as_deref()
                            == Some(normalized.as_slice())
                });
                let mut found = false;
                for change in matches {
                    found = true;
                    selected.insert(change.ordinal);
                }
                if !found {
                    return Err(AppError::input(format!(
                        "path was not changed by fix revision: {path}"
                    )));
                }
            }
            selected
        }
    };
    let changes = all_changes
        .unwrap_or_default()
        .into_iter()
        .filter(|change| selected_ordinals.contains(&change.ordinal))
        .collect::<Vec<_>>();
    let object_ids = changes
        .iter()
        .flat_map(|change| [&change.old_blob, &change.new_blob].into_iter().flatten())
        .cloned()
        .collect::<Vec<_>>();
    if !git.missing_objects(&object_ids)?.is_empty() {
        warnings.push(
            "warning: fix diff blobs are missing locally; deleted-line lineage may be incomplete (confidence: low)."
                .to_owned(),
        );
    }
    let hunks_available = hunks.is_some();
    if !hunks_available {
        warnings.push(
            "warning: fix diff hunks are unavailable locally; deleted-line lineage may be incomplete (confidence: low)."
                .to_owned(),
        );
    }
    let mut deleted_lines = Vec::new();
    if let (Some(parent), Some(hunks)) = (parent.as_deref(), hunks.as_ref()) {
        let ignore_file = blame_ignore_file(git)?;
        for hunk in hunks {
            if !selected_ordinals.contains(&hunk.change_ordinal) {
                continue;
            }
            let Some(change) = changes
                .iter()
                .find(|change| change.ordinal == hunk.change_ordinal)
            else {
                continue;
            };
            let Some(path) = change.old_path.as_deref() else {
                continue;
            };
            let path_text = String::from_utf8_lossy(path).into_owned();
            let mut old_line = hunk.old_start;
            for diff_line in hunk.text.split_inclusive(|byte| *byte == b'\n') {
                let Some(marker) = diff_line.first().copied() else {
                    continue;
                };
                match marker {
                    b'-' => {
                        if let Ok(line) = usize::try_from(old_line) {
                            let (blame, used_ignore_file) =
                                read_blame(git, parent, &path_text, line, ignore_file.as_deref())?;
                            if used_ignore_file {
                                push_warning(
                                    &mut warnings,
                                    "warning: blame ignored revisions from .git-blame-ignore-revs; trace-fix attribution may be incomplete."
                                        .to_owned(),
                                );
                            }
                            if let Some(blame) = &blame {
                                if shallow && blame.boundary {
                                    push_warning(
                                        &mut warnings,
                                        "warning: fix-parent blame reached a shallow boundary; introducing lineage may be incomplete (confidence: low)."
                                            .to_owned(),
                                    );
                                }
                            } else {
                                push_warning(
                                    &mut warnings,
                                    "warning: fix-parent deleted-line blame is unavailable; introducing lineage may be incomplete (confidence: low)."
                                        .to_owned(),
                                );
                            }
                            deleted_lines.push(DeletedLine {
                                path: path.to_vec(),
                                line,
                                blame,
                            });
                        }
                        old_line += 1;
                    }
                    b' ' => old_line += 1,
                    b'+' | b'\\' | b'@' => {}
                    _ => {}
                }
            }
        }
    }
    if hunks_available && deleted_lines.is_empty() {
        warnings.push(
            "warning: fix has no deleted lines; introducing lineage is unavailable (confidence: low)."
                .to_owned(),
        );
    }
    Ok(TraceFixTarget {
        revision,
        parent,
        shallow,
        fix,
        deleted_lines,
        warnings,
    })
}

fn push_warning(warnings: &mut Vec<String>, warning: String) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

pub(super) fn pin(
    git: &Git,
    revision: Option<&str>,
    path: &str,
    anchor: WhyAnchor,
) -> Result<WhyTarget, AppError> {
    validate_path(path)?;
    let requested_revision = revision.unwrap_or("HEAD");
    if requested_revision.contains('\0') {
        return Err(AppError::input("revision must not contain a NUL byte"));
    }
    let revision = resolve_revision(git, requested_revision)?;
    let reachable = read_reachable(git, &revision)?;
    let shallow = !read_shallow_boundaries(git)?.is_empty();
    let mut warnings = Vec::new();
    if shallow {
        warnings
            .push("warning: local history is shallow; why material may be incomplete.".to_owned());
    }

    let entry = read_tree_entry(git, &revision, path)?;
    if entry.kind == "commit" {
        warnings.push(
            "warning: target path is a submodule; history stops at the submodule boundary."
                .to_owned(),
        );
        return Ok(WhyTarget {
            revision,
            path: path.as_bytes().to_vec(),
            anchor,
            blame: None,
            reachable,
            anchor_valid: false,
            warnings,
        });
    }
    if entry.kind != "blob" {
        return Err(AppError::input(format!(
            "path is not a file at revision: {path}"
        )));
    }

    let content = git
        .output(["cat-file", "blob", &format!("{}:{path}", revision)], &[])
        .map_err(|error| {
            AppError::operational(format!(
                "error: reading target path at {requested_revision}: {error}"
            ))
        })?;
    let (anchor, number) = resolve_anchor(&anchor, &content, path)?;
    let ignore_file = blame_ignore_file(git)?;
    let (blame, used_ignore_file) =
        read_blame(git, &revision, path, number, ignore_file.as_deref())?;
    if blame.is_none() {
        warnings.push(
            "warning: Git line attribution unavailable; explanation ranking uses cached history."
                .to_owned(),
        );
    }
    if used_ignore_file {
        warnings.push(
            "warning: blame ignored revisions from .git-blame-ignore-revs; attribution may be incomplete."
                .to_owned(),
        );
    }
    let blame = blame.inspect(|blame| {
        if shallow && blame.boundary {
            warnings.push(
                "warning: blame reached a local history boundary; the original explanation may be incomplete."
                    .to_owned(),
            );
        }
    });

    Ok(WhyTarget {
        revision,
        path: path.as_bytes().to_vec(),
        anchor,
        blame,
        reachable,
        anchor_valid: true,
        warnings,
    })
}

fn normalize_path(path: &[u8]) -> Vec<u8> {
    let mut normalized = Vec::new();
    for component in path.split(|byte| *byte == b'/' || *byte == b'\\') {
        if component.is_empty() || component == b"." {
            continue;
        }
        if !normalized.is_empty() {
            normalized.push(b'/');
        }
        normalized.extend_from_slice(component);
    }
    normalized
}

fn validate_path(path: &str) -> Result<(), AppError> {
    let value = Path::new(path);
    if path.is_empty() {
        return Err(AppError::input("path must not be empty"));
    }
    if path.contains('\0') {
        return Err(AppError::input("path must not contain a NUL byte"));
    }
    if value.is_absolute()
        || value.has_root()
        || value
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(AppError::input(format!(
            "path must be repository-relative: {path}"
        )));
    }
    Ok(())
}

fn resolve_revision(git: &Git, requested: &str) -> Result<String, AppError> {
    let spec = format!("{requested}^{{commit}}");
    let result = git.text([
        "rev-parse",
        "--verify",
        "--quiet",
        "--end-of-options",
        &spec,
    ]);
    match result {
        Ok(value) => Ok(value.trim().to_owned()),
        Err(_) => Err(AppError::input(format!("invalid revision: {requested}"))),
    }
}

fn read_reachable(git: &Git, revision: &str) -> Result<HashSet<String>, AppError> {
    let output = git.text(["rev-list", "--topo-order", revision])?;
    output
        .lines()
        .map(|oid| {
            if is_oid(oid.as_bytes()) {
                Ok(oid.to_owned())
            } else {
                Err(AppError::operational(
                    "error: Git revision graph contained an invalid object ID",
                ))
            }
        })
        .collect()
}

struct TreeEntry {
    kind: String,
}

fn read_tree_entry(git: &Git, revision: &str, path: &str) -> Result<TreeEntry, AppError> {
    let output = git.output(
        [
            "--literal-pathspecs",
            "ls-tree",
            "-z",
            "--full-tree",
            revision,
            "--",
            path,
        ],
        &[],
    )?;
    for entry in output
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
            return Err(AppError::operational(
                "error: Git tree entry had an unexpected shape",
            ));
        };
        let (header, entry_path) = entry.split_at(tab);
        let entry_path = &entry_path[1..];
        if entry_path == path.as_bytes() {
            let mut fields = header.split(|byte| *byte == b' ');
            let _mode = fields.next();
            let kind = fields
                .next()
                .and_then(|value| std::str::from_utf8(value).ok())
                .ok_or_else(|| {
                    AppError::operational("error: Git tree entry omitted its object type")
                })?;
            return Ok(TreeEntry {
                kind: kind.to_owned(),
            });
        }
    }
    Err(AppError::input(format!(
        "path does not exist at revision: {path}"
    )))
}

fn resolve_anchor(
    anchor: &WhyAnchor,
    content: &[u8],
    path: &str,
) -> Result<(WhyAnchor, usize), AppError> {
    match anchor {
        WhyAnchor::Line { number } => {
            let line_count = if content.is_empty() {
                0
            } else {
                let count = content.split(|byte| *byte == b'\n').count();
                if content.ends_with(b"\n") {
                    count.saturating_sub(1)
                } else {
                    count
                }
            };
            if *number == 0 || *number > line_count {
                return Err(AppError::input(format!(
                    "line {number} is outside {path} at the target revision"
                )));
            }
            Ok((anchor.clone(), *number))
        }
        WhyAnchor::Symbol { name, .. } => {
            if name.trim().is_empty() {
                return Err(AppError::input("symbol must not be empty"));
            }
            let lines = content
                .split(|byte| *byte == b'\n')
                .enumerate()
                .filter_map(|(index, line)| {
                    contains_symbol(line, name.as_bytes()).then_some(index + 1)
                })
                .collect::<Vec<_>>();
            let number = lines
                .iter()
                .copied()
                .find(|line| {
                    let bytes = content
                        .split(|byte| *byte == b'\n')
                        .nth(line.saturating_sub(1))
                        .unwrap_or_default();
                    is_declaration(bytes, name.as_bytes())
                })
                .or_else(|| lines.first().copied())
                .ok_or_else(|| {
                    AppError::input(format!(
                        "symbol {name} was not found in {path} at the target revision"
                    ))
                })?;
            Ok((
                WhyAnchor::Symbol {
                    name: name.clone(),
                    number,
                },
                number,
            ))
        }
    }
}

fn contains_symbol(line: &[u8], symbol: &[u8]) -> bool {
    if symbol.is_empty() {
        return false;
    }
    line.windows(symbol.len())
        .enumerate()
        .any(|(index, window)| {
            window == symbol
                && (index == 0 || !is_symbol_byte(line[index - 1]))
                && (index + symbol.len() == line.len()
                    || !is_symbol_byte(line[index + symbol.len()]))
        })
}

fn is_declaration(line: &[u8], symbol: &[u8]) -> bool {
    let trimmed = line
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .collect::<Vec<_>>();
    if trimmed.starts_with(b"//")
        || trimmed.starts_with(b"#")
        || trimmed.starts_with(b"/*")
        || trimmed.starts_with(b"*")
        || trimmed.starts_with(b"\"")
        || trimmed.starts_with(b"'")
    {
        return false;
    }
    let Some(index) = line
        .windows(symbol.len())
        .position(|window| window == symbol)
    else {
        return false;
    };
    if index > 0 && is_symbol_byte(line[index - 1])
        || index + symbol.len() < line.len() && is_symbol_byte(line[index + symbol.len()])
    {
        return false;
    }
    let before = &line[..index];
    let after = line[index + symbol.len()..]
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .collect::<Vec<_>>();
    let shape = after.starts_with(b"(")
        || after.starts_with(b"{")
        || after.starts_with(b":")
        || after.starts_with(b"=");
    let tokens = before
        .split(|byte| !is_symbol_byte(*byte))
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let keyword = tokens.last().is_some_and(|token| {
        matches!(
            *token,
            b"fn"
                | b"func"
                | b"function"
                | b"def"
                | b"class"
                | b"struct"
                | b"enum"
                | b"trait"
                | b"interface"
                | b"type"
                | b"const"
                | b"let"
                | b"var"
                | b"module"
                | b"namespace"
                | b"macro"
                | b"impl"
        )
    });
    keyword && shape
}

fn is_symbol_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn blame_ignore_file(git: &Git) -> Result<Option<String>, AppError> {
    let configured = git
        .output(["config", "--get", "blame.ignoreRevsFile"], &[])
        .ok()
        .and_then(|output| {
            let value = String::from_utf8_lossy(&output).trim().to_owned();
            (!value.is_empty()).then_some(value)
        });
    let candidate = configured.unwrap_or_else(|| ".git-blame-ignore-revs".to_owned());
    if candidate == "none" {
        return Ok(None);
    }
    let path = Path::new(&candidate);
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        git.root().join(path)
    };
    if path.is_file()
        && path
            .metadata()
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false)
    {
        Ok(Some(candidate))
    } else {
        Ok(None)
    }
}
fn read_blame(
    git: &Git,
    revision: &str,
    path: &str,
    line: usize,
    ignore_file: Option<&str>,
) -> Result<(Option<Blame>, bool), AppError> {
    let mut args = vec![
        "-c".to_owned(),
        "core.fsmonitor=false".to_owned(),
        "--literal-pathspecs".to_owned(),
        "blame".to_owned(),
        "--no-textconv".to_owned(),
        "--line-porcelain".to_owned(),
    ];
    if let Some(ignore_file) = ignore_file {
        args.push(format!("--ignore-revs-file={ignore_file}"));
    }
    args.extend([
        "-L".to_owned(),
        format!("{line},{line}"),
        revision.to_owned(),
        "--".to_owned(),
        path.to_owned(),
    ]);
    let output = match git.output(args.iter().map(String::as_str), &[]) {
        Ok(output) => output,
        Err(_error) if ignore_file.is_some() => {
            let fallback = [
                "-c",
                "core.fsmonitor=false",
                "--literal-pathspecs",
                "blame",
                "--no-textconv",
                "--line-porcelain",
                "-L",
                &format!("{line},{line}"),
                revision,
                "--",
                path,
            ];
            match git.output(fallback, &[]) {
                Ok(output) => return Ok((parse_blame(&output)?, false)),
                Err(_) => return Ok((None, false)),
            }
        }
        Err(_) => return Ok((None, false)),
    };
    Ok((parse_blame(&output)?, ignore_file.is_some()))
}

fn parse_blame(output: &[u8]) -> Result<Option<Blame>, AppError> {
    let Some(header) = output.split(|byte| *byte == b'\n').next() else {
        return Ok(None);
    };
    let fields = std::str::from_utf8(header)
        .map_err(|_| AppError::operational("error: parsing Git blame: header was not UTF-8"))?
        .split_ascii_whitespace()
        .collect::<Vec<_>>();
    if fields.len() < 3 {
        return Err(AppError::operational(
            "error: parsing Git blame: header had an unexpected shape",
        ));
    }
    let boundary = fields[0].starts_with('^')
        || output
            .split(|byte| *byte == b'\n')
            .skip(1)
            .any(|line| line == b"boundary");
    let oid = fields[0].trim_start_matches('^');
    if !is_oid(oid.as_bytes()) {
        return Err(AppError::operational(
            "error: parsing Git blame: invalid object ID",
        ));
    }
    fields[1]
        .parse::<usize>()
        .map_err(|_| AppError::operational("error: parsing Git blame: invalid line number"))?;
    let subject = output
        .split(|byte| *byte == b'\n')
        .find_map(|line| line.strip_prefix(b"summary "))
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .unwrap_or_default();
    Ok(Some(Blame {
        oid: oid.to_owned(),
        subject,
        boundary,
    }))
}

fn read_shallow_boundaries(git: &Git) -> Result<Vec<String>, AppError> {
    if git.text(["rev-parse", "--is-shallow-repository"])?.trim() != "true" {
        return Ok(Vec::new());
    }
    let path = git.text(["rev-parse", "--git-path", "shallow"])?;
    let path = Path::new(path.trim());
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        git.root().join(path)
    };
    let contents = std::fs::read_to_string(path).map_err(|error| {
        AppError::operational(format!("error: reading shallow boundaries: {error}"))
    })?;
    Ok(contents.lines().map(str::to_owned).collect())
}

fn is_oid(value: &[u8]) -> bool {
    matches!(value.len(), 40 | 64) && value.iter().all(u8::is_ascii_hexdigit)
}

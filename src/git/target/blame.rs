//! Git blame reading, ignore-revs handling and porcelain parsing.

use super::is_oid;
use crate::app::AppError;
use crate::git::process::Git;
use std::{collections::HashMap, path::Path};
pub(crate) struct Blame {
    pub(crate) oid: String,
    pub(crate) subject: String,
    pub(crate) original_line: usize,
    pub(crate) boundary: bool,
}

pub(super) fn blame_ignore_file(git: &Git) -> Result<Option<String>, AppError> {
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
pub(super) fn read_blame(
    git: &Git,
    revision: &str,
    path: &str,
    line: usize,
    ignore_file: Option<&str>,
) -> Result<(Option<Blame>, bool), AppError> {
    let (mut blamed, used_ignore_file) =
        read_blame_range(git, revision, path, line, line, ignore_file)?;
    Ok((blamed.remove(&line), used_ignore_file))
}

pub(super) fn read_blame_range(
    git: &Git,
    revision: &str,
    path: &str,
    start: usize,
    end: usize,
    ignore_file: Option<&str>,
) -> Result<(HashMap<usize, Blame>, bool), AppError> {
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
        format!("{start},{end}"),
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
                &format!("{start},{end}"),
                revision,
                "--",
                path,
            ];
            match git.output(fallback, &[]) {
                Ok(output) => return Ok((parse_blame_range(&output)?, false)),
                Err(_) => return Ok((HashMap::new(), false)),
            }
        }
        Err(_) => return Ok((HashMap::new(), false)),
    };
    Ok((parse_blame_range(&output)?, ignore_file.is_some()))
}

fn parse_blame_range(output: &[u8]) -> Result<HashMap<usize, Blame>, AppError> {
    let mut blamed = HashMap::new();
    let mut start = 0;
    let mut end = 0;
    for line in output.split_inclusive(|byte| *byte == b'\n') {
        end += line.len();
        if !line.starts_with(b"\t") {
            continue;
        }
        let record = &output[start..end];
        if let Some(blame) = parse_blame(record)? {
            let final_line = record
                .split(|byte| *byte == b'\n')
                .next()
                .and_then(|header| std::str::from_utf8(header).ok())
                .and_then(|header| header.split_ascii_whitespace().nth(2))
                .and_then(|number| number.parse::<usize>().ok())
                .ok_or_else(|| {
                    AppError::operational("error: parsing Git blame: invalid final line number")
                })?;
            blamed.insert(final_line, blame);
        }
        start = end;
    }
    if start != output.len() {
        return Err(AppError::operational(
            "error: parsing Git blame: incomplete line record",
        ));
    }
    Ok(blamed)
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
    let original_line = fields[1]
        .parse::<usize>()
        .map_err(|_| AppError::operational("error: parsing Git blame: invalid line number"))?;
    let subject = output
        .split(|byte| *byte == b'\n')
        .find_map(|line| line.strip_prefix(b"summary "))
        .map(|line| String::from_utf8_lossy(line).into_owned())
        .unwrap_or_default();
    Ok(Some(Blame {
        oid: oid.to_owned(),
        original_line,
        subject,
        boundary,
    }))
}

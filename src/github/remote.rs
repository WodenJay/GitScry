use std::process::Command;

use crate::git::Repository;

pub(super) fn resolve(explicit: Option<&str>) -> Result<String, &'static str> {
    if let Some(explicit) = explicit {
        return parse_repository(explicit).ok_or("Invalid GitHub repository; expected OWNER/REPO.");
    }

    let repository = Repository::discover().map_err(
        |_| "Could not determine the GitHub repository; specify --github-repo OWNER/REPO.",
    )?;
    let output = Command::new("git")
        .arg("-C")
        .arg(repository.root)
        .args(["remote", "-v"])
        .output()
        .map_err(|_| "Could not read Git remotes; specify --github-repo OWNER/REPO.")?;
    if !output.status.success() {
        return Err("Could not read Git remotes; specify --github-repo OWNER/REPO.");
    }

    let remotes = String::from_utf8_lossy(&output.stdout);
    unique_repository(
        remotes
            .lines()
            .filter_map(|line| line.split_whitespace().nth(1))
            .map(str::to_owned),
    )
}

fn unique_repository(urls: impl IntoIterator<Item = String>) -> Result<String, &'static str> {
    let mut repositories = Vec::new();
    for url in urls {
        let Some(candidate) = github_repository(&url) else {
            continue;
        };
        if !repositories
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&candidate))
        {
            repositories.push(candidate);
        }
    }

    match repositories.len() {
        0 => Err("No github.com remote found; specify --github-repo OWNER/REPO."),
        1 => Ok(repositories.remove(0)),
        _ => Err("Multiple GitHub repositories found; specify --github-repo OWNER/REPO."),
    }
}

fn github_repository(url: &str) -> Option<String> {
    let path = if let Some((scheme, remainder)) = url.split_once("://") {
        if !matches!(scheme, "https" | "http" | "ssh" | "git") {
            return None;
        }
        let (authority, path) = remainder.split_once('/')?;
        let host = authority.rsplit('@').next()?.split(':').next()?;
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        path
    } else {
        let (authority, path) = url.split_once(':')?;
        let host = authority.rsplit('@').next()?;
        if !host.eq_ignore_ascii_case("github.com") {
            return None;
        }
        path
    };

    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    parse_repository(path)
}

fn parse_repository(value: &str) -> Option<String> {
    let mut parts = value.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    if parts.next().is_some() || !valid_segment(owner) || !valid_segment(name) {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::{github_repository, parse_repository, unique_repository};

    #[test]
    fn accepts_supported_github_remote_forms() {
        for remote in [
            "https://github.com/acme/widget.git",
            "ssh://git@github.com/acme/widget",
            "git@github.com:acme/widget.git",
        ] {
            assert_eq!(github_repository(remote).as_deref(), Some("acme/widget"));
        }
    }

    #[test]
    fn duplicate_remotes_are_one_repository_but_distinct_repositories_are_ambiguous() {
        let duplicate = unique_repository([
            "https://github.com/Acme/widget.git".to_owned(),
            "git@github.com:acme/WIDGET.git".to_owned(),
        ])
        .unwrap();
        assert_eq!(duplicate, "Acme/widget");

        let ambiguous = unique_repository([
            "https://github.com/acme/one.git".to_owned(),
            "https://github.com/acme/two.git".to_owned(),
        ]);
        assert!(
            ambiguous
                .unwrap_err()
                .contains("Multiple GitHub repositories")
        );
    }

    #[test]
    fn ignores_other_hosts_and_malformed_repository_paths() {
        assert_eq!(github_repository("https://gitlab.com/acme/widget"), None);
        assert_eq!(github_repository("https://github.com/acme"), None);
        assert_eq!(parse_repository("acme/widget/extra"), None);
        assert_eq!(parse_repository("../widget"), None);
    }
}

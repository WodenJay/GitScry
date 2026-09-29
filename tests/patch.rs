mod support;

use std::{fs, path::Path};

#[cfg(target_os = "linux")]
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;
use support::{TestRepo, git, git_command};

fn commit(repo: &TestRepo, files: &[(&str, &[u8])], subject: &str, day: u8) {
    for (path, contents) in files {
        let path = Path::new(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(repo.dir.path().join(parent)).expect("create parent directory");
        }
        fs::write(repo.dir.path().join(path), contents).expect("write tracked file");
    }
    git(repo.dir.path(), ["add", "--all"]);
    let date = format!("2000-01-{day:02}T00:00:00+0000");
    let output = git_command(repo.dir.path())
        .args(["commit", "--quiet", "-m", subject])
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .expect("run git commit");
    assert!(
        output.status.success(),
        "git commit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn json(repo: &TestRepo, args: &[&str]) -> Value {
    let output = repo.run(args);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid JSON")
}

fn engine(delay: &str, enabled: bool) -> Vec<u8> {
    let mut source = format!("fn run() {{\n    let delay = {delay};\n");
    for index in 0..16 {
        source.push_str(&format!("    let filler_{index} = {index};\n"));
    }
    source.push_str(&format!("    let enabled = {enabled};\n}}\n"));
    source.into_bytes()
}

#[test]
fn patch_excerpts_are_opt_in_relevant_and_path_scoped() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[
            ("src/engine.rs", &engine("1", false)),
            ("docs/guide.md", b"A stable guide.\n"),
        ],
        "Create core files",
        1,
    );
    commit(
        &repo,
        &[
            ("src/engine.rs", &engine("retry_delay", true)),
            ("docs/guide.md", b"An updated stable guide.\n"),
        ],
        "Improve retry backoff handling",
        2,
    );
    repo.index();

    let plain = json(&repo, &["search", "retry", "--json"]);
    assert_eq!(plain["schema_version"], 1);
    assert!(plain["materials"][0].get("patch").is_none());

    let search = json(&repo, &["search", "retry", "--patch", "--json"]);
    assert_eq!(search["schema_version"], 2);
    assert_eq!(search["matched_count"], plain["matched_count"]);
    let material = &search["materials"][0];
    assert_eq!(material["subject"], "Improve retry backoff handling");
    assert_eq!(material["patch"]["status"], "available");
    assert_eq!(
        material["patch"]["commit_oid"],
        material["citations"][0]["oid"]
    );
    let hunks = material["patch"]["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 1);
    assert_eq!(hunks[0]["new_path"], "src/engine.rs");
    let excerpt = hunks[0]["text"].as_str().unwrap();
    assert!(excerpt.contains("retry_delay"));
    assert!(!excerpt.contains("enabled"));
    assert!(!excerpt.contains("docs/guide.md"));

    let examples = json(
        &repo,
        &[
            "examples",
            "retry",
            "--path",
            "src/engine.rs",
            "--patch",
            "--json",
        ],
    );
    assert_eq!(examples["schema_version"], 2);
    assert_eq!(examples["materials"][0]["patch"]["status"], "available");
    let example_hunks = examples["materials"][0]["patch"]["hunks"]
        .as_array()
        .unwrap();
    assert_eq!(example_hunks.len(), 1);
    assert_eq!(example_hunks[0]["new_path"], "src/engine.rs");
    let explicit_path = json(
        &repo,
        &[
            "examples",
            "retry",
            "docs/guide.md",
            "--path",
            "src/engine.rs",
            "--patch",
            "--json",
        ],
    );
    let explicit_hunks = explicit_path["materials"][0]["patch"]["hunks"]
        .as_array()
        .unwrap();
    assert!(
        explicit_hunks
            .iter()
            .all(|hunk| hunk["new_path"] == "src/engine.rs")
    );
    let example_text = repo.run(["examples", "retry", "--path", "src/engine.rs", "--patch"]);
    assert_eq!(example_text.status.code(), Some(0));
    let example_text = String::from_utf8(example_text.stdout).expect("patch text must be UTF-8");
    assert!(example_text.contains("retry_delay"));
    assert!(!example_text.contains("enabled"));
    let plain_example_text = repo.run(["examples", "retry", "--path", "src/engine.rs"]);
    assert!(!String::from_utf8_lossy(&plain_example_text.stdout).contains("patch excerpt:"));

    let plain_examples = json(
        &repo,
        &["examples", "retry", "--path", "src/engine.rs", "--json"],
    );
    assert_eq!(plain_examples["schema_version"], 1);
    assert!(plain_examples["materials"][0].get("patch").is_none());
    let plain_search_text = repo.run(["search", "retry"]);
    assert!(!String::from_utf8_lossy(&plain_search_text.stdout).contains("patch excerpt:"));

    let text = repo.run(["search", "retry", "--patch"]);
    assert_eq!(text.status.code(), Some(0));
    let text = String::from_utf8(text.stdout).expect("patch text must be UTF-8");
    assert!(text.contains("retry_delay"));

    for args in [
        vec!["failures", "retry", "--patch"],
        vec!["search", "--code", "delay", "--patch"],
    ] {
        assert_eq!(repo.run(args).status.code(), Some(2));
    }
}

#[test]
fn patch_reports_unavailable_and_no_relevant_hunks() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[("src/audit.rs", b"const STATE: u8 = 0;\n")],
        "Create audit source",
        1,
    );
    commit(
        &repo,
        &[("src/audit.rs", b"const STATE: u8 = 1;\n")],
        "Record metadataonly policy",
        2,
    );
    commit(
        &repo,
        &[("assets/payload.bin", b"binary\0payload")],
        "Add binaryonly payload",
        3,
    );
    repo.index();

    let metadata = json(&repo, &["search", "metadataonly", "--patch", "--json"]);
    assert_eq!(metadata["schema_version"], 2);
    assert_eq!(
        metadata["materials"][0]["patch"]["status"],
        "no_relevant_hunks"
    );
    assert_eq!(
        metadata["materials"][0]["patch"]["hunks"],
        serde_json::json!([])
    );
    let metadata_text = repo.run(["search", "metadataonly", "--patch"]);
    assert!(
        String::from_utf8_lossy(&metadata_text.stdout).contains("No relevant text hunk found.")
    );

    let binary = json(&repo, &["search", "binaryonly", "--patch", "--json"]);
    assert_eq!(binary["materials"][0]["patch"]["status"], "unavailable");
    assert_eq!(
        binary["materials"][0]["patch"]["hunks"],
        serde_json::json!([])
    );
    let binary_text = repo.run(["search", "binaryonly", "--patch"]);
    assert!(String::from_utf8_lossy(&binary_text.stdout).contains("Text hunk unavailable."));

    let empty = json(
        &repo,
        &["search", "nothingmatches-this", "--patch", "--json"],
    );
    assert_eq!(empty["schema_version"], 2);
    assert_eq!(empty["materials"], serde_json::json!([]));
}

#[cfg(target_os = "linux")]
#[test]
fn patch_json_preserves_non_utf8_paths_and_diff_bytes() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

    let repo = TestRepo::new();
    let path = Path::new(OsStr::from_bytes(b"src/raw-\xff.rs"));
    fs::create_dir_all(repo.dir.path().join("src")).expect("create source directory");
    fs::write(repo.dir.path().join(path), b"old line\n").expect("write initial file");
    commit(&repo, &[], "Create byte fixture", 1);

    let mut contents = b"old line\noctetsignal ".to_vec();
    contents.extend_from_slice(b"\xff\x1b\n");
    fs::write(repo.dir.path().join(path), contents).expect("write changed file");
    commit(&repo, &[], "Add octetsignal bytes", 2);
    repo.index();

    let output = json(&repo, &["search", "octetsignal", "--patch", "--json"]);
    let hunk = &output["materials"][0]["patch"]["hunks"][0];
    let encoded_path = hunk["new_path"]["base64"].as_str().unwrap();
    assert_eq!(STANDARD.decode(encoded_path).unwrap(), b"src/raw-\xff.rs");
    let encoded_text = hunk["text"]["base64"].as_str().unwrap();
    let text = STANDARD.decode(encoded_text).unwrap();
    assert!(
        text.windows(b"octetsignal".len())
            .any(|part| part == b"octetsignal")
    );
    assert!(text.contains(&0xff));
    assert!(text.contains(&0x1b));

    let text_output = repo.run(["search", "octetsignal", "--patch"]);
    assert_eq!(text_output.status.code(), Some(0));
    assert!(std::str::from_utf8(&text_output.stdout).is_ok());
    assert!(!text_output.stdout.contains(&0xff));
    assert!(!text_output.stdout.contains(&0x1b));
}

#[test]
fn patch_clips_oversized_hunks() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[
            ("src/large.rs", b"let value = 1;\n"),
            ("src/oversizedneedle.rs", b"let value = 0;\n"),
        ],
        "Create large fixture",
        1,
    );
    let mut changed = b"let retry_value = ".to_vec();
    changed.extend(std::iter::repeat_n(b'x', 40_000));
    changed.extend_from_slice(b";\n");
    let mut oversized = b"let oversizedneedle = ".to_vec();
    oversized.extend(std::iter::repeat_n(b'x', 100_000));
    oversized.extend_from_slice(b";\n");
    commit(
        &repo,
        &[
            ("src/large.rs", &changed),
            ("src/oversizedneedle.rs", &oversized),
        ],
        "Add large retry and oversizedneedle values",
        2,
    );
    repo.index();

    let output = json(&repo, &["search", "retry", "--patch", "--json"]);
    let patch = &output["materials"][0]["patch"];
    let hunk = &patch["hunks"][0];
    assert!(hunk["text"].as_str().unwrap().len() <= 8 * 1024);
    assert_eq!(hunk["truncated"], true);
    assert_eq!(patch["truncated"], true);
    let output = json(
        &repo,
        &["search", "oversizedneedle.rs", "--patch", "--json"],
    );
    let patch = &output["materials"][0]["patch"];
    assert_eq!(patch["status"], "available");
    let hunk = &patch["hunks"][0];
    assert!(hunk["text"].is_null());
    assert_eq!(hunk["truncated"], true);
    assert_eq!(patch["truncated"], true);
}

fn separated_source(count: usize, updated: bool, late_match: bool) -> Vec<u8> {
    let mut source = String::new();
    for index in 0..count {
        if updated && late_match && index + 1 == count {
            source.push_str("let lateprobe = true;\n");
        } else if updated {
            source.push_str(&format!("let hunkboundneedle_{index} = true;\n"));
        } else {
            source.push_str(&format!("let value_{index} = false;\n"));
        }
        for spacer in 0..10 {
            source.push_str(&format!("let filler_{index}_{spacer} = {spacer};\n"));
        }
    }
    source.into_bytes()
}

#[test]
fn patch_preserves_limit_and_examples_modes() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[("src/engine.rs", b"fn run() { let delay = 0; }\n")],
        "Create engine",
        1,
    );
    commit(
        &repo,
        &[("src/engine.rs", b"fn run() { let retry_delay = 1; }\n")],
        "Improve retry first time",
        2,
    );
    commit(
        &repo,
        &[("src/engine.rs", b"fn run() { let retry_delay = 2; }\n")],
        "Improve retry second time",
        3,
    );
    repo.index();

    let plain_search = json(&repo, &["search", "retry", "--limit", "1", "--json"]);
    let patch_search = json(
        &repo,
        &["search", "retry", "--limit", "1", "--patch", "--json"],
    );
    assert_eq!(plain_search["matched_count"], patch_search["matched_count"]);
    assert!(plain_search["matched_count"].as_u64().unwrap() > 1);
    assert_eq!(plain_search["materials"].as_array().unwrap().len(), 1);
    assert_eq!(patch_search["materials"].as_array().unwrap().len(), 1);
    assert_eq!(
        plain_search["materials"][0]["subject"],
        patch_search["materials"][0]["subject"]
    );

    let plain_examples = json(
        &repo,
        &[
            "examples",
            "retry",
            "--path",
            "src/engine.rs",
            "--limit",
            "1",
            "--json",
        ],
    );
    let patch_examples = json(
        &repo,
        &[
            "examples",
            "retry",
            "--path",
            "src/engine.rs",
            "--limit",
            "1",
            "--patch",
            "--json",
        ],
    );
    assert_eq!(
        plain_examples["matched_count"],
        patch_examples["matched_count"]
    );
    assert!(plain_examples["matched_count"].as_u64().unwrap() > 1);
    assert_eq!(plain_examples["materials"].as_array().unwrap().len(), 1);
    assert_eq!(patch_examples["materials"].as_array().unwrap().len(), 1);
    assert_eq!(
        plain_examples["materials"][0]["subject"],
        patch_examples["materials"][0]["subject"]
    );

    let plain_text = repo.run([
        "examples",
        "retry",
        "--path",
        "src/engine.rs",
        "--limit",
        "1",
    ]);
    let patch_text = repo.run([
        "examples",
        "retry",
        "--path",
        "src/engine.rs",
        "--limit",
        "1",
        "--patch",
    ]);
    assert_eq!(plain_text.status.code(), Some(0));
    assert_eq!(patch_text.status.code(), Some(0));
    assert!(!String::from_utf8_lossy(&plain_text.stdout).contains("patch excerpt:"));
    assert!(String::from_utf8_lossy(&patch_text.stdout).contains("patch excerpt: available"));
}

#[test]
fn patch_bounds_files_per_material() {
    let repo = TestRepo::new();
    for index in 0..10 {
        let path = repo.dir.path().join(format!("src/file-{index}.rs"));
        fs::create_dir_all(path.parent().unwrap()).expect("create source directory");
        fs::write(path, b"const value = 0;\n").expect("write initial source");
    }
    commit(&repo, &[], "Create file-bound fixture", 1);
    for index in 0..10 {
        let path = repo.dir.path().join(format!("src/file-{index}.rs"));
        fs::write(path, format!("const fileboundneedle_{index} = true;\n"))
            .expect("write changed source");
    }
    commit(&repo, &[], "Add fileboundneedle examples", 2);
    repo.index();

    let output = json(&repo, &["search", "fileboundneedle", "--patch", "--json"]);
    let patch = &output["materials"][0]["patch"];
    let hunks = patch["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 8);
    let files = hunks
        .iter()
        .map(|hunk| hunk["new_path"].as_str().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(files.len(), 8);
    assert_eq!(patch["truncated"], true);
}

#[test]
fn patch_bounds_hunks_per_material() {
    let repo = TestRepo::new();
    let before = separated_source(20, false, false);
    commit(
        &repo,
        &[("src/many.rs", &before)],
        "Create many-hunk fixture",
        1,
    );
    let after = separated_source(20, true, false);
    commit(
        &repo,
        &[("src/many.rs", &after)],
        "Add hunkboundneedle changes",
        2,
    );
    repo.index();

    let output = json(&repo, &["search", "hunkboundneedle", "--patch", "--json"]);
    let patch = &output["materials"][0]["patch"];
    let hunks = patch["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 16);
    assert!(hunks.iter().all(|hunk| hunk["new_path"] == "src/many.rs"));
    assert_eq!(patch["truncated"], true);
}

#[test]
fn patch_does_not_claim_no_match_after_incomplete_scan() {
    let repo = TestRepo::new();
    let before = separated_source(70, false, false);
    commit(
        &repo,
        &[("src/late.rs", &before)],
        "Create late-match fixture",
        1,
    );
    let after = separated_source(70, true, true);
    commit(
        &repo,
        &[("src/late.rs", &after)],
        "Add lateprobe to final hunk",
        2,
    );
    repo.index();

    let output = json(&repo, &["search", "lateprobe", "--patch", "--json"]);
    let patch = &output["materials"][0]["patch"];
    assert_eq!(patch["status"], "unavailable");
    assert_eq!(patch["hunks"], serde_json::json!([]));
    assert_eq!(patch["truncated"], true);
}

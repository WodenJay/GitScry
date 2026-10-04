mod support;

use std::{fs, path::Path};

use serde_json::Value;
use support::{TestRepo, git};

fn commit(repo: &TestRepo, contents: &[u8]) {
    fs::write(repo.dir.path().join("tracked.txt"), contents).expect("write tracked file");
    git(repo.dir.path(), ["add", "tracked.txt"]);
    git(repo.dir.path(), ["commit", "-m", "Initial commit"]);
}

fn json(output: &std::process::Output) -> Value {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid stats JSON")
}

fn command<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["commands"]
        .as_array()
        .expect("commands array")
        .iter()
        .find(|entry| entry["command"] == name)
        .expect("command entry")
}

#[test]
fn records_commands_across_repositories_and_excludes_the_report() {
    let first = TestRepo::new();
    let second = TestRepo::new();
    commit(&first, b"PRIVATE_QUERY_SENTINEL\n");
    commit(&second, b"another repository\n");
    let shared_user_data = tempfile::tempdir().expect("create shared user data");

    for repo in [&first, &second] {
        let indexed = repo.run_with_user_data_dir(["index"], shared_user_data.path());
        assert_eq!(indexed.status.code(), Some(0));
    }
    let searched = first.run_with_user_data_dir(
        ["search", "PRIVATE_QUERY_SENTINEL", "--json"],
        shared_user_data.path(),
    );
    assert_eq!(
        searched.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&searched.stderr)
    );

    let outside_repository = tempfile::tempdir().expect("create non-repository working directory");
    let failed = TestRepo::run_at_with_user_data_dir(
        outside_repository.path(),
        ["index"],
        shared_user_data.path(),
    );
    assert!(
        !failed.status.success(),
        "index outside a repository should fail"
    );

    let report_output = TestRepo::run_at_with_user_data_dir(
        outside_repository.path(),
        ["stats", "--all", "--json"],
        shared_user_data.path(),
    );
    let report = json(&report_output);
    assert_eq!(report["kind"], "stats");
    assert_eq!(report["group"], "day");
    assert_eq!(report["range"]["since"], Value::Null);
    assert_eq!(report["range"]["until"], Value::Null);
    assert_eq!(report["total"]["calls"], 4);
    assert_eq!(command(&report, "index")["calls"], 3);
    assert_eq!(command(&report, "search")["calls"], 1);
    assert!(report["total"]["cumulative_elapsed_ns"].as_i64().unwrap() >= 0);
    assert!(report["total"]["average_elapsed_ns"].is_number());
    assert!(
        !report_output
            .stdout
            .windows(22)
            .any(|window| window == b"PRIVATE_QUERY_SENTINEL")
    );
    assert!(
        !report_output
            .stdout
            .windows(first.dir.path().to_string_lossy().len())
            .any(|window| { window == first.dir.path().to_string_lossy().as_bytes() })
    );

    let repeated_report =
        json(&first.run_with_user_data_dir(["stats", "--all", "--json"], shared_user_data.path()));
    assert_eq!(repeated_report["total"]["calls"], 4);

    let text_report = first.run_with_user_data_dir(["stats", "--all"], shared_user_data.path());
    assert_eq!(text_report.status.code(), Some(0));
    let text = String::from_utf8(text_report.stdout).expect("text stats output");
    assert!(text.contains("Total: 4 calls |"));
    assert!(text.contains("index: 3 calls |"));
    assert!(text.contains("search: 1 calls |"));
}

#[test]
fn help_version_and_parse_failures_are_not_recorded() {
    let repo = TestRepo::new();
    for args in [
        vec!["--help"],
        vec!["--version"],
        vec!["stats", "--help"],
        vec!["search"],
        vec!["unknown-command"],
    ] {
        repo.run(args);
    }

    let report = json(&repo.run(["stats", "--all", "--json"]));
    assert_eq!(report["total"]["calls"], 0);
    assert_eq!(report["commands"], Value::Array(Vec::new()));
    assert_eq!(fs::read_dir(repo.user_data_dir()).unwrap().count(), 0);
}

#[test]
fn recording_failure_does_not_change_command_output_or_exit_status() {
    let repo = TestRepo::new();
    commit(&repo, b"searchable content\n");
    repo.index();

    let healthy_user_data = tempfile::tempdir().expect("create healthy user data");
    let baseline =
        repo.run_with_user_data_dir(["search", "searchable", "--json"], healthy_user_data.path());
    let unusable_user_data = tempfile::tempdir().expect("create unusable user data");
    let data_file = unusable_user_data.path().join("not-a-directory");
    fs::write(&data_file, b"block database directory").expect("create database path blocker");

    let storage_failure =
        repo.run_with_user_data_dir(["search", "searchable", "--json"], &data_file);
    assert_eq!(storage_failure.status.code(), baseline.status.code());
    assert_eq!(storage_failure.stdout, baseline.stdout);
    assert_eq!(storage_failure.stderr, baseline.stderr);
}

#[test]
fn concurrent_invocations_are_all_counted() {
    let repo = TestRepo::new();
    commit(&repo, b"concurrent search content\n");
    repo.index();

    let children = (0..8)
        .map(|_| {
            TestRepo::command_at(repo.dir.path(), repo.user_data_dir())
                .args(["search", "concurrent", "--json"])
                .spawn()
                .expect("spawn concurrent search")
        })
        .collect::<Vec<_>>();
    for child in children {
        let output = child
            .wait_with_output()
            .expect("wait for concurrent search");
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let report = json(&repo.run(["stats", "--all", "--json"]));
    assert_eq!(report["total"]["calls"], 9);
    assert_eq!(command(&report, "index")["calls"], 1);
    assert_eq!(command(&report, "search")["calls"], 8);
}

#[test]
fn invalid_or_reversed_stats_dates_are_reported_as_input_errors() {
    let repo = TestRepo::new();
    for args in [
        vec!["stats", "--since", "2025-2-01"],
        vec!["stats", "--since", "2025-+1-+2"],
        vec!["stats", "--since", "2025-02-02", "--until", "2025-02-01"],
        vec!["stats", "--all", "--since", "2025-01-01"],
    ] {
        let output = repo.run(args);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("error:"));
    }
}

#[test]
fn explicit_range_and_group_options_are_available_in_help() {
    let output = TestRepo::run_at(Path::new("."), ["stats", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for option in ["--since", "--until", "--all", "--group", "--json"] {
        assert!(
            help.contains(option),
            "stats help is missing {option}: {help}"
        );
    }
}

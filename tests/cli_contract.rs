mod support;

use std::{fs, path::Path};

use support::{TestRepo, git, git_command};

const CONTRACT: &str = include_str!("fixtures/cli-contract.txt");

fn commit(repo: &TestRepo, files: &[(&str, &[u8])], subject: &str, body: &str, date: &str) {
    for (path, contents) in files {
        let path = Path::new(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(repo.dir.path().join(parent)).expect("create parent directory");
        }
        fs::write(repo.dir.path().join(path), contents).expect("write tracked file");
    }
    git(repo.dir.path(), ["add", "--all"]);
    let output = git_command(repo.dir.path())
        .args(["commit", "-m", subject, "-m", body])
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn escaped(bytes: &[u8]) -> String {
    bytes.escape_ascii().map(char::from).collect()
}

#[test]
fn ordinary_commands_have_a_byte_exact_cli_contract() {
    let repo = TestRepo::new();
    commit(
        &repo,
        &[
            (
                "src/lib.rs",
                b"pub fn provider(value: &str) -> &str {\n    value\n}\n",
            ),
            (
                "tests/provider.rs",
                b"#[test]\nfn provider_accepts_values() {}\n",
            ),
        ],
        "Add provider normalization path",
        "Keep provider values available to callers.",
        "2000-01-01T00:00:00+0000",
    );
    commit(
        &repo,
        &[
            (
                "src/lib.rs",
                b"pub fn provider(value: &str) -> &str {\n    value.trim()\n}\n",
            ),
            (
                "tests/provider.rs",
                b"#[test]\nfn provider_normalizes_values() {}\n",
            ),
        ],
        "Retire legacy provider safely",
        "Normalize provider values so that stale input does not leak to callers.",
        "2000-01-02T00:00:00+0000",
    );
    let retired = repo.head();
    commit(
        &repo,
        &[
            (
                "src/lib.rs",
                b"pub fn provider(value: &str) -> &str {\n    value\n}\n",
            ),
            (
                "tests/provider.rs",
                b"#[test]\nfn provider_accepts_values() {}\n",
            ),
        ],
        "Revert \"Retire legacy provider safely\"",
        &format!(
            "This reverts commit {retired}.\n\nReason: provider normalization caused a regression in legacy callers.\nRetry: restore normalization after callers migrate."
        ),
        "2000-01-03T00:00:00+0000",
    );
    commit(
        &repo,
        &[
            (
                "src/lib.rs",
                b"pub fn provider(value: &str) -> &str {\n    value.trim()\n}\n",
            ),
            (
                "tests/provider.rs",
                b"#[test]\nfn provider_regression_is_fixed() {}\n",
            ),
        ],
        "Fix provider regression after revert",
        "Fix provider regression by restoring normalization after callers migrate.",
        "2000-01-04T00:00:00+0000",
    );

    let commands: [(&str, &[&str]); 8] = [
        ("search", &["search", "provider"]),
        (
            "examples",
            &["examples", "retire", "provider", "--path", "src/lib.rs"],
        ),
        (
            "failures",
            &["failures", "provider", "--path", "src/lib.rs"],
        ),
        ("related", &["related", "src/lib.rs"]),
        ("tests", &["tests", "src/lib.rs"]),
        ("why", &["why", "src/lib.rs", "--line", "1", "--limit", "3"]),
        (
            "regression",
            &["regression", "provider regression", "--path", "src/lib.rs"],
        ),
        ("trace-fix", &["trace-fix", "HEAD", "--path", "src/lib.rs"]),
    ];

    let mut observed = String::new();
    for (name, args) in commands {
        let output = repo.run(args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        observed.push_str(&format!(
            "[{name}]\nstatus={:?}\nstdout={}\nstderr={}\n",
            output.status.code(),
            escaped(&output.stdout),
            escaped(&output.stderr),
        ));
    }

    if std::env::var_os("GITSCRY_UPDATE_CLI_CONTRACT").is_some() {
        fs::write(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cli-contract.txt"),
            &observed,
        )
        .expect("write CLI contract");
    } else {
        assert_eq!(observed, CONTRACT);
    }
}

mod support;

use std::fs;
use support::{TestRepo, git};

fn commit(repo: &TestRepo, source: &str, subject: &str) -> String {
    fs::create_dir_all(repo.dir.path().join("src")).unwrap();
    fs::write(repo.dir.path().join("src/lib.rs"), source).unwrap();
    git(repo.dir.path(), ["add", "."]);
    git(repo.dir.path(), ["commit", "-m", subject]);
    repo.head()
}

fn query(repo: &TestRepo, command: &str, selector: &str, revision: &str) -> serde_json::Value {
    let args = if command == "regression" {
        vec![
            command,
            "failure",
            "--path",
            "src/lib.rs",
            "--symbol",
            selector,
            "--bad",
            revision,
            "--json",
        ]
    } else {
        vec![
            command,
            "src/lib.rs",
            "--symbol",
            selector,
            "--at",
            revision,
            "--json",
        ]
    };
    let output = repo.run(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn four_commands_expose_the_pinned_structured_selection() {
    let repo = TestRepo::new();
    let revision = commit(
        &repo,
        "mod net {\n #[inline]\n pub fn parse<T>(value: T) -> T {\n  value\n }\n fn neighbor() {}\n}\n",
        "Create parser",
    );
    repo.index();
    // Neither the working tree nor a later declaration changes the explicit target.
    fs::write(repo.dir.path().join("src/lib.rs"), "fn parse() {}\n").unwrap();
    for command in ["why", "tests", "related", "regression"] {
        let report = query(&repo, command, "net::parse", &revision);
        let selected = &report["symbol_selection"];
        assert_eq!(
            selected["input_selector"], "net::parse",
            "{command}: {report}"
        );
        assert_eq!(selected["qualified_name"], "net::parse");
        assert_eq!(selected["kind"], "function");
        assert_eq!(selected["start_line"], 2);
        assert_eq!(selected["end_line"], 5);
        assert_eq!(selected["identifier_line"], 3);
        assert_eq!(selected["language"], "rust");
        assert_eq!(selected["mode"], "structured");
    }
}

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

#[test]
fn qualified_history_relocates_ranges_and_excludes_neighbor_edits() {
    let repo = TestRepo::new();
    let initial = commit(
        &repo,
        "mod net {\n pub fn parse() -> i32 {\n  1\n }\n fn neighbor() -> i32 { 1 }\n}\n",
        "Create parser",
    );
    let changed = commit(
        &repo,
        "mod net {\n #[inline]\n pub fn parse() -> i32 {\n  2\n }\n fn neighbor() -> i32 { 1 }\n}\n",
        "Change parser",
    );
    let neighbor = commit(
        &repo,
        "mod net {\n #[inline]\n pub fn parse() -> i32 {\n  2\n }\n fn neighbor() -> i32 { 2 }\n}\n",
        "Change neighbor",
    );
    repo.index();
    let why = query(&repo, "why", "net::parse", &neighbor);
    assert_eq!(why["symbol_summary"]["introduction"]["commit_oid"], initial);
    let rendered = why["target_related_modifications"].to_string();
    assert!(rendered.contains(&changed), "{why}");
    assert!(!rendered.contains(&neighbor), "{why}");
    let regression = query(&repo, "regression", "net::parse", &neighbor);
    let rendered = regression.to_string();
    assert!(rendered.contains(&changed), "{regression}");
    assert!(!rendered.contains("Change neighbor"), "{regression}");
}

#[test]
fn historical_parse_degradation_is_observable_across_commands() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "fn parse() {\n let value = 1;\n}\nfn broken(\n",
        "Broken historical source",
    );
    let revision = commit(&repo, "fn parse() {\n let value = 2;\n}\n", "Repair source");
    repo.index();
    for command in ["why", "tests", "related", "regression"] {
        let report = query(&repo, command, "parse", &revision);
        assert!(
            report.to_string().contains("lightweight symbol selection"),
            "{command}: {report}"
        );
    }
}

#[test]
fn partial_qualification_does_not_follow_a_surviving_copy_source() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "mod old {\n mod net {\n  fn parse() {}\n }\n}\n",
        "Original source",
    );
    let revision = commit(
        &repo,
        "mod old {\n mod moved {\n  fn parse() {}\n }\n}\nmod new {\n mod net {\n  fn parse() {}\n }\n}\n",
        "Move and copy",
    );
    repo.index();
    let report = query(&repo, "why", "net::parse", &revision);
    assert_eq!(
        report["symbol_summary"]["introduction"]["status"], "unknown",
        "{report}"
    );
}

#[test]
fn composite_impl_methods_are_not_silently_omitted() {
    let repo = TestRepo::new();
    commit(
        &repo,
        "trait T { fn m(&self) {} }\nimpl T for (u8, u8) { fn m(&self) {} }\n",
        "Tuple implementation",
    );
    repo.index();
    let output = repo.run(["why", "src/lib.rs", "--symbol", "m"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("ambiguous"));
}

#[test]
fn regression_degradation_notices_are_unique() {
    let repo = TestRepo::new();
    let revision = commit(&repo, "fn parse() {}\nfn broken(\n", "Malformed source");
    repo.index();
    let report = query(&repo, "regression", "parse", &revision);
    let notices = report["warnings"].as_array().unwrap();
    assert_eq!(
        notices
            .iter()
            .filter(|notice| notice
                .as_str()
                .unwrap()
                .contains("lightweight symbol selection"))
            .count(),
        1,
        "{report}"
    );
}

#[test]
fn lightweight_results_disclose_mode_in_text_and_json_without_stripping_owners() {
    let repo = TestRepo::new();
    let revision = commit(&repo, "fn parse() {}\nfn broken(\n", "Malformed source");
    repo.index();
    for command in ["why", "tests", "related", "regression"] {
        let report = query(&repo, command, "parse", &revision);
        assert_eq!(report["symbol_selection"]["mode"], "lightweight");
        assert_eq!(report["symbol_selection"]["language"], "rust");
        let qualified = if command == "regression" {
            repo.run(vec![
                command,
                "failure",
                "--path",
                "src/lib.rs",
                "--symbol",
                "net::parse",
            ])
        } else {
            repo.run(vec![command, "src/lib.rs", "--symbol", "net::parse"])
        };
        assert_eq!(qualified.status.code(), Some(2));
    }
    let output = repo.run(["why", "src/lib.rs", "--symbol", "parse"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains(
            "parse -> parse (declaration, lines 1-1, identifier line 1, rust, lightweight)"
        ),
        "{text}"
    );
    assert!(text.contains("lightweight symbol selection"), "{text}");
}

fn commit_py(repo: &TestRepo, source: &str, subject: &str) -> String {
    fs::create_dir_all(repo.dir.path().join("src")).unwrap();
    fs::write(repo.dir.path().join("src/store.py"), source).unwrap();
    git(repo.dir.path(), ["add", "."]);
    git(repo.dir.path(), ["commit", "-m", subject]);
    repo.head()
}

fn query_py(repo: &TestRepo, command: &str, selector: &str, revision: &str) -> serde_json::Value {
    let args = if command == "regression" {
        vec![
            command,
            "failure",
            "--path",
            "src/store.py",
            "--symbol",
            selector,
            "--bad",
            revision,
            "--json",
        ]
    } else {
        vec![
            command,
            "src/store.py",
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
fn python_decorated_methods_track_history_across_commands() {
    let repo = TestRepo::new();
    let initial = commit_py(
        &repo,
        concat!(
            "class Store:\n",
            "    def run(self, value):\n",
            "        return value\n",
            "\n",
            "    def neighbor(self):\n",
            "        return 1\n",
        ),
        "Create store",
    );
    let decorated = commit_py(
        &repo,
        concat!(
            "class Store:\n",
            "    @staticmethod\n",
            "    def run(self, value):\n",
            "        return value\n",
            "\n",
            "    def neighbor(self):\n",
            "        return 1\n",
        ),
        "Decorate run",
    );
    let neighbor = commit_py(
        &repo,
        concat!(
            "class Store:\n",
            "    @staticmethod\n",
            "    def run(self, value):\n",
            "        return value\n",
            "\n",
            "    def neighbor(self):\n",
            "        return 2\n",
        ),
        "Change neighbor",
    );
    repo.index();
    for command in ["why", "tests", "related", "regression"] {
        let report = query_py(&repo, command, "Store.run", &neighbor);
        let selected = &report["symbol_selection"];
        assert_eq!(
            selected["input_selector"], "Store.run",
            "{command}: {report}"
        );
        assert_eq!(selected["qualified_name"], "Store.run");
        assert_eq!(selected["kind"], "method");
        assert_eq!(selected["start_line"], 2);
        assert_eq!(selected["end_line"], 4);
        assert_eq!(selected["identifier_line"], 3);
        assert_eq!(selected["language"], "python");
        assert_eq!(selected["mode"], "structured");
        if command == "why" {
            let introduction = &report["symbol_summary"]["introduction"]["commit_oid"];
            assert_eq!(introduction, &initial, "{command}: {report}");
        }
        // Only `why` (symbol_summary) and `regression` (symbol_changes) carry
        // symbol history material; `tests`/`related` expose selection only.
        let rendered = report.to_string();
        if command == "why" || command == "regression" {
            assert!(rendered.contains("Decorate run"), "{command}: {report}");
            assert!(!rendered.contains("Change neighbor"), "{command}: {report}");
        }
        let _ = decorated;
    }
}

#[test]
fn python_simple_and_suffix_selectors_resolve_uniquely() {
    let repo = TestRepo::new();
    let revision = commit_py(
        &repo,
        concat!(
            "def produce():\n",
            "    return 1\n",
            "\n",
            "class Store:\n",
            "    class Part:\n",
            "        def build(self):\n",
            "            return 2\n",
        ),
        "Create store",
    );
    repo.index();
    let report = query_py(&repo, "why", "build", &revision);
    assert_eq!(
        report["symbol_selection"]["qualified_name"],
        "Store.Part.build"
    );
    let report = query_py(&repo, "why", "Part.build", &revision);
    assert_eq!(
        report["symbol_selection"]["qualified_name"],
        "Store.Part.build"
    );
    let output = repo.run(["why", "src/store.py", "--symbol", "Store.run"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no supported declaration"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

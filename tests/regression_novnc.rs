mod support;

use std::fs;
use support::{TestRepo, git};

/// The verbatim noVNC core/rfb.js snapshot (tag 1.5.0, commit 7fcf9dc,
/// MPL-2.0) serves as a real-world regression fixture. The tests stay
/// offline: the fixture directory is committed into the test repository as-is.
fn fixture_source() -> &'static str {
    include_str!("fixtures/novnc-7fcf9dc/rfb.js")
}

fn commit_fixture(repo: &TestRepo, subject: &str) -> String {
    fs::create_dir_all(repo.dir.path().join("core")).unwrap();
    fs::write(repo.dir.path().join("core/rfb.js"), fixture_source()).unwrap();
    git(repo.dir.path(), ["add", "."]);
    git(repo.dir.path(), ["commit", "-m", subject]);
    repo.head()
}

fn query_why(repo: &TestRepo, selector: &str, revision: &str) -> serde_json::Value {
    let output = repo.run([
        "why",
        "core/rfb.js",
        "--symbol",
        selector,
        "--at",
        revision,
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn edit_disconnect(repo: &TestRepo) -> String {
    let path = repo.dir.path().join("core/rfb.js");
    let source = fs::read_to_string(&path).unwrap();
    let updated = source.replacen(
        "    disconnect() {\n",
        "    disconnect() {\n        // unrelated maintenance edit\n",
        1,
    );
    assert_ne!(source, updated, "disconnect() body must be editable");
    fs::write(&path, updated).unwrap();
    git(repo.dir.path(), ["add", "."]);
    git(repo.dir.path(), ["commit", "-m", "Edit disconnect"]);
    repo.head()
}

#[test]
fn novnc_symbols_resolve_structurally_with_shared_matching() {
    let repo = TestRepo::new();
    let revision = commit_fixture(&repo, "Import noVNC rfb.js at 7fcf9dc");
    repo.index();
    for (selector, qualified, kind, start, end) in [
        ("RFB", "RFB", "class", 88, 2936),
        (
            "clipboardPasteFrom",
            "RFB.clipboardPasteFrom",
            "method",
            496,
            530,
        ),
        (
            "RFB.clipboardPasteFrom",
            "RFB.clipboardPasteFrom",
            "method",
            496,
            530,
        ),
    ] {
        let report = query_why(&repo, selector, &revision);
        let selected = &report["symbol_selection"];
        assert_eq!(
            selected["qualified_name"], qualified,
            "{selector}: {report}"
        );
        assert_eq!(selected["kind"], kind, "{selector}: {report}");
        assert_eq!(selected["language"], "javascript", "{selector}: {report}");
        assert_eq!(selected["mode"], "structured", "{selector}: {report}");
        assert_eq!(selected["start_line"], start, "{selector}: {report}");
        assert_eq!(selected["end_line"], end, "{selector}: {report}");
    }
}

#[test]
fn novnc_independent_method_edits_are_not_modifications() {
    let repo = TestRepo::new();
    let initial = commit_fixture(&repo, "Import noVNC rfb.js at 7fcf9dc");
    let neighbor = edit_disconnect(&repo);
    repo.index();
    let report = query_why(&repo, "RFB.clipboardPasteFrom", &neighbor);
    assert_eq!(
        report["symbol_summary"]["introduction"]["commit_oid"], initial,
        "{report}"
    );
    let rendered = report["target_related_modifications"].to_string();
    assert!(
        !rendered.contains("Edit disconnect"),
        "unrelated method edits must not count: {rendered}"
    );
}

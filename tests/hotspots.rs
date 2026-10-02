mod support;

use serde_json::Value;
use std::fs;
use support::{TestRepo, git};

fn commit(repo: &TestRepo, path: &str, text: &str) {
    fs::write(repo.dir.path().join(path), text).unwrap();
    git(repo.dir.path(), ["add", "--all"]);
    git(repo.dir.path(), ["commit", "-m", "touch"]);
}
fn report(repo: &TestRepo, args: &[&str]) -> Value {
    let output = repo.run(args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn ranks_repeated_touches_at_published_tip_and_limits_after_sorting() {
    let repo = TestRepo::new();
    commit(&repo, "often", "one\n");
    commit(&repo, "once", &"large rewrite\n".repeat(100));
    commit(&repo, "often", "two\n");
    repo.index();
    let target = repo.head();
    commit(&repo, "unindexed", "ignored\n");
    fs::remove_file(repo.dir.path().join("often")).unwrap();
    let json = report(&repo, &["hotspots", "--json"]);
    assert_eq!(json["scope"]["target_rev"], target);
    assert_eq!(json["files"].as_array().unwrap().len(), 2);
    assert_eq!(json["files"][0]["path"], "often");
    assert_eq!(json["files"][0]["touching_commits"], 2);
    assert_eq!(json["files"][1]["touching_commits"], 1);
    assert_eq!(
        report(&repo, &["hotspots", "--json", "--limit", "1"])["files"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let text = repo.run(["hotspots"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.find("often").unwrap() < text.find("once").unwrap());
    assert!(text.contains(json["files"][0]["last_changed"].as_str().unwrap()));
    let stamp = json["files"][0]["last_changed"].as_str().unwrap();
    assert_eq!(stamp.len(), 20);
    assert!(stamp.ends_with('Z'));
    assert!(
        !json["files"][0]
            .as_object()
            .unwrap()
            .contains_key("additions")
    );
}

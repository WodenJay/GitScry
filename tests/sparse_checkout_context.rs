mod support;

use std::{fs, process::Output};
use support::{TestRepo, git};

#[test]
fn sparse_checkout_omissions_are_not_current_changes() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.dir.path().join("keep")).unwrap();
    fs::create_dir_all(repo.dir.path().join("omit")).unwrap();
    fs::write(repo.dir.path().join("keep/a.txt"), "keep\n").unwrap();
    fs::write(repo.dir.path().join("omit/b.txt"), "omit\n").unwrap();
    git(repo.dir.path(), ["add", "--all"]);
    git(repo.dir.path(), ["commit", "-m", "base"]);
    repo.index();
    git(repo.dir.path(), ["sparse-checkout", "init", "--cone"]);
    git(repo.dir.path(), ["sparse-checkout", "set", "keep"]);

    assert_eq!(
        support::git_stdout(repo.dir.path(), ["status", "--porcelain"]),
        "",
    );
    let real_index = repo.dir.path().join(".git/index");
    let index_before = fs::read(&real_index).unwrap();
    let report = json(repo.run(["context", "--json"]));
    assert_eq!(report["input"]["changes"], serde_json::json!([]));
    assert_eq!(fs::read(&real_index).unwrap(), index_before);
    assert_eq!(
        support::git_stdout(repo.dir.path(), ["status", "--porcelain"]),
        "",
    );
}

fn json(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

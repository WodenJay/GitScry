mod support;

use std::{fs, process::Output};
use support::{TestRepo, git};

fn query(repo: &TestRepo, json: bool, full: &str) -> Output {
    let mut command = TestRepo::command_at(repo.dir.path(), repo.user_data_dir());
    command.args(["timeline", "file.txt", "--limit", "1000"]);
    if json {
        command.arg("--json");
    }
    let output = command.env("GITSCRY_FULL_OUTPUT", full).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn captured_text_and_json_preserve_complete_output_in_unique_persistent_files() {
    let repo = TestRepo::new();
    fs::write(repo.dir.path().join("file.txt"), "hello\n").unwrap();
    git(repo.dir.path(), ["add", "."]);
    let subject = "historical material ".repeat(2400);
    // A message file avoids the Windows command-line length limit.
    fs::write(repo.dir.path().join("message"), subject).unwrap();
    git(repo.dir.path(), ["commit", "-F", "message"]);
    repo.index();

    let mut paths = Vec::new();
    for json in [false, true] {
        let full = query(&repo, json, "1");
        assert!(full.stdout.len() > 32768);
        if json {
            serde_json::from_slice::<serde_json::Value>(&full.stdout).unwrap();
        }
        for setting in ["", "true", "0"] {
            let displayed = query(&repo, json, setting);
            let text = String::from_utf8(displayed.stdout).unwrap();
            let (prefix, notice) = text
                .split_once("\n\n[GitScry output truncated]")
                .expect("truncation notice");
            assert!(full.stdout.starts_with(prefix.as_bytes()));
            assert!(prefix.len() <= 32768);
            assert!(prefix.lines().count() <= 400);
            assert!(notice.contains(&format!("{} UTF-8 bytes", full.stdout.len())));
            assert!(notice.contains(&format!(
                "{} lines",
                String::from_utf8_lossy(&full.stdout).lines().count()
            )));
            assert!(notice.contains("GITSCRY_FULL_OUTPUT=1"));
            assert!(notice.contains("rg or other search tools"));
            assert!(
                notice.find("search tools").unwrap()
                    < notice.find("read the complete file").unwrap()
            );
            let path = notice
                .lines()
                .find_map(|line| line.strip_prefix("Full output file: "))
                .unwrap();
            assert!(!paths.iter().any(|previous| previous == path));
            assert_eq!(fs::read(path).unwrap(), full.stdout);
            paths.push(path.to_owned());
        }
    }
    for path in paths {
        assert!(fs::metadata(&path).is_ok());
        fs::remove_file(path).unwrap();
    }
}

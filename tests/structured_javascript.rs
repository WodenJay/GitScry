mod support;

use std::fs;
use support::{TestRepo, git};

fn commit_source(repo: &TestRepo, file: &str, source: &str, subject: &str) -> String {
    fs::create_dir_all(repo.dir.path().join("src")).unwrap();
    fs::write(repo.dir.path().join(file), source).unwrap();
    git(repo.dir.path(), ["add", "."]);
    git(repo.dir.path(), ["commit", "-m", subject]);
    repo.head()
}

fn query_source(
    repo: &TestRepo,
    command: &str,
    file: &str,
    selector: &str,
    revision: &str,
) -> serde_json::Value {
    let args = if command == "regression" {
        vec![
            command, "failure", "--path", file, "--symbol", selector, "--bad", revision, "--json",
        ]
    } else {
        vec![
            command, file, "--symbol", selector, "--at", revision, "--json",
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
fn javascript_methods_track_history_across_commands_and_exclude_neighbors() {
    let repo = TestRepo::new();
    let initial = commit_source(
        &repo,
        "src/rfb.js",
        concat!(
            "export class RFB {\n",
            "    clipboardPasteFrom(text) {\n",
            "        this._send(text);\n",
            "    }\n",
            "\n",
            "    disconnect() {\n",
            "        this._socket.close();\n",
            "    }\n",
            "}\n",
        ),
        "Create clipboard paste",
    );
    let _changed = commit_source(
        &repo,
        "src/rfb.js",
        concat!(
            "export class RFB {\n",
            "    clipboardPasteFrom(text) {\n",
            "        this._send(text, { sync: true });\n",
            "    }\n",
            "\n",
            "    disconnect() {\n",
            "        this._socket.close();\n",
            "    }\n",
            "}\n",
        ),
        "Change clipboard paste",
    );
    let neighbor = commit_source(
        &repo,
        "src/rfb.js",
        concat!(
            "export class RFB {\n",
            "    clipboardPasteFrom(text) {\n",
            "        this._send(text, { sync: true });\n",
            "    }\n",
            "\n",
            "    disconnect() {\n",
            "        this._socket.close(1);\n",
            "    }\n",
            "}\n",
        ),
        "Change disconnect",
    );
    repo.index();
    for command in ["why", "tests", "related", "regression"] {
        let report = query_source(
            &repo,
            command,
            "src/rfb.js",
            "RFB.clipboardPasteFrom",
            &neighbor,
        );
        let selected = &report["symbol_selection"];
        assert_eq!(
            selected["input_selector"], "RFB.clipboardPasteFrom",
            "{command}: {report}"
        );
        assert_eq!(selected["qualified_name"], "RFB.clipboardPasteFrom");
        assert_eq!(selected["kind"], "method");
        assert_eq!(selected["start_line"], 2);
        assert_eq!(selected["end_line"], 4);
        assert_eq!(selected["identifier_line"], 2);
        assert_eq!(selected["language"], "javascript");
        assert_eq!(selected["mode"], "structured");
        let rendered = report.to_string();
        if command == "why" {
            assert_eq!(
                report["symbol_summary"]["introduction"]["commit_oid"],
                initial
            );
        }
        if command == "why" || command == "regression" {
            assert!(
                rendered.contains("Change clipboard paste"),
                "{command}: {report}"
            );
            assert!(
                !rendered.contains("Change disconnect"),
                "{command}: {report}"
            );
        }
    }
}

#[test]
fn javascript_suffix_selectors_and_ambiguity_rules_apply() {
    let repo = TestRepo::new();
    let revision = commit_source(
        &repo,
        "src/rfb.js",
        concat!(
            "export class RFB {\n",
            "    get status() {\n",
            "        return this._status;\n",
            "    }\n",
            "    set status(value) {\n",
            "        this._status = value;\n",
            "    }\n",
            "    async connect() {\n",
            "        await this._open();\n",
            "    }\n",
            "}\n",
        ),
        "Create status accessors",
    );
    repo.index();
    let report = query_source(&repo, "why", "src/rfb.js", "connect", &revision);
    assert_eq!(report["symbol_selection"]["qualified_name"], "RFB.connect");
    assert_eq!(report["symbol_selection"]["kind"], "method");
    let output = repo.run(["why", "src/rfb.js", "--symbol", "status"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ambiguous"), "{stderr}");
    assert!(stderr.contains("getter"), "{stderr}");
    assert!(stderr.contains("setter"), "{stderr}");
    assert!(!stderr.contains("lightweight symbol selection"), "{stderr}");
}

#[test]
fn typescript_interfaces_select_and_bodyless_signatures_do_not() {
    let repo = TestRepo::new();
    let revision = commit_source(
        &repo,
        "src/store.ts",
        concat!(
            "interface Options {\n",
            "    resize(value: number): void;\n",
            "}\n",
            "\n",
            "export class Store implements Options {\n",
            "    resize(value: number) {\n",
            "        this._size = value;\n",
            "    }\n",
            "}\n",
        ),
        "Create store",
    );
    repo.index();
    let report = query_source(&repo, "why", "src/store.ts", "Options", &revision);
    let selected = &report["symbol_selection"];
    assert_eq!(selected["qualified_name"], "Options", "{report}");
    assert_eq!(selected["kind"], "interface");
    assert_eq!(selected["language"], "typescript");
    assert_eq!(selected["mode"], "structured");
    let report = query_source(&repo, "why", "src/store.ts", "Store.resize", &revision);
    assert_eq!(report["symbol_selection"]["kind"], "method");
    assert_eq!(report["symbol_selection"]["language"], "typescript");
    // The unqualified method name resolves to the only location without a
    // bodyless signature: the concrete implementation.
    let output = repo.run(["why", "src/store.ts", "--symbol", "resize", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["symbol_selection"]["qualified_name"], "Store.resize");
    assert_eq!(report["symbol_selection"]["kind"], "method");
}

#[test]
fn tsx_and_jsx_grammars_follow_the_extension() {
    let repo = TestRepo::new();
    let revision = commit_source(
        &repo,
        "src/widget.tsx",
        concat!(
            "interface Props {\n",
            "    label: string;\n",
            "}\n",
            "\n",
            "export function Widget({ label }: Props) {\n",
            "    return <span className=\"label\">{label}</span>;\n",
            "}\n",
        ),
        "Create widget",
    );
    repo.index();
    let report = query_source(&repo, "why", "src/widget.tsx", "Widget", &revision);
    let selected = &report["symbol_selection"];
    assert_eq!(selected["qualified_name"], "Widget", "{report}");
    assert_eq!(selected["kind"], "function");
    assert_eq!(selected["language"], "typescript");
    assert_eq!(selected["mode"], "structured");
    let revision = commit_source(
        &repo,
        "src/legacy.jsx",
        concat!(
            "export function Badge({ text }) {\n",
            "    return <b>{text}</b>;\n",
            "}\n",
        ),
        "Create badge",
    );
    let report = query_source(&repo, "why", "src/legacy.jsx", "Badge", &revision);
    assert_eq!(report["symbol_selection"]["language"], "javascript");
    assert_eq!(report["symbol_selection"]["mode"], "structured");
}

#[test]
fn javascript_degradation_preserves_lightweight_rules() {
    let repo = TestRepo::new();
    let revision = commit_source(
        &repo,
        "src/rfb.js",
        "function parse() {}\nfunction broken(\n",
        "Malformed source",
    );
    repo.index();
    for command in ["why", "tests", "related", "regression"] {
        let report = query_source(&repo, command, "src/rfb.js", "parse", &revision);
        assert_eq!(
            report["symbol_selection"]["mode"], "lightweight",
            "{command}: {report}"
        );
        assert_eq!(report["symbol_selection"]["language"], "javascript");
        assert!(report.to_string().contains("lightweight symbol selection"));
    }
}

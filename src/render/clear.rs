use std::fmt::Write;

use crate::cache::ClearReport;

pub(crate) fn format_report(report: &ClearReport) -> String {
    let mut output = String::new();
    if report.dry_run {
        output.push_str("Dry run: would clear the entire repository-wide GitScry cache.\n");
    } else {
        output.push_str("Cleared the entire repository-wide GitScry cache.\n");
    }

    output.push_str("Cache material:\n");
    if let Some(counts) = &report.counts {
        writeln!(output, "  commits: {}", counts.commits).unwrap();
        writeln!(output, "  changes: {}", counts.changes).unwrap();
        writeln!(output, "  path records: {}", counts.path_records).unwrap();
        writeln!(output, "  hunks: {}", counts.hunks).unwrap();
        writeln!(output, "  semantic vectors: {}", counts.semantic_vectors).unwrap();
    } else {
        output.push_str("  counts: unavailable (cache database could not be read)\n");
    }

    if report.dry_run {
        output.push_str("Planned data-file deletions:\n");
    } else {
        output.push_str("Deleted data files:\n");
    }
    if report.data_files.is_empty() {
        output.push_str("  none\n");
    } else {
        for file in &report.data_files {
            writeln!(
                output,
                "  gitscry/{} ({} bytes)",
                file.name, file.size_bytes
            )
            .unwrap();
        }
    }

    output.push_str("Cache data-file usage:\n");
    writeln!(output, "  before: {} bytes", report.before_bytes).unwrap();
    if report.dry_run {
        writeln!(output, "  projected after: {} bytes", report.after_bytes).unwrap();
        writeln!(output, "  would release: {} bytes", report.released_bytes).unwrap();
    } else {
        writeln!(output, "  after: {} bytes", report.after_bytes).unwrap();
        writeln!(output, "  released: {} bytes", report.released_bytes).unwrap();
    }
    output.push_str("  Byte totals are measured file sizes; filesystem allocation may differ.\n");
    output.push_str(
        "Retained: gitscry/cache.lock, gitscry/.gitignore, Git history, and globally shared model/runtime resources.\n",
    );
    output
}

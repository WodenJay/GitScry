use std::fmt::Write;

use crate::cache::PruneReport;

pub(crate) fn format_report(report: &PruneReport) -> String {
    let mut output = String::new();
    if report.dry_run {
        output.push_str(
            "Dry run: inspect repository-wide GitScry material for missing Git commit objects.\n",
        );
    } else if report.counts.commits == 0 {
        output
            .push_str("No cached commits have missing Git objects; the cache was not compacted.\n");
    } else {
        output
            .push_str("Pruned repository-wide GitScry material for missing Git commit objects.\n");
    }

    output.push_str("Cache material:\n");
    if report.dry_run {
        writeln!(output, "  candidate commits: {}", report.counts.commits).unwrap();
    } else {
        writeln!(output, "  deleted commits: {}", report.counts.commits).unwrap();
    }
    writeln!(output, "  changes: {}", report.counts.changes).unwrap();
    writeln!(output, "  path records: {}", report.counts.path_records).unwrap();
    writeln!(output, "  hunks: {}", report.counts.hunks).unwrap();
    writeln!(
        output,
        "  semantic vectors: {}",
        report.counts.semantic_vectors
    )
    .unwrap();

    output.push_str("Cache data-file usage:\n");
    writeln!(output, "  before: {} bytes", report.before_bytes).unwrap();
    if report.dry_run {
        output.push_str(
            "  after/released: not estimated (dry run does not estimate reclaimed bytes).\n",
        );
    } else {
        writeln!(
            output,
            "  after: {} bytes",
            report
                .after_bytes
                .expect("completed prune measured the resulting cache size")
        )
        .unwrap();
        writeln!(
            output,
            "  released: {} bytes",
            report
                .released_bytes
                .expect("completed prune measured released cache bytes")
        )
        .unwrap();
    }
    output.push_str("  Byte totals are measured file sizes; filesystem allocation may differ.\n");
    output
}

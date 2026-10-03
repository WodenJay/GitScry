use crate::analysis::capabilities::usage::Report;

pub(crate) fn format_text(report: &Report) -> String {
    let mut output = format!(
        "GitScry usage statistics\nRange: {}\nGrouping: {}{}\nTotal: {} calls | {} cumulative | {} average",
        format_range(report),
        report.group,
        if report.group == "week" {
            " (weeks start Monday)"
        } else {
            ""
        },
        report.total.calls,
        format_elapsed(report.total.cumulative_elapsed_ns as f64),
        format_average(report.total.average_elapsed_ns),
    );

    output.push_str("\n\nPer command:");
    if report.commands.is_empty() {
        output.push_str("\n  No recorded commands in this range.");
    } else {
        for command in &report.commands {
            output.push_str(&format!(
                "\n  {}: {} calls | {} cumulative | {} average",
                command.command,
                command.calls,
                format_elapsed(command.cumulative_elapsed_ns as f64),
                format_average(command.average_elapsed_ns),
            ));
        }
    }

    output.push_str(&format!("\n\n{} call trend:", report.group));
    if report.trends.is_empty() {
        output.push_str("\n  No recorded commands in this range.");
    } else {
        for trend in &report.trends {
            output.push_str(&format!("\n  {}: {} calls", trend.period, trend.calls));
        }
    }
    output
}

pub(crate) fn format_json(report: &Report) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(report)
}

fn format_range(report: &Report) -> String {
    match (report.range.since.as_deref(), report.range.until.as_deref()) {
        (None, None) => "all recorded dates".to_owned(),
        (Some(since), Some(until)) => format!("{since} through {until}"),
        (Some(since), None) => format!("from {since}"),
        (None, Some(until)) => format!("through {until}"),
    }
}

fn format_average(elapsed_ns: Option<f64>) -> String {
    elapsed_ns.map_or_else(|| "n/a".to_owned(), format_elapsed)
}

fn format_elapsed(elapsed_ns: f64) -> String {
    format!("{:.3} ms", elapsed_ns / 1_000_000.0)
}

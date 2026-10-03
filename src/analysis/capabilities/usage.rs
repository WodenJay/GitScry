//! Local command-use aggregation and date-range policy.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::cache::usage::{self, DailyUsageRecord};
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Date {
    year: u16,
    month: u8,
    day: u8,
}

impl Date {
    fn parse(value: &str) -> Option<Self> {
        let bytes = value.as_bytes();
        if bytes.len() != 10
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || !bytes[..4].iter().all(|byte| byte.is_ascii_digit())
            || !bytes[5..7].iter().all(|byte| byte.is_ascii_digit())
            || !bytes[8..10].iter().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let year = value.get(0..4)?.parse().ok()?;
        let month = value.get(5..7)?.parse().ok()?;
        let day = value.get(8..10)?.parse().ok()?;
        let date = Self { year, month, day };
        (year > 0 && (1..=12).contains(&month) && (1..=date.days_in_month()).contains(&day))
            .then_some(date)
    }

    fn days_in_month(self) -> u8 {
        match self.month {
            2 if Self::is_leap_year(self.year) => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }

    fn is_leap_year(year: u16) -> bool {
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
    }

    fn previous_day(self) -> Option<Self> {
        if self.day > 1 {
            Some(Self {
                day: self.day - 1,
                ..self
            })
        } else if self.month > 1 {
            let previous = Self {
                month: self.month - 1,
                day: 1,
                ..self
            };
            Some(Self {
                day: previous.days_in_month(),
                ..previous
            })
        } else if self.year > 1 {
            Some(Self {
                year: self.year - 1,
                month: 12,
                day: 31,
            })
        } else {
            None
        }
    }

    fn days_before(mut self, count: usize) -> Self {
        for _ in 0..count {
            let Some(previous) = self.previous_day() else {
                break;
            };
            self = previous;
        }
        self
    }

    fn monday(self) -> Self {
        self.days_before(usize::from(self.weekday_monday_zero()))
    }

    fn weekday_monday_zero(self) -> u8 {
        let mut year = i64::from(self.year);
        let mut month = i64::from(self.month);
        if month < 3 {
            year -= 1;
            month += 12;
        }
        let day = i64::from(self.day);
        let century_year = year % 100;
        let century = year / 100;
        let zeller = (day
            + (13 * (month + 1)) / 5
            + century_year
            + century_year / 4
            + century / 4
            + 5 * century)
            % 7;
        ((zeller + 5) % 7) as u8
    }

    fn iso(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    fn month(self) -> String {
        format!("{:04}-{:02}", self.year, self.month)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GroupBy {
    Day,
    Week,
    Month,
}

impl GroupBy {
    fn name(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }

    fn period(self, date: Date) -> String {
        match self {
            Self::Day => date.iso(),
            Self::Week => date.monday().iso(),
            Self::Month => date.month(),
        }
    }
}

#[derive(Debug)]
pub(crate) enum Error {
    Input(String),
    Storage(String),
}

#[derive(Clone, Copy, Debug)]
struct DateRange {
    since: Option<Date>,
    until: Option<Date>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Report {
    pub(crate) schema_version: u8,
    pub(crate) kind: &'static str,
    pub(crate) timezone: &'static str,
    pub(crate) range: ReportRange,
    pub(crate) group: &'static str,
    pub(crate) total: Totals,
    pub(crate) commands: Vec<CommandUsage>,
    pub(crate) trends: Vec<Trend>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReportRange {
    pub(crate) since: Option<String>,
    pub(crate) until: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Totals {
    pub(crate) calls: u64,
    pub(crate) cumulative_elapsed_ns: u64,
    pub(crate) average_elapsed_ns: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommandUsage {
    pub(crate) command: String,
    pub(crate) calls: u64,
    pub(crate) cumulative_elapsed_ns: u64,
    pub(crate) average_elapsed_ns: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Trend {
    pub(crate) period: String,
    pub(crate) calls: u64,
}

#[derive(Default)]
struct Accumulator {
    calls: u64,
    elapsed_ns: u64,
}

impl Accumulator {
    fn add(&mut self, calls: u64, elapsed_ns: u64) {
        self.calls = self.calls.saturating_add(calls);
        self.elapsed_ns = self.elapsed_ns.saturating_add(elapsed_ns);
    }

    fn average(&self) -> Option<f64> {
        (self.calls > 0).then(|| self.elapsed_ns as f64 / self.calls as f64)
    }
}
pub(crate) fn report(
    all: bool,
    since: Option<&str>,
    until: Option<&str>,
    group: GroupBy,
) -> Result<Report, Error> {
    let today = usage::local_today().map_err(Error::Storage)?;
    let today = Date::parse(&today)
        .ok_or_else(|| Error::Storage(format!("invalid local calendar date '{today}'")))?;
    let range = resolve_range(all, since, until, today)?;
    let records = usage::read_records().map_err(Error::Storage)?;
    build_report(&records, range, group)
}

fn resolve_range(
    all: bool,
    since: Option<&str>,
    until: Option<&str>,
    today: Date,
) -> Result<DateRange, Error> {
    if all && (since.is_some() || until.is_some()) {
        return Err(Error::Input(
            "--all cannot be combined with --since or --until".to_owned(),
        ));
    }
    if all {
        return Ok(DateRange {
            since: None,
            until: None,
        });
    }

    let explicit_range = since.is_some() || until.is_some();
    let since = since
        .map(|value| parse_option_date("--since", value))
        .transpose()?;
    let until = until
        .map(|value| parse_option_date("--until", value))
        .transpose()?
        .or(Some(today));
    let since = match since {
        Some(date) => Some(date),
        None if explicit_range => None,
        None => Some(today.days_before(29)),
    };
    if since.zip(until).is_some_and(|(start, end)| start > end) {
        return Err(Error::Input("--since must not be after --until".to_owned()));
    }
    Ok(DateRange { since, until })
}

fn parse_option_date(option: &str, value: &str) -> Result<Date, Error> {
    Date::parse(value).ok_or_else(|| {
        Error::Input(format!(
            "invalid {option} date '{value}'; expected a valid YYYY-MM-DD date"
        ))
    })
}

fn build_report(
    records: &[DailyUsageRecord],
    range: DateRange,
    group: GroupBy,
) -> Result<Report, Error> {
    let mut total = Accumulator::default();
    let mut commands = BTreeMap::<String, Accumulator>::new();
    let mut trends = BTreeMap::<String, u64>::new();

    for record in records {
        let date = Date::parse(&record.date).ok_or_else(|| {
            Error::Storage(format!("invalid date in usage database: '{}'", record.date))
        })?;
        if range.since.is_some_and(|since| date < since)
            || range.until.is_some_and(|until| date > until)
        {
            continue;
        }
        total.add(record.calls, record.elapsed_ns);
        commands
            .entry(record.command.clone())
            .or_default()
            .add(record.calls, record.elapsed_ns);
        let calls = trends.entry(group.period(date)).or_default();
        *calls = calls.saturating_add(record.calls);
    }

    Ok(Report {
        schema_version: 1,
        kind: "stats",
        timezone: "local",
        range: ReportRange {
            since: range.since.map(Date::iso),
            until: range.until.map(Date::iso),
        },
        group: group.name(),
        total: Totals {
            calls: total.calls,
            cumulative_elapsed_ns: total.elapsed_ns,
            average_elapsed_ns: total.average(),
        },
        commands: commands
            .into_iter()
            .map(|(command, totals)| CommandUsage {
                command,
                calls: totals.calls,
                cumulative_elapsed_ns: totals.elapsed_ns,
                average_elapsed_ns: totals.average(),
            })
            .collect(),
        trends: trends
            .into_iter()
            .map(|(period, calls)| Trend { period, calls })
            .collect(),
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    fn date(value: &str) -> Date {
        Date::parse(value).expect("valid fixture date")
    }

    fn usage(day: &str, command: &str, calls: u64, elapsed_ns: u64) -> DailyUsageRecord {
        DailyUsageRecord {
            date: day.to_owned(),
            command: command.to_owned(),
            calls,
            elapsed_ns,
        }
    }

    #[test]
    fn default_range_covers_thirty_local_calendar_days() {
        let range = resolve_range(false, None, None, date("2024-03-01")).unwrap();
        assert_eq!(range.since.map(Date::iso).as_deref(), Some("2024-02-01"));
        assert_eq!(range.until.map(Date::iso).as_deref(), Some("2024-03-01"));
    }

    #[test]
    fn range_filtering_keeps_partial_week_and_month_buckets_partial() {
        let records = [
            usage("2024-01-30", "index", 7, 700),
            usage("2024-01-31", "search", 2, 400),
            usage("2024-02-01", "search", 1, 100),
            usage("2024-02-01", "index", 1, 50),
            usage("2024-02-02", "update", 9, 900),
        ];
        let range = DateRange {
            since: Some(date("2024-01-31")),
            until: Some(date("2024-02-01")),
        };

        let weekly = build_report(&records, range, GroupBy::Week).unwrap();
        assert_eq!(weekly.total.calls, 4);
        assert_eq!(weekly.total.cumulative_elapsed_ns, 550);
        assert_eq!(weekly.total.average_elapsed_ns, Some(137.5));
        assert_eq!(weekly.trends.len(), 1);
        assert_eq!(weekly.trends[0].period, "2024-01-29");
        assert_eq!(weekly.trends[0].calls, 4);

        let monthly = build_report(&records, range, GroupBy::Month).unwrap();
        assert_eq!(monthly.trends.len(), 2);
        assert_eq!(monthly.trends[0].period, "2024-01");
        assert_eq!(monthly.trends[0].calls, 2);
        assert_eq!(monthly.trends[1].period, "2024-02");
        assert_eq!(monthly.trends[1].calls, 2);

        let search = monthly
            .commands
            .iter()
            .find(|usage| usage.command == "search")
            .unwrap();
        assert_eq!(search.calls, 3);
        assert_eq!(search.cumulative_elapsed_ns, 500);
        assert_eq!(search.average_elapsed_ns, Some(500.0 / 3.0));
    }

    #[test]
    fn invalid_dates_and_reversed_ranges_are_rejected() {
        assert!(parse_option_date("--since", "2025-2-01").is_err());
        assert!(parse_option_date("--since", "2025-02-29").is_err());
        assert!(
            resolve_range(
                false,
                Some("2025-02-02"),
                Some("2025-02-01"),
                date("2025-03-01"),
            )
            .is_err()
        );
    }
}

use crate::{
    analysis::SearchScopeInfo,
    app::AppError,
    cache::{QuerySession, SearchFilter},
    git::Repository,
};

#[derive(Default)]
pub(super) struct SearchScopeOptions {
    pub(super) from_rev: Option<String>,
    pub(super) to_rev: Option<String>,
    pub(super) since: Option<String>,
    pub(super) until: Option<String>,
}

impl SearchScopeOptions {
    pub(super) fn is_empty(&self) -> bool {
        self.from_rev.is_none()
            && self.to_rev.is_none()
            && self.since.is_none()
            && self.until.is_none()
    }
}

pub(super) struct ResolvedSearchScope {
    pub(super) filter: SearchFilter,
    pub(super) report: SearchScopeInfo,
}

struct TimeBound {
    second: i64,
    fraction: String,
    normalized: String,
    filter_second: i64,
    date_end_exclusive: bool,
}

impl TimeBound {
    fn compares_after(&self, other: &Self) -> bool {
        match self.second.cmp(&other.second) {
            std::cmp::Ordering::Equal => compare_fraction(&self.fraction, &other.fraction).is_gt(),
            order => order.is_gt(),
        }
    }

    fn compares_at_or_after(&self, other: &Self) -> bool {
        match self.second.cmp(&other.second) {
            std::cmp::Ordering::Equal => !compare_fraction(&self.fraction, &other.fraction).is_lt(),
            order => order.is_gt(),
        }
    }
}

pub(super) fn resolve(
    session: &QuerySession,
    options: SearchScopeOptions,
) -> Result<Option<ResolvedSearchScope>, AppError> {
    resolve_with_target(session, options, None)
}

pub(super) fn resolve_for_target(
    session: &QuerySession,
    options: SearchScopeOptions,
    target_revision: &str,
) -> Result<Option<ResolvedSearchScope>, AppError> {
    resolve_with_target(session, options, Some(target_revision))
}

fn resolve_with_target(
    session: &QuerySession,
    options: SearchScopeOptions,
    target_revision: Option<&str>,
) -> Result<Option<ResolvedSearchScope>, AppError> {
    if options.is_empty() {
        return Ok(None);
    }

    let since = options
        .since
        .as_deref()
        .map(|value| parse_time_bound(value, "since", false))
        .transpose()?;
    let until = options
        .until
        .as_deref()
        .map(|value| parse_time_bound(value, "until", true))
        .transpose()?;
    if let (Some(since), Some(until)) = (&since, &until) {
        let reversed = if until.date_end_exclusive {
            since.compares_at_or_after(until)
        } else {
            since.compares_after(until)
        };
        if reversed {
            return Err(AppError::input("--since must not be later than --until"));
        }
    }

    let cache_tip = session.completed_tip()?;
    let repository = Repository::discover()?;
    let to_rev = match options.to_rev.as_deref() {
        Some(requested) => {
            let revision = repository.resolve_commit(requested)?;
            require_cached_revision(session, requested, &revision)?;
            revision
        }
        None => target_revision
            .map(str::to_owned)
            .unwrap_or_else(|| cache_tip.clone()),
    };
    let from_rev = options
        .from_rev
        .as_deref()
        .map(|requested| {
            let revision = repository.resolve_commit(requested)?;
            require_cached_revision(session, requested, &revision)?;
            Ok::<_, AppError>(revision)
        })
        .transpose()?;

    if let Some(from_rev) = &from_rev {
        let scope_reachable = session.ancestors(&to_rev)?;
        if !scope_reachable.contains(from_rev) {
            return Err(AppError::input(
                "--from-rev must be an ancestor of --to-rev (or the effective upper revision)",
            ));
        }
        if let Some(target_revision) = target_revision {
            let target_reachable = session.ancestors(target_revision)?;
            if !target_reachable.contains(from_rev) {
                return Err(AppError::input(
                    "--from-rev must be an ancestor of both the scope upper revision and target revision",
                ));
            }
        }
    }

    let filter = SearchFilter {
        from_oid: from_rev.clone(),
        to_oid: to_rev.clone(),
        since: since.as_ref().map(|bound| bound.filter_second),
        until: until.as_ref().map(|bound| bound.filter_second),
    };
    let report = SearchScopeInfo {
        from_rev,
        to_rev,
        target_rev: target_revision.map(str::to_owned),
        since: since.map(|bound| bound.normalized),
        until: until.map(|bound| bound.normalized),
        cache_tip,
    };
    Ok(Some(ResolvedSearchScope { filter, report }))
}

fn require_cached_revision(
    session: &QuerySession,
    requested: &str,
    resolved: &str,
) -> Result<(), AppError> {
    if session.contains_revision(resolved)? {
        Ok(())
    } else {
        Err(AppError::input(format!(
            "revision {requested} is outside the published cache generation"
        )))
    }
}

fn parse_time_bound(value: &str, option: &str, is_until: bool) -> Result<TimeBound, AppError> {
    let invalid = || {
        AppError::input(format!(
            "invalid --{option} value `{value}`: expected YYYY-MM-DD or an RFC 3339 timestamp with Z or an explicit UTC offset"
        ))
    };
    if !value.is_ascii() {
        return Err(invalid());
    }
    if value.len() == 10 {
        let (year, month, day) = parse_date(value).ok_or_else(invalid)?;
        let day_start = days_from_civil(year, month, day)
            .checked_mul(86_400)
            .ok_or_else(invalid)?;
        if is_until {
            let next_day = day_start.checked_add(86_400).ok_or_else(invalid)?;
            let filter_second = next_day.checked_sub(1).ok_or_else(invalid)?;
            return Ok(TimeBound {
                second: next_day,
                fraction: String::new(),
                normalized: value.to_owned(),
                filter_second,
                date_end_exclusive: true,
            });
        }
        return Ok(TimeBound {
            second: day_start,
            fraction: String::new(),
            normalized: value.to_owned(),
            filter_second: day_start,
            date_end_exclusive: false,
        });
    }

    let (second, fraction) = parse_timestamp(value).ok_or_else(invalid)?;
    let filter_second = if !is_until && fraction.bytes().any(|byte| byte != b'0') {
        second.checked_add(1).ok_or_else(invalid)?
    } else {
        second
    };
    let normalized = format_utc_timestamp(second, &fraction).ok_or_else(invalid)?;
    Ok(TimeBound {
        second,
        fraction,
        normalized,
        filter_second,
        date_end_exclusive: false,
    })
}

fn parse_date(value: &str) -> Option<(i64, u32, u32)> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = parse_digits(&bytes[0..4])? as i64;
    let month = parse_digits(&bytes[5..7])?;
    let day = parse_digits(&bytes[8..10])?;
    if year == 0 || !(1..=12).contains(&month) {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days_in_month = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=days_in_month)
        .contains(&day)
        .then_some((year, month, day))
}

fn parse_timestamp(value: &str) -> Option<(i64, String)> {
    let bytes = value.as_bytes();
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || (bytes[10] != b'T' && bytes[10] != b't')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let (year, month, day) = parse_date(&value[0..10])?;
    let hour = parse_digits(&bytes[11..13])?;
    let minute = parse_digits(&bytes[14..16])?;
    let second = parse_digits(&bytes[17..19])?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let mut position = 19;
    let mut fraction = String::new();
    if bytes.get(position) == Some(&b'.') {
        position += 1;
        let start = position;
        while bytes.get(position).is_some_and(u8::is_ascii_digit) {
            position += 1;
        }
        if position == start {
            return None;
        }
        fraction = value[start..position].trim_end_matches('0').to_owned();
    }

    let (offset_sign, offset_hour, offset_minute) = match bytes.get(position..) {
        Some([b'Z']) | Some([b'z']) => (0_i64, 0_i64, 0_i64),
        Some(
            [
                sign @ (b'+' | b'-'),
                hour_tens,
                hour_ones,
                b':',
                minute_tens,
                minute_ones,
            ],
        ) => {
            let hour = parse_digits(&[*hour_tens, *hour_ones])? as i64;
            let minute = parse_digits(&[*minute_tens, *minute_ones])? as i64;
            if hour > 23 || minute > 59 {
                return None;
            }
            (if *sign == b'+' { 1 } else { -1 }, hour, minute)
        }
        _ => return None,
    };
    let days = days_from_civil(year, month, day);
    let local_second = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    let offset = offset_sign * (offset_hour * 3_600 + offset_minute * 60);
    Some((local_second.checked_sub(offset)?, fraction))
}

fn parse_digits(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0_u32, |number, byte| {
        byte.is_ascii_digit()
            .then(|| number * 10 + u32::from(byte - b'0'))
    })
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let adjusted_month = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn format_utc_timestamp(second: i64, fraction: &str) -> Option<String> {
    let days = second.div_euclid(86_400);
    let seconds_of_day = second.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(1..=9_999).contains(&year) {
        return None;
    }
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    let fraction = if fraction.is_empty() {
        String::new()
    } else {
        format!(".{fraction}")
    };
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}{fraction}Z"
    ))
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    (year, month, day)
}

fn compare_fraction(left: &str, right: &str) -> std::cmp::Ordering {
    let length = left.len().max(right.len());
    (0..length)
        .map(|index| {
            left.as_bytes()
                .get(index)
                .copied()
                .unwrap_or(b'0')
                .cmp(&right.as_bytes().get(index).copied().unwrap_or(b'0'))
        })
        .find(|order| !order.is_eq())
        .unwrap_or(std::cmp::Ordering::Equal)
}

use crate::api_types::DashboardDataWarning;
use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

use super::ReadModelError;

pub(super) struct UsageClock {
    pub(super) timezone: String,
    pub(super) tz: Tz,
    pub(super) now_utc: DateTime<Utc>,
    pub(super) now_local: DateTime<Tz>,
    pub(super) warnings: Vec<DashboardDataWarning>,
}

pub(super) struct UsageWindowRange {
    pub(super) key: &'static str,
    pub(super) label: &'static str,
    pub(super) since_utc: Option<DateTime<Utc>>,
    pub(super) until_utc: DateTime<Utc>,
    pub(super) range_start: Option<String>,
    pub(super) range_end: String,
    pub(super) range_label: String,
}

pub(super) fn resolve_usage_clock(
    generated_at: &str,
    requested_timezone: Option<&str>,
) -> Result<UsageClock, ReadModelError> {
    let now_utc = DateTime::parse_from_rfc3339(generated_at)?.with_timezone(&Utc);
    let mut warnings = Vec::new();

    let (timezone, tz) = match requested_timezone
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    {
        Some(raw) => match raw.parse::<Tz>() {
            Ok(tz) => (raw.to_owned(), tz),
            Err(_) => {
                warnings.push(DashboardDataWarning {
                    source: "usage.timezone".to_owned(),
                    kind: "invalid_timezone".to_owned(),
                    message: format!("invalid usage timezone `{raw}`; falling back to UTC"),
                });
                ("UTC".to_owned(), chrono_tz::UTC)
            }
        },
        None => {
            warnings.push(DashboardDataWarning {
                source: "usage.timezone".to_owned(),
                kind: "missing_timezone".to_owned(),
                message: "missing usage timezone; falling back to UTC".to_owned(),
            });
            ("UTC".to_owned(), chrono_tz::UTC)
        }
    };
    let now_local = now_utc.with_timezone(&tz);

    Ok(UsageClock {
        timezone,
        tz,
        now_utc,
        now_local,
        warnings,
    })
}

pub(super) fn usage_window_ranges(
    clock: &UsageClock,
) -> Result<Vec<UsageWindowRange>, ReadModelError> {
    let today = clock.now_local.date_naive();
    let today_start = local_start_of_day(today, &clock.tz)?;
    let last_7_start = local_start_of_day(today - Duration::days(6), &clock.tz)?;
    let last_30_start = local_start_of_day(today - Duration::days(29), &clock.tz)?;

    Ok(vec![
        window_range(clock, "today", "Today", Some(today_start)),
        window_range(clock, "last_7_days", "Last 7 days", Some(last_7_start)),
        window_range(clock, "last_30_days", "Last 30 days", Some(last_30_start)),
        window_range(clock, "all_time", "All time", None),
    ])
}

pub(super) fn chart_start_utc(
    clock: &UsageClock,
    days: i64,
) -> Result<DateTime<Utc>, ReadModelError> {
    let start_date = clock.now_local.date_naive() - Duration::days(days - 1);
    Ok(local_start_of_day(start_date, &clock.tz)?.with_timezone(&Utc))
}

pub(super) fn local_date_label(ts: &DateTime<Utc>, tz: &Tz) -> String {
    ts.with_timezone(tz)
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

pub(super) fn local_chart_dates(
    clock: &UsageClock,
    days: i64,
) -> Result<Vec<String>, ReadModelError> {
    let start_date = clock.now_local.date_naive() - Duration::days(days - 1);
    (0..days)
        .map(|offset| {
            let date = start_date + Duration::days(offset);
            local_start_of_day(date, &clock.tz)?;
            Ok(date.format("%Y-%m-%d").to_string())
        })
        .collect()
}

fn window_range(
    clock: &UsageClock,
    key: &'static str,
    label: &'static str,
    since_local: Option<DateTime<Tz>>,
) -> UsageWindowRange {
    let range_end = clock.now_local.to_rfc3339();
    let range_label = range_label(clock, since_local.as_ref());
    UsageWindowRange {
        key,
        label,
        since_utc: since_local.map(|since| since.with_timezone(&Utc)),
        until_utc: clock.now_utc,
        range_start: since_local.map(|since| since.to_rfc3339()),
        range_end,
        range_label,
    }
}

fn range_label(clock: &UsageClock, since_local: Option<&DateTime<Tz>>) -> String {
    let end_label = clock.now_local.format("%b %-d %H:%M").to_string();
    let Some(since_local) = since_local else {
        return format!(
            "All recorded usage through {end_label} · {}",
            clock.timezone
        );
    };

    if since_local.date_naive() == clock.now_local.date_naive() {
        return format!(
            "{} · {}-{}",
            clock.timezone,
            since_local.format("%b %-d %H:%M"),
            clock.now_local.format("%H:%M")
        );
    }

    format!(
        "{} · {}-{}",
        clock.timezone,
        since_local.format("%b %-d %H:%M"),
        end_label
    )
}

fn local_start_of_day(date: NaiveDate, tz: &Tz) -> Result<DateTime<Tz>, ReadModelError> {
    match tz.with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0) {
        LocalResult::Single(start) => Ok(start),
        LocalResult::Ambiguous(earliest, _) => Ok(earliest),
        LocalResult::None => first_valid_local_instant(date, tz)
            .ok_or_else(|| ReadModelError::InvalidStartOfDay(format!("{date} in {tz}"))),
    }
}

fn first_valid_local_instant(date: NaiveDate, tz: &Tz) -> Option<DateTime<Tz>> {
    // Some zones skip midnight during DST transitions; bucket from the first
    // representable instant still belonging to that local calendar date.
    for minute in 1..(24 * 60) {
        let hour = minute / 60;
        let minute = minute % 60;
        match tz.with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0) {
            LocalResult::Single(start) => return Some(start),
            LocalResult::Ambiguous(earliest, _) => return Some(earliest),
            LocalResult::None => {}
        }
    }
    None
}

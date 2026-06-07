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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum UsageRangeKey {
    Today,
    Last3Days,
    Last7Days,
    Last30Days,
    AllTime,
}

impl UsageRangeKey {
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Today => "today",
            Self::Last3Days => "last_3_days",
            Self::Last7Days => "last_7_days",
            Self::Last30Days => "last_30_days",
            Self::AllTime => "all_time",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Today => "Today",
            Self::Last3Days => "Last 3 days",
            Self::Last7Days => "Last 7 days",
            Self::Last30Days => "Last 30 days",
            Self::AllTime => "All time",
        }
    }
}

pub(super) const DEFAULT_USAGE_RANGE: UsageRangeKey = UsageRangeKey::Last7Days;

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

pub(super) fn resolve_usage_range(
    requested_range: Option<&str>,
) -> (UsageRangeKey, Vec<DashboardDataWarning>) {
    let Some(raw) = requested_range.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return (DEFAULT_USAGE_RANGE, Vec::new());
    };

    match raw {
        "today" => (UsageRangeKey::Today, Vec::new()),
        "last_3_days" => (UsageRangeKey::Last3Days, Vec::new()),
        "last_7_days" => (UsageRangeKey::Last7Days, Vec::new()),
        "last_30_days" => (UsageRangeKey::Last30Days, Vec::new()),
        "all_time" => (UsageRangeKey::AllTime, Vec::new()),
        invalid => (
            DEFAULT_USAGE_RANGE,
            vec![DashboardDataWarning {
                source: "usage.range".to_owned(),
                kind: "invalid_range".to_owned(),
                message: format!("invalid usage range `{invalid}`; falling back to last_7_days"),
            }],
        ),
    }
}

pub(super) fn usage_window_range(
    clock: &UsageClock,
    range: UsageRangeKey,
) -> Result<UsageWindowRange, ReadModelError> {
    let today = clock.now_local.date_naive();
    let since_local = match range {
        UsageRangeKey::Today => Some(local_start_of_day(today, &clock.tz)?),
        UsageRangeKey::Last3Days => Some(local_start_of_day(today - Duration::days(2), &clock.tz)?),
        UsageRangeKey::Last7Days => Some(local_start_of_day(today - Duration::days(6), &clock.tz)?),
        UsageRangeKey::Last30Days => {
            Some(local_start_of_day(today - Duration::days(29), &clock.tz)?)
        }
        UsageRangeKey::AllTime => None,
    };

    Ok(window_range(clock, range.key(), range.label(), since_local))
}

pub(super) fn local_date_start_utc(
    date: NaiveDate,
    tz: &Tz,
) -> Result<DateTime<Utc>, ReadModelError> {
    Ok(local_start_of_day(date, tz)?.with_timezone(&Utc))
}

pub(super) fn local_date_label(ts: &DateTime<Utc>, tz: &Tz) -> String {
    ts.with_timezone(tz)
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

pub(super) fn local_chart_dates_from(
    clock: &UsageClock,
    chart_start_utc: DateTime<Utc>,
) -> Result<Vec<String>, ReadModelError> {
    let start_date = chart_start_utc.with_timezone(&clock.tz).date_naive();
    let end_date = clock.now_local.date_naive();
    let day_count = (end_date - start_date).num_days();
    if day_count < 0 {
        return Ok(Vec::new());
    }

    (0..=day_count)
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

//! Deterministic schedule arithmetic for the resident runner.
//!
//! All wall-clock schedules use a supplied `chrono::TimeZone` so tests can pin
//! DST behaviour. Production callers pass `chrono::Local`. The engine never
//! consults the wall clock itself.

use std::collections::BTreeSet;

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone,
    Timelike, Weekday,
};

use crate::resident::model::ScheduleWhen;

/// Parse an interval used by schedules, pacing, throttles, backoff and
/// retention: one or more digits followed by `ms`, `s`, `m`, `h` or `d`.
pub fn parse_span(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    if number.is_empty() || unit.is_empty() {
        return Err(format!("'{text}' is not a duration such as 30s or 6h"));
    }
    let number: u64 = number
        .parse()
        .map_err(|_| format!("'{text}' is not a duration"))?;
    let millis = match unit {
        "ms" => Some(number),
        "s" => number.checked_mul(1_000),
        "m" => number.checked_mul(60_000),
        "h" => number.checked_mul(3_600_000),
        "d" => number.checked_mul(86_400_000),
        _ => return Err(format!("'{text}' has an unknown duration unit")),
    }
    .ok_or_else(|| format!("'{text}' is too large"))?;
    if millis == 0 {
        return Err(format!("'{text}' must be greater than zero"));
    }
    let millis = i64::try_from(millis).map_err(|_| format!("'{text}' is too large"))?;
    Ok(Duration::milliseconds(millis))
}

/// Parse `HH:MM` or `HH:MM:SS` in local wall-clock time.
pub fn parse_hhmm(text: &str) -> Result<NaiveTime, String> {
    let parts: Vec<&str> = text.trim().split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return Err(format!("'{text}' is not a time such as 02:30"));
    }
    let hour: u32 = parts
        .first()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| format!("'{text}' has an invalid hour"))?;
    let minute: u32 = parts
        .get(1)
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| format!("'{text}' has an invalid minute"))?;
    let second: u32 = match parts.get(2) {
        Some(value) => value
            .parse()
            .map_err(|_| format!("'{text}' has an invalid second"))?,
        None => 0,
    };
    NaiveTime::from_hms_opt(hour, minute, second)
        .ok_or_else(|| format!("'{text}' is outside 00:00:00..=23:59:59"))
}

/// Parse a local `once` timestamp. Offset-qualified RFC 3339 values are
/// rejected because a schedule is defined in the host's local time.
pub fn parse_once(text: &str) -> Result<NaiveDateTime, String> {
    let text = text.trim();
    for format in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(text, format) {
            return Ok(value);
        }
    }
    Err(format!(
        "'{text}' is not a local date-time such as 2026-11-01T09:00"
    ))
}

pub fn parse_weekday(text: &str) -> Result<Weekday, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "mon" | "monday" => Ok(Weekday::Mon),
        "tue" | "tues" | "tuesday" => Ok(Weekday::Tue),
        "wed" | "wednesday" => Ok(Weekday::Wed),
        "thu" | "thur" | "thurs" | "thursday" => Ok(Weekday::Thu),
        "fri" | "friday" => Ok(Weekday::Fri),
        "sat" | "saturday" => Ok(Weekday::Sat),
        "sun" | "sunday" => Ok(Weekday::Sun),
        other => Err(format!("'{other}' is not a weekday")),
    }
}

/// Validate one schedule expression without resolving it to an instant.
pub fn validate_when(when: &ScheduleWhen) -> Result<(), String> {
    match when {
        ScheduleWhen::Once { at } => {
            parse_once(at)?;
        }
        ScheduleWhen::Daily { at } => {
            parse_hhmm(at)?;
        }
        ScheduleWhen::Weekly { days, at } => {
            parse_hhmm(at)?;
            if days.is_empty() {
                return Err("weekly.days must not be empty".to_string());
            }
            for day in days {
                parse_weekday(day)?;
            }
        }
        ScheduleWhen::Interval { every } => {
            parse_span(every)?;
        }
        ScheduleWhen::Cron { expr } => {
            parse_cron(expr)?;
        }
    }
    Ok(())
}

/// The next fire strictly after `after`.
pub fn next_after<Tz: TimeZone>(
    when: &ScheduleWhen,
    after: &DateTime<Tz>,
) -> Result<Option<DateTime<Tz>>, String> {
    match when {
        ScheduleWhen::Once { at } => {
            let naive = parse_once(at)?;
            let tz = after.timezone();
            let Some(candidate) = earliest_local(&tz, naive) else {
                return Ok(None);
            };
            Ok((&candidate > after).then_some(candidate))
        }
        ScheduleWhen::Daily { at } => {
            let time = parse_hhmm(at)?;
            next_by_date(after, |date| Some(date.and_time(time)))
        }
        ScheduleWhen::Weekly { days, at } => {
            let time = parse_hhmm(at)?;
            let mut wanted = BTreeSet::new();
            for day in days {
                wanted.insert(parse_weekday(day)?.num_days_from_monday());
            }
            if wanted.is_empty() {
                return Err("weekly.days must not be empty".to_string());
            }
            next_by_date(after, |date| {
                wanted
                    .contains(&date.weekday().num_days_from_monday())
                    .then(|| date.and_time(time))
            })
        }
        ScheduleWhen::Interval { every } => {
            let gap = parse_span(every)?;
            Ok(Some(after.clone() + gap))
        }
        ScheduleWhen::Cron { expr } => Ok(parse_cron(expr)?.next_after(after)),
    }
}

/// The next `count` fire times, stopping when the expression has no future
/// occurrence (for example a past `once`).
pub fn next_fires<Tz: TimeZone>(
    when: &ScheduleWhen,
    after: &DateTime<Tz>,
    count: usize,
) -> Result<Vec<DateTime<Tz>>, String> {
    let mut fires = Vec::new();
    let mut cursor = after.clone();
    for _ in 0..count.min(100) {
        let Some(next) = next_after(when, &cursor)? else {
            break;
        };
        cursor = next;
        fires.push(cursor.clone());
    }
    Ok(fires)
}

fn next_by_date<Tz, F>(
    after: &DateTime<Tz>,
    mut candidate: F,
) -> Result<Option<DateTime<Tz>>, String>
where
    Tz: TimeZone,
    F: FnMut(NaiveDate) -> Option<NaiveDateTime>,
{
    let tz = after.timezone();
    let mut date = after.date_naive();
    for _ in 0..=3660 {
        if let Some(naive) = candidate(date) {
            // For an ambiguous wall-clock time only the first occurrence on a
            // date is ever used. This keeps a daily 01:30 schedule from firing
            // twice when the clock falls back.
            if let Some(first) = earliest_local(&tz, naive) {
                if &first > after {
                    return Ok(Some(first));
                }
            }
        }
        let Some(next) = date.succ_opt() else {
            return Ok(None);
        };
        date = next;
    }
    Ok(None)
}

fn earliest_local<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> Option<DateTime<Tz>> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(value) => Some(value),
        LocalResult::Ambiguous(first, second) => Some(if first <= second { first } else { second }),
        LocalResult::None => None,
    }
}

/// A parsed five-field cron expression. Supported forms are `*`, `n`, `a-b`,
/// comma lists and `/step` on any of those. Month and weekday names are
/// accepted (`jan`, `mon`). Weekday 0 and 7 both mean Sunday.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronExpr {
    minutes: BTreeSet<u32>,
    hours: BTreeSet<u32>,
    days_of_month: BTreeSet<u32>,
    months: BTreeSet<u32>,
    days_of_week: BTreeSet<u32>,
    dom_restricted: bool,
    dow_restricted: bool,
}

/// Validate and parse a cron expression.
pub fn parse_cron(expr: &str) -> Result<CronExpr, String> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(format!(
            "cron '{expr}' must have five fields: minute hour day-of-month month day-of-week"
        ));
    }
    let (minutes, _) = parse_cron_field(string_at(&fields, 0, expr)?, 0, 59, false, false)?;
    let (hours, _) = parse_cron_field(string_at(&fields, 1, expr)?, 0, 23, false, false)?;
    let (days_of_month, dom_restricted) =
        parse_cron_field(string_at(&fields, 2, expr)?, 1, 31, false, false)?;
    let (months, _) = parse_cron_field(string_at(&fields, 3, expr)?, 1, 12, true, false)?;
    let (days_of_week, dow_restricted) =
        parse_cron_field(string_at(&fields, 4, expr)?, 0, 7, false, true)?;
    if minutes.is_empty() || hours.is_empty() || days_of_month.is_empty() || months.is_empty() {
        return Err(format!("cron '{expr}' has an empty field"));
    }
    Ok(CronExpr {
        minutes,
        hours,
        days_of_month,
        months,
        days_of_week,
        dom_restricted,
        dow_restricted,
    })
}

/// Validate a cron expression and discard the parse result.
pub fn validate_cron(expr: &str) -> Result<(), String> {
    parse_cron(expr).map(|_| ())
}

fn string_at<'a>(fields: &'a [&str], index: usize, expr: &str) -> Result<&'a str, String> {
    fields
        .get(index)
        .copied()
        .ok_or_else(|| format!("cron '{expr}' is missing field {}", index + 1))
}

fn parse_cron_field(
    text: &str,
    min: u32,
    max: u32,
    named_months: bool,
    named_weekdays: bool,
) -> Result<(BTreeSet<u32>, bool), String> {
    let mut values = BTreeSet::new();
    let mut restricted = false;
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("cron field '{text}' contains an empty item"));
        }
        let (base, step) = match part.split_once('/') {
            Some((base, step)) => {
                let step: u32 = step
                    .parse()
                    .map_err(|_| format!("cron step '{step}' is not a number"))?;
                if step == 0 {
                    return Err("cron step must be greater than zero".to_string());
                }
                (base, Some(step))
            }
            None => (part, None),
        };
        let (start, end) = if base == "*" {
            (min, max)
        } else if let Some((start, end)) = base.split_once('-') {
            (
                parse_cron_value(start, min, max, named_months, named_weekdays)?,
                parse_cron_value(end, min, max, named_months, named_weekdays)?,
            )
        } else {
            let value = parse_cron_value(base, min, max, named_months, named_weekdays)?;
            (value, if step.is_some() { max } else { value })
        };
        if start > end {
            return Err(format!("cron range '{base}' runs backwards"));
        }
        if base != "*" || step.is_some() {
            restricted = true;
        }
        let step = step.unwrap_or(1);
        let mut value = start;
        while value <= end {
            let normalized = if named_weekdays && value == 7 {
                0
            } else {
                value
            };
            values.insert(normalized);
            value = match value.checked_add(step) {
                Some(next) => next,
                None => break,
            };
        }
    }
    if values.is_empty() {
        return Err(format!("cron field '{text}' selects no values"));
    }
    Ok((values, restricted))
}

fn parse_cron_value(
    text: &str,
    min: u32,
    max: u32,
    named_months: bool,
    named_weekdays: bool,
) -> Result<u32, String> {
    let lower = text.trim().to_ascii_lowercase();
    let named = if named_months {
        match lower.as_str() {
            "jan" => Some(1),
            "feb" => Some(2),
            "mar" => Some(3),
            "apr" => Some(4),
            "may" => Some(5),
            "jun" => Some(6),
            "jul" => Some(7),
            "aug" => Some(8),
            "sep" => Some(9),
            "oct" => Some(10),
            "nov" => Some(11),
            "dec" => Some(12),
            _ => None,
        }
    } else if named_weekdays {
        match lower.as_str() {
            "sun" => Some(0),
            "mon" => Some(1),
            "tue" => Some(2),
            "wed" => Some(3),
            "thu" => Some(4),
            "fri" => Some(5),
            "sat" => Some(6),
            _ => None,
        }
    } else {
        None
    };
    let value = named
        .or_else(|| lower.parse::<u32>().ok())
        .ok_or_else(|| format!("cron value '{text}' is not a number"))?;
    if !(min..=max).contains(&value) {
        return Err(format!("cron value '{text}' is outside {min}..={max}"));
    }
    Ok(value)
}

impl CronExpr {
    /// Does this expression match the local wall-clock minute?
    #[must_use]
    pub fn matches(&self, value: &NaiveDateTime) -> bool {
        let day_of_month = self.days_of_month.contains(&value.day());
        let day_of_week = self
            .days_of_week
            .contains(&value.weekday().num_days_from_sunday());
        let day = match (self.dom_restricted, self.dow_restricted) {
            (false, false) => true,
            (true, false) => day_of_month,
            (false, true) => day_of_week,
            (true, true) => day_of_month || day_of_week,
        };
        day && self.months.contains(&value.month())
            && self.hours.contains(&value.hour())
            && self.minutes.contains(&value.minute())
    }

    /// The next matching local minute strictly after `after`.
    ///
    /// Iteration is over local wall-clock minutes, not UTC instants. That
    /// makes a fall-back hour fire once and a spring-forward gap simply not
    /// exist, instead of accidentally repeating or skipping resolved instants.
    #[must_use]
    pub fn next_after<Tz: TimeZone>(&self, after: &DateTime<Tz>) -> Option<DateTime<Tz>> {
        let tz = after.timezone();
        let start = after.naive_local().with_second(0)?.with_nanosecond(0)? + Duration::minutes(1);
        let limit = start + Duration::days(366 * 5);
        let mut local = start;
        while local <= limit {
            if self.matches(&local) {
                match tz.from_local_datetime(&local) {
                    LocalResult::Single(value) if &value > after => return Some(value),
                    LocalResult::Ambiguous(first, second) => {
                        let first = if first <= second { first } else { second };
                        if &first > after {
                            return Some(first);
                        }
                    }
                    LocalResult::Single(_) | LocalResult::None => {}
                }
            }
            local += Duration::minutes(1);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::America::New_York;
    use chrono_tz::Tz;

    fn at(tz: Tz, year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Tz> {
        tz.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .earliest()
            .expect("local time")
    }

    fn when(kind: &str) -> ScheduleWhen {
        serde_yaml_ng::from_str(kind).expect("when")
    }

    #[test]
    fn daily_and_weekly_choose_the_next_wall_clock_occurrence() {
        let base = at(New_York, 2026, 11, 4, 12, 0);
        let next = next_after(&when("kind: daily\nat: '02:30'"), &base)
            .expect("next")
            .expect("some");
        assert_eq!(next, at(New_York, 2026, 11, 5, 2, 30));

        let next = next_after(&when("kind: weekly\ndays: [mon, thu]\nat: '08:00'"), &base)
            .expect("next")
            .expect("some");
        assert_eq!(next, at(New_York, 2026, 11, 5, 8, 0));
    }

    #[test]
    fn once_and_interval_are_strictly_after() {
        let base = at(New_York, 2026, 11, 4, 12, 0);
        assert!(
            next_after(&when("kind: once\nat: '2026-11-04T11:00'"), &base)
                .expect("next")
                .is_none()
        );
        let next = next_after(&when("kind: once\nat: '2026-11-04T13:00'"), &base)
            .expect("next")
            .expect("some");
        assert_eq!(next, at(New_York, 2026, 11, 4, 13, 0));
        let next = next_after(&when("kind: interval\nevery: 6h"), &base)
            .expect("next")
            .expect("some");
        assert_eq!(next, at(New_York, 2026, 11, 4, 18, 0));
    }

    #[test]
    fn cron_supports_ranges_lists_and_names() {
        let base = at(New_York, 2026, 11, 4, 12, 0);
        let next = next_after(&when("kind: cron\nexpr: '0 2 * * mon-fri'"), &base)
            .expect("next")
            .expect("some");
        assert_eq!(next, at(New_York, 2026, 11, 5, 2, 0));

        let expression = parse_cron("*/15 9-17 * jan,dec sun").expect("cron");
        assert!(expression.matches(
            &NaiveDate::from_ymd_opt(2026, 12, 6)
                .expect("date")
                .and_hms_opt(9, 30, 0)
                .expect("time")
        ));
    }

    #[test]
    fn spring_forward_skips_a_nonexistent_wall_clock_time() {
        // 2026-03-08 02:30 does not exist in New York.
        let base = at(New_York, 2026, 3, 8, 0, 0);
        let next = next_after(&when("kind: daily\nat: '02:30'"), &base)
            .expect("next")
            .expect("some");
        assert_eq!(next, at(New_York, 2026, 3, 9, 2, 30));
    }

    #[test]
    fn fall_back_fires_once_at_the_first_occurrence() {
        let base = at(New_York, 2026, 11, 1, 0, 0);
        let daily = when("kind: daily\nat: '01:30'");
        let first = next_after(&daily, &base).expect("next").expect("some");
        assert_eq!(first, at(New_York, 2026, 11, 1, 1, 30));
        let second = next_after(&daily, &first).expect("next").expect("some");
        assert_eq!(second, at(New_York, 2026, 11, 2, 1, 30));
    }

    #[test]
    fn cron_does_not_repeat_the_ambiguous_hour() {
        let base = at(New_York, 2026, 11, 1, 0, 0);
        let expression = when("kind: cron\nexpr: '30 1 * * *'");
        let first = next_after(&expression, &base).expect("next").expect("some");
        assert_eq!(first, at(New_York, 2026, 11, 1, 1, 30));
        let second = next_after(&expression, &first)
            .expect("next")
            .expect("some");
        assert_eq!(second, at(New_York, 2026, 11, 2, 1, 30));
    }

    #[test]
    fn next_fires_is_bounded_and_stops_at_the_end() {
        let base = at(New_York, 2026, 11, 4, 12, 0);
        let fires =
            next_fires(&when("kind: once\nat: '2026-11-04T13:00'"), &base, 5).expect("fires");
        assert_eq!(fires.len(), 1);
    }

    #[test]
    fn invalid_expressions_are_rejected() {
        assert!(parse_span("0s").is_err());
        assert!(parse_span("6 hours").is_err());
        assert!(parse_hhmm("24:00").is_err());
        assert!(parse_cron("0 2 * *").is_err());
        assert!(parse_cron("60 * * * *").is_err());
        assert!(parse_cron("0 2 * * 1-9").is_err());
    }
}

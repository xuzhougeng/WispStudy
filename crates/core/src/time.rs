//! Timestamps: stored as RFC 3339 UTC with milliseconds, shown in local time.

use chrono::{DateTime, Datelike, Local, SecondsFormat, Utc};

pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn now_millis() -> i64 {
    Utc::now().timestamp_millis()
}

/// The year a moment falls in on this machine's calendar: what 「近 3 年」 counts back from (`filter::year_span`).
pub fn year_of(millis: i64) -> i32 {
    DateTime::from_timestamp_millis(millis).map_or(1970, |moment| moment.with_timezone(&Local).year())
}

fn parse(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim()).ok().map(|d| d.with_timezone(&Utc))
}

/// Today on this machine's calendar, `2026-11-11`: a day of use (`settings::note_use`).
pub fn today() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// `2026.11.11` in local time; 日期待补 when the value is not a timestamp.
pub fn date_text(value: &str) -> String {
    match parse(value) {
        Some(d) => d.with_timezone(&Local).format("%Y.%m.%d").to_string(),
        None => "日期待补".into(),
    }
}

/// `2026.11.11` in local time from milliseconds since the epoch; empty when out of range.
pub fn millis_date(millis: i64) -> String {
    DateTime::from_timestamp_millis(millis).map(|d| d.with_timezone(&Local).format("%Y.%m.%d").to_string()).unwrap_or_default()
}

/// `11-11 09:30` in local time.
pub fn short_time(value: &str) -> String {
    parse(value).map(|d| d.with_timezone(&Local).format("%m-%d %H:%M").to_string()).unwrap_or_default()
}

/// `09:30` in local time: under a heading that already says the day.
pub fn clock_time(value: &str) -> String {
    parse(value).map(|d| d.with_timezone(&Local).format("%H:%M").to_string()).unwrap_or_default()
}

/// `now`, or a millisecond after `last` when `now` is not later: a moment that comes after the one before
/// though both fell in the same millisecond, so an order kept by time has no ties.
pub fn later_than(now: &str, last: &str) -> String {
    let before = millis(last);
    if millis(now) > before { return now.to_string() }
    DateTime::from_timestamp_millis(before + 1).map_or_else(|| now.to_string(), |d| d.to_rfc3339_opts(SecondsFormat::Millis, true))
}

/// Milliseconds since the epoch, 0 when unparsable.
pub fn millis(value: &str) -> i64 {
    parse(value).map(|d| d.timestamp_millis()).unwrap_or(0)
}

/// How long ago something was, by calendar day: what 历史 groups its conversations under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ago {
    Today,
    Yesterday,
    /// Two to six days before today: with today, the last seven days.
    Week,
    Earlier,
}

impl Ago {
    /// The heading, a key for `i18n::t`.
    pub fn label(self) -> &'static str {
        match self {
            Ago::Today => "今天",
            Ago::Yesterday => "昨天",
            Ago::Week => "近 7 天",
            Ago::Earlier => "更早",
        }
    }
}

/// Which day `value` falls on, counted back from `now`'s date in `now`'s time zone. A time after
/// `now` (another machine's clock) is today; one that is not a timestamp is earlier.
pub fn ago<Tz: chrono::TimeZone>(value: &str, now: &DateTime<Tz>) -> Ago {
    let Some(then) = parse(value) else { return Ago::Earlier };
    let days = (now.date_naive() - then.with_timezone(&now.timezone()).date_naive()).num_days();
    match days {
        ..=0 => Ago::Today,
        1 => Ago::Yesterday,
        2..=6 => Ago::Week,
        _ => Ago::Earlier,
    }
}

/// `ago` counted from this moment, on this machine's calendar.
pub fn ago_today(value: &str) -> Ago {
    ago(value, &Local::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_after_the_last_one() {
        assert_eq!(later_than("2026-11-28T06:45:21.124Z", "2026-11-28T06:45:21.123Z"), "2026-11-28T06:45:21.124Z");
        assert_eq!(later_than("2026-11-28T06:45:21.123Z", "2026-11-28T06:45:21.123Z"), "2026-11-28T06:45:21.124Z");
        assert_eq!(later_than("2026-11-28T06:45:21.100Z", "2026-11-28T06:45:21.999Z"), "2026-11-28T06:45:22.000Z");
        // Nothing before: the moment itself.
        assert_eq!(later_than("2026-11-28T06:45:21.123Z", ""), "2026-11-28T06:45:21.123Z");
    }

    #[test]
    fn days_ago_follow_the_local_calendar() {
        use chrono::FixedOffset;
        // 00:30 on 22 November in Beijing is still 21 November in UTC.
        let beijing = FixedOffset::east_opt(8 * 3600).unwrap();
        let now = DateTime::parse_from_rfc3339("2026-11-22T00:30:00+08:00").unwrap().with_timezone(&beijing);
        assert_eq!(ago("2026-11-21T16:10:00.000Z", &now), Ago::Today);
        assert_eq!(ago("2026-11-21T15:59:00.000Z", &now), Ago::Yesterday);
        assert_eq!(ago("2026-11-20T16:00:00.000Z", &now), Ago::Yesterday);
        assert_eq!(ago("2026-11-19T12:00:00.000Z", &now), Ago::Week);
        assert_eq!(ago("2026-11-15T16:00:00.000Z", &now), Ago::Week);
        assert_eq!(ago("2026-11-15T15:00:00.000Z", &now), Ago::Earlier);
        assert_eq!(ago("2025-01-01T00:00:00.000Z", &now), Ago::Earlier);
        // Ahead of the clock, or not a time at all.
        assert_eq!(ago("2026-11-23T09:00:00.000Z", &now), Ago::Today);
        assert_eq!(ago("", &now), Ago::Earlier);
        // The same moment is another day in another zone.
        let utc = now.with_timezone(&Utc);
        assert_eq!(ago("2026-11-21T15:59:00.000Z", &utc), Ago::Today);
    }

    #[test]
    fn round_trips() {
        let now = now_iso();
        assert!(millis(&now) > 0);
        assert_eq!(date_text("nope"), "日期待补");
        assert_eq!(date_text(&now).len(), 10);
    }
}

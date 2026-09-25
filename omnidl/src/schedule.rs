//! Start times for "later" downloads, in local time.

use chrono::{DateTime, Duration, Local, NaiveDateTime, NaiveTime, TimeZone};

/// Hour of the "tonight" preset, when lines are quiet.
pub const NIGHT_HOUR: u32 = 2;

pub fn now_unix() -> i64 {
    Local::now().timestamp()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    In30Min,
    In1Hour,
    Tonight,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::In30Min, Preset::In1Hour, Preset::Tonight];

    pub fn label(self) -> String {
        match self {
            Preset::In30Min => "In 30 Minuten".into(),
            Preset::In1Hour => "In einer Stunde".into(),
            Preset::Tonight => format!("Heute Nacht um {NIGHT_HOUR}:00"),
        }
    }

    pub fn start(self, now: NaiveDateTime) -> NaiveDateTime {
        match self {
            Preset::In30Min => now + Duration::minutes(30),
            Preset::In1Hour => now + Duration::hours(1),
            Preset::Tonight => next_at(now, NIGHT_HOUR, 0),
        }
    }
}

/// The next time the clock shows `hour:minute`, today or tomorrow.
pub fn next_at(now: NaiveDateTime, hour: u32, minute: u32) -> NaiveDateTime {
    let time = NaiveTime::from_hms_opt(hour.min(23), minute.min(59), 0).unwrap_or(NaiveTime::MIN);
    let today = now.date().and_time(time);
    if today > now { today } else { today + Duration::days(1) }
}

/// Local wall-clock time to Unix seconds. In the hour skipped by daylight
/// saving the time does not exist; an hour later is close enough.
pub fn to_unix(t: NaiveDateTime) -> i64 {
    match Local.from_local_datetime(&t) {
        chrono::LocalResult::Single(d) => d.timestamp(),
        chrono::LocalResult::Ambiguous(first, _) => first.timestamp(),
        chrono::LocalResult::None => Local
            .from_local_datetime(&(t + Duration::hours(1)))
            .earliest()
            .map_or(now_unix(), |d| d.timestamp()),
    }
}

pub fn local(unix: i64) -> NaiveDateTime {
    DateTime::from_timestamp(unix, 0)
        .map(|d| d.with_timezone(&Local).naive_local())
        .unwrap_or_default()
}

pub fn local_now() -> NaiveDateTime {
    Local::now().naive_local()
}

/// Starting point for a custom time: the next full hour.
pub fn next_full_hour(now: NaiveDateTime) -> u32 {
    (chrono::Timelike::hour(&now) + 1) % 24
}

/// "heute um 2:00", "morgen um 23:30", "am 24.09. um 8:15", "am 01.01.2030 um 1:00".
pub fn describe(at: NaiveDateTime, now: NaiveDateTime) -> String {
    let time = format!("{}", at.format("%-H:%M"));
    let day = at.date();
    let today = now.date();
    if day == today {
        format!("heute um {time}")
    } else if Some(day) == today.succ_opt() {
        format!("morgen um {time}")
    } else if chrono::Datelike::year(&day) == chrono::Datelike::year(&today) {
        format!("am {} um {time}", day.format("%d.%m."))
    } else {
        format!("am {} um {time}", day.format("%d.%m.%Y"))
    }
}

/// Start time of a scheduled job for the status line.
pub fn describe_unix(unix: i64) -> String {
    describe(local(unix), local_now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn date(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d).unwrap().and_hms_opt(h, min, 0).unwrap()
    }

    #[test]
    fn next_at_rolls_over_to_tomorrow() {
        let evening = date(2026, 9, 23, 21, 15);
        assert_eq!(next_at(evening, 2, 0), date(2026, 9, 24, 2, 0));
        assert_eq!(next_at(evening, 22, 30), date(2026, 9, 23, 22, 30));
        // exactly now is not in the future
        assert_eq!(next_at(evening, 21, 15), date(2026, 9, 24, 21, 15));
        let small_hours = date(2026, 9, 23, 0, 40);
        assert_eq!(next_at(small_hours, 2, 0), date(2026, 9, 23, 2, 0), "nach Mitternacht: noch heute");
    }

    #[test]
    fn presets() {
        let now = date(2026, 12, 31, 23, 45);
        assert_eq!(Preset::In30Min.start(now), date(2027, 1, 1, 0, 15));
        assert_eq!(Preset::In1Hour.start(now), date(2027, 1, 1, 0, 45));
        assert_eq!(Preset::Tonight.start(now), date(2027, 1, 1, 2, 0));
        assert_eq!(Preset::Tonight.label(), "Heute Nacht um 2:00");
    }

    #[test]
    fn descriptions_read_naturally() {
        let now = date(2026, 9, 23, 21, 15);
        assert_eq!(describe(date(2026, 9, 23, 23, 5), now), "heute um 23:05");
        assert_eq!(describe(date(2026, 9, 24, 2, 0), now), "morgen um 2:00");
        assert_eq!(describe(date(2026, 9, 26, 8, 15), now), "am 26.09. um 8:15");
        assert_eq!(describe(date(2030, 1, 1, 1, 0), now), "am 01.01.2030 um 1:00");
        let silvester = date(2026, 12, 31, 22, 0);
        assert_eq!(describe(date(2027, 1, 1, 2, 0), silvester), "morgen um 2:00", "Jahreswechsel");
    }

    #[test]
    fn unix_roundtrip_in_local_time() {
        let t = date(2026, 9, 24, 2, 0);
        assert_eq!(local(to_unix(t)), t);
    }
}

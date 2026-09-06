use jiff::{Timestamp, civil::Date};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BillingCycle {
    pub start_utc: i64,
    pub end_utc: i64,
}

pub fn day_start_utc(timestamp: i64, timezone: &str) -> Result<i64, jiff::Error> {
    Timestamp::from_second(timestamp)?
        .in_tz(timezone)?
        .start_of_day()
        .map(|start| start.timestamp().as_second())
}

pub fn previous_day_start_utc(timestamp: i64, timezone: &str) -> Result<i64, jiff::Error> {
    Timestamp::from_second(timestamp)?
        .in_tz(timezone)?
        .start_of_day()?
        .yesterday()
        .map(|start| start.timestamp().as_second())
}

pub fn billing_cycle(
    timestamp: i64,
    timezone: &str,
    reset_day: i64,
) -> Result<BillingCycle, jiff::Error> {
    if !(1..=31).contains(&reset_day) {
        return Err(jiff::Error::from_args(format_args!(
            "billing reset day must be between 1 and 31"
        )));
    }

    let local = Timestamp::from_second(timestamp)?.in_tz(timezone)?;
    let date = local.date();
    let current_month = (date.year(), date.month());
    let current_boundary = boundary_date(current_month, reset_day)?;
    let (start_month, end_month) = if date < current_boundary {
        (previous_month(current_month), current_month)
    } else {
        (current_month, next_month(current_month))
    };
    let zone = local.time_zone().clone();
    let start = boundary_date(start_month, reset_day)?
        .at(0, 0, 0, 0)
        .to_zoned(zone.clone())?
        .timestamp()
        .as_second();
    let end = boundary_date(end_month, reset_day)?
        .at(0, 0, 0, 0)
        .to_zoned(zone)?
        .timestamp()
        .as_second();
    Ok(BillingCycle {
        start_utc: start,
        end_utc: end,
    })
}

fn boundary_date((year, month): (i16, i8), reset_day: i64) -> Result<Date, jiff::Error> {
    let first = Date::new(year, month, 1)?;
    let day = reset_day.min(i64::from(first.days_in_month())) as i8;
    Date::new(year, month, day)
}

fn previous_month((year, month): (i16, i8)) -> (i16, i8) {
    if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    }
}

fn next_month((year, month): (i16, i8)) -> (i16, i8) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn second(value: &str) -> i64 {
        value
            .parse::<Timestamp>()
            .expect("valid timestamp")
            .as_second()
    }

    #[test]
    fn natural_days_use_site_timezone_and_dst() {
        assert_eq!(
            day_start_utc(second("2026-09-06T15:59:59Z"), "Asia/Shanghai")
                .expect("Shanghai day start"),
            second("2026-09-05T16:00:00Z")
        );
        assert_eq!(
            day_start_utc(second("2026-09-06T16:00:00Z"), "Asia/Shanghai")
                .expect("Shanghai next day start"),
            second("2026-09-06T16:00:00Z")
        );

        let before_dst = day_start_utc(second("2026-03-08T16:00:00Z"), "America/New_York")
            .expect("DST transition day");
        let after_dst = day_start_utc(second("2026-03-09T16:00:00Z"), "America/New_York")
            .expect("day after DST transition");
        assert_eq!(before_dst, second("2026-03-08T05:00:00Z"));
        assert_eq!(after_dst, second("2026-03-09T04:00:00Z"));
        assert_eq!(after_dst - before_dst, 23 * 60 * 60);
    }

    #[test]
    fn previous_natural_day_uses_local_calendar_across_dst() {
        assert_eq!(
            previous_day_start_utc(second("2026-09-06T18:00:00Z"), "Asia/Shanghai")
                .expect("previous Shanghai day"),
            second("2026-09-05T16:00:00Z")
        );

        let current = day_start_utc(second("2026-03-09T16:00:00Z"), "America/New_York")
            .expect("current New York day");
        let previous = previous_day_start_utc(second("2026-03-09T16:00:00Z"), "America/New_York")
            .expect("previous New York day");
        assert_eq!(current, second("2026-03-09T04:00:00Z"));
        assert_eq!(previous, second("2026-03-08T05:00:00Z"));
        assert_eq!(current - previous, 23 * 60 * 60);
    }

    #[test]
    fn billing_boundaries_cover_reset_days_and_calendar_edges() {
        let cases = [
            (
                "2026-02-20T12:00:00Z",
                1,
                "2026-02-01T00:00:00Z",
                "2026-03-01T00:00:00Z",
            ),
            (
                "2026-02-20T12:00:00Z",
                15,
                "2026-02-15T00:00:00Z",
                "2026-03-15T00:00:00Z",
            ),
            (
                "2026-02-28T12:00:00Z",
                28,
                "2026-02-28T00:00:00Z",
                "2026-03-28T00:00:00Z",
            ),
            (
                "2024-02-29T12:00:00Z",
                29,
                "2024-02-29T00:00:00Z",
                "2024-03-29T00:00:00Z",
            ),
            (
                "2026-02-28T12:00:00Z",
                30,
                "2026-02-28T00:00:00Z",
                "2026-03-30T00:00:00Z",
            ),
            (
                "2026-02-28T12:00:00Z",
                31,
                "2026-02-28T00:00:00Z",
                "2026-03-31T00:00:00Z",
            ),
            (
                "2026-01-15T12:00:00Z",
                31,
                "2025-12-31T00:00:00Z",
                "2026-01-31T00:00:00Z",
            ),
        ];
        for (now, reset, expected_start, expected_end) in cases {
            let cycle = billing_cycle(second(now), "UTC", reset).expect("billing cycle");
            assert_eq!(
                cycle.start_utc,
                second(expected_start),
                "start for reset {reset}"
            );
            assert_eq!(cycle.end_utc, second(expected_end), "end for reset {reset}");
        }
    }

    #[test]
    fn billing_boundaries_are_local_midnights_across_dst() {
        let cycle = billing_cycle(second("2026-03-20T12:00:00Z"), "America/New_York", 15)
            .expect("New York cycle");
        assert_eq!(cycle.start_utc, second("2026-03-15T04:00:00Z"));
        assert_eq!(cycle.end_utc, second("2026-04-15T04:00:00Z"));
    }
}

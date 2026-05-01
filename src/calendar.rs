//! Thin wrapper over `chrono` for date/time/period arithmetic. Keeps
//! the rest of the engine free of `chrono::*` types.

use chrono::{Datelike, Days, Months, NaiveDate, NaiveTime, Utc};

use crate::error::Error;
use crate::unit::Unit;
use crate::value::{InstantValue, LinearValue, PeriodValue, Value};

pub fn make_instant(year: i32, month: u32, day: u32) -> Result<InstantValue, Error> {
    let date = NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| {
        Error::TimeArithmetic(format!("invalid date: {year:04}-{month:02}-{day:02}"))
    })?;
    Ok(InstantValue { timestamp: date.and_time(NaiveTime::MIN) })
}

pub fn instant_plus_duration(i: InstantValue, l: LinearValue) -> Result<InstantValue, Error> {
    require_seconds(&l)?;
    let nanos = (l.mag * 1e9) as i64;
    let new_ts = i
        .timestamp
        .checked_add_signed(chrono::Duration::nanoseconds(nanos))
        .ok_or_else(|| Error::TimeArithmetic("date arithmetic overflow".into()))?;
    Ok(InstantValue { timestamp: new_ts })
}

pub fn instant_minus_duration(i: InstantValue, l: LinearValue) -> Result<InstantValue, Error> {
    require_seconds(&l)?;
    let nanos = (l.mag * 1e9) as i64;
    let new_ts = i
        .timestamp
        .checked_sub_signed(chrono::Duration::nanoseconds(nanos))
        .ok_or_else(|| Error::TimeArithmetic("date arithmetic overflow".into()))?;
    Ok(InstantValue { timestamp: new_ts })
}

/// Per the v2 spec, `Instant - Instant` returns a Period — the calendar
/// decomposition of the difference into (years, months, days). No
/// normalization: components can be negative and don't borrow across
/// boundaries, so the result is fully determined by component-wise
/// subtraction.
///
/// Round-trip note: `a + (a - b)` is NOT guaranteed to equal `a` in general
/// because Period application clamps end-of-month overflow (Jan 31 + 1mo →
/// Feb 28). The spec accepts this non-invertibility.
pub fn instant_minus_instant(a: InstantValue, b: InstantValue) -> Value {
    let years = a.timestamp.year() - b.timestamp.year();
    let months = a.timestamp.month() as i32 - b.timestamp.month() as i32;
    let days = a.timestamp.day() as i32 - b.timestamp.day() as i32;
    Value::Period(PeriodValue { years, months, days })
}

/// Used by `as_duration` to compute the *exact seconds* between two
/// instants (not the Period decomposition). Returns a Linear value
/// with `unit = SECONDS`.
fn instant_diff_seconds(a: InstantValue, b: InstantValue) -> LinearValue {
    let diff = a.timestamp - b.timestamp;
    let secs = diff.num_seconds() as f64 + (diff.subsec_nanos() as f64) / 1e9;
    LinearValue::new(secs, Unit::SECONDS)
}

// ─── v2 builtins ───────────────────────────────────────────────────────────

pub fn now() -> InstantValue {
    InstantValue { timestamp: Utc::now().naive_utc() }
}

pub fn today() -> InstantValue {
    let date = Utc::now().naive_utc().date();
    InstantValue { timestamp: date.and_time(NaiveTime::MIN) }
}

/// `as_duration(period, anchor)` — apply the period to the anchor instant
/// and return the seconds between the two instants. Anchor is required
/// because months/years have varying lengths.
pub fn as_duration(p: PeriodValue, anchor: InstantValue) -> Result<LinearValue, Error> {
    let after = apply_period(anchor.clone(), p, 1)?;
    Ok(instant_diff_seconds(after, anchor))
}

/// `as_period(duration, anchor)` — apply the linear duration to the
/// anchor and return the calendar decomposition of the resulting span.
/// Anchor is required because the same number of seconds spans different
/// numbers of calendar months/days depending on where it starts.
pub fn as_period(l: LinearValue, anchor: InstantValue) -> Result<PeriodValue, Error> {
    require_seconds(&l)?;
    let after = instant_plus_duration(anchor.clone(), l)?;
    let years = after.timestamp.year() - anchor.timestamp.year();
    let months = after.timestamp.month() as i32 - anchor.timestamp.month() as i32;
    let days = after.timestamp.day() as i32 - anchor.timestamp.day() as i32;
    Ok(PeriodValue { years, months, days })
}

pub fn instant_plus_period(i: InstantValue, p: PeriodValue) -> Result<InstantValue, Error> {
    apply_period(i, p, /* sign */ 1)
}

pub fn instant_minus_period(i: InstantValue, p: PeriodValue) -> Result<InstantValue, Error> {
    apply_period(i, p, -1)
}

/// Apply Period to an Instant in step order (years, months, then days)
/// per the spec. Day-overflow uses chrono's `checked_add_months`, which
/// implements the spec's clamp policy: Jan 31 + 1 month → Feb 28.
fn apply_period(i: InstantValue, p: PeriodValue, sign: i32) -> Result<InstantValue, Error> {
    let mut date = i.timestamp.date();
    let total_months: i64 = (p.years as i64) * 12 + (p.months as i64);
    let signed_months = total_months * (sign as i64);
    if signed_months > 0 {
        date = date
            .checked_add_months(Months::new(signed_months as u32))
            .ok_or_else(|| Error::TimeArithmetic("date arithmetic overflow".into()))?;
    } else if signed_months < 0 {
        date = date
            .checked_sub_months(Months::new((-signed_months) as u32))
            .ok_or_else(|| Error::TimeArithmetic("date arithmetic overflow".into()))?;
    }
    let signed_days = (p.days as i64) * (sign as i64);
    if signed_days > 0 {
        date = date
            .checked_add_days(Days::new(signed_days as u64))
            .ok_or_else(|| Error::TimeArithmetic("date arithmetic overflow".into()))?;
    } else if signed_days < 0 {
        date = date
            .checked_sub_days(Days::new((-signed_days) as u64))
            .ok_or_else(|| Error::TimeArithmetic("date arithmetic overflow".into()))?;
    }
    Ok(InstantValue { timestamp: date.and_time(i.timestamp.time()) })
}

fn require_seconds(l: &LinearValue) -> Result<(), Error> {
    if l.unit != Unit::SECONDS {
        return Err(Error::TimeArithmetic(format!(
            "instant arithmetic requires a duration (seconds-dim), got unit {}",
            if l.unit.is_dimensionless() { "dimensionless".to_string() } else { l.unit.render() }
        )));
    }
    if l.is_absolute_temp {
        // Defensive: this would mean someone added an absolute temperature
        // (display=K, but is_absolute_temp=true) to an instant. The flag
        // prevents temperature math from leaking into instant math.
        return Err(Error::TimeArithmetic(
            "cannot apply an absolute temperature to an instant".into(),
        ));
    }
    Ok(())
}

pub fn format_instant(i: &InstantValue) -> String {
    if i.timestamp.time() == NaiveTime::MIN {
        i.timestamp.format("%Y-%m-%d").to_string()
    } else {
        i.timestamp.format("%Y-%m-%dT%H:%M:%SZ").to_string()
    }
}

pub fn format_period(p: &PeriodValue) -> String {
    if p.years == 0 && p.months == 0 && p.days == 0 {
        return "P0D".to_string();
    }
    let mut s = String::from("P");
    if p.years != 0 {
        s.push_str(&format!("{}Y", p.years));
    }
    if p.months != 0 {
        s.push_str(&format!("{}M", p.months));
    }
    if p.days != 0 {
        s.push_str(&format!("{}D", p.days));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jan31_plus_1_month_clamps_feb28() {
        let i = make_instant(2026, 1, 31).unwrap();
        let r = instant_plus_period(i, PeriodValue { years: 0, months: 1, days: 0 }).unwrap();
        assert_eq!(format_instant(&r), "2026-02-28");
    }

    #[test]
    fn mar31_minus_1_month_clamps_feb28() {
        let i = make_instant(2026, 3, 31).unwrap();
        let r = instant_minus_period(i, PeriodValue { years: 0, months: 1, days: 0 }).unwrap();
        assert_eq!(format_instant(&r), "2026-02-28");
    }

    #[test]
    fn period_render_iso() {
        assert_eq!(format_period(&PeriodValue { years: 1, months: 2, days: 3 }), "P1Y2M3D");
        assert_eq!(format_period(&PeriodValue { years: 1, months: 0, days: 0 }), "P1Y");
        assert_eq!(format_period(&PeriodValue { years: 0, months: 0, days: 0 }), "P0D");
        assert_eq!(format_period(&PeriodValue { years: 0, months: -1, days: 1 }), "P-1M1D");
    }
}

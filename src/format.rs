use crate::ast::UnitExpr;
use crate::calendar;
use crate::eval::Answer;
use crate::unit::{AtomKind, UnitRegistry};
use crate::value::{LinearValue, Value};

pub fn format(answer: &Answer) -> String {
    match answer {
        Answer::Bare(v) => format_value(v),
        Answer::Assigned { name, value } => format!("{name} = {}", format_value(value)),
    }
}

pub fn format_value(v: &Value) -> String {
    match v {
        Value::Linear(l) => format_linear(l),
        Value::Instant(i) => calendar::format_instant(i),
        Value::Period(p) => calendar::format_period(p),
    }
}

fn format_linear(v: &LinearValue) -> String {
    // Pure-time formatting. Temperature values have `unit == K`, so this
    // branch doesn't fire for them.
    if v.unit.is_pure_time() && v.mag.is_finite() {
        if v.force_hms {
            return format_hms(v.mag);
        }
        let display_overrides_hms = v.display_explicit
            || v.display
                .as_ref()
                .map(is_explicit_time_unit)
                .unwrap_or(false);
        if !display_overrides_hms {
            return format_hms(v.mag);
        }
    }

    // Temperature rendering — branches ONLY on `is_absolute_temp` and
    // `render_as_delta`. Never on `unit` or atom names.
    if v.is_absolute_temp || v.render_as_delta {
        if let Some(s) = render_temperature(v) {
            return s;
        }
    }

    if let Some(rendered) = render_with_display(v) {
        return rendered;
    }
    let mag = trim_float(v.mag);
    if v.unit.is_dimensionless() {
        mag
    } else {
        format!("{mag} {}", v.unit.render())
    }
}

fn is_explicit_time_unit(u: &UnitExpr) -> bool {
    matches!(
        u,
        UnitExpr::Atom(name, 1)
            if matches!(
                name.as_str(),
                "min" | "minute" | "minutes" | "mins"
                    | "hour" | "hours" | "hr" | "h"
                    | "day" | "days" | "d"
                    | "week" | "weeks"
                    | "year" | "years" | "yr"
            )
    )
}

fn render_temperature(v: &LinearValue) -> Option<String> {
    let display = v.display.as_ref()?;
    let registry = UnitRegistry::standard();
    let r = registry.resolve(display).ok()?;
    if r.unit != v.unit {
        return None;
    }
    if r.factor == 0.0 || !r.factor.is_finite() {
        return None;
    }
    let atom = r.atom;
    let is_affine = atom.map(|a| matches!(a.kind, AtomKind::AffineTemp)).unwrap_or(false);

    if v.is_absolute_temp {
        let offset = if is_affine { atom.unwrap().offset } else { 0.0 };
        let display_mag = (v.mag - offset) / r.factor;
        let unit_str = render_unit_expr(display);
        return Some(format_with_unit(display_mag, &unit_str));
    }

    if v.render_as_delta {
        let display_mag = v.mag / r.factor;
        let unit_str = render_unit_expr(display);
        if is_affine {
            return Some(format!("{} Δ{}", trim_float(display_mag), unit_str));
        }
        return Some(format_with_unit(display_mag, &unit_str));
    }

    None
}

fn format_with_unit(mag: f64, unit_str: &str) -> String {
    let m = trim_float(mag);
    if unit_str.is_empty() {
        m
    } else {
        format!("{m} {unit_str}")
    }
}

fn render_with_display(v: &LinearValue) -> Option<String> {
    let display = v.display.as_ref()?;
    let registry = UnitRegistry::standard();
    let r = registry.resolve(display).ok()?;
    if r.unit != v.unit {
        return None;
    }
    if r.factor == 0.0 || !r.factor.is_finite() {
        return None;
    }
    let mag = trim_float(v.mag / r.factor);
    let unit_str = render_unit_expr(display);
    if unit_str.is_empty() {
        Some(mag)
    } else {
        Some(format!("{mag} {unit_str}"))
    }
}

fn render_unit_expr(u: &UnitExpr) -> String {
    match u {
        UnitExpr::Atom(name, exp) => match *exp {
            0 => String::new(),
            1 => name.clone(),
            n => format!("{name}^{n}"),
        },
        UnitExpr::Mul(a, b) => {
            let ra = render_unit_expr(a);
            let rb = render_unit_expr(b);
            match (ra.is_empty(), rb.is_empty()) {
                (true, _) => rb,
                (_, true) => ra,
                _ => format!("{ra}*{rb}"),
            }
        }
        UnitExpr::Div(a, b) => {
            let ra = render_unit_expr(a);
            let rb = render_unit_expr(b);
            match (ra.is_empty(), rb.is_empty()) {
                (true, true) => String::new(),
                (true, _) => format!("1/{rb}"),
                (_, true) => ra,
                _ => format!("{ra}/{rb}"),
            }
        }
    }
}

fn format_hms(seconds: f64) -> String {
    let neg = seconds < 0.0;
    let s = seconds.abs();
    let total_secs = s;
    let prefix = if neg { "-" } else { "" };

    if total_secs < 60.0 {
        format!("{prefix}{total_secs:.2}")
    } else if total_secs < 3600.0 {
        let mm = (total_secs / 60.0).floor() as u64;
        let ss = total_secs - (mm as f64) * 60.0;
        format!("{prefix}{mm:02}:{ss:05.2}")
    } else {
        let hh = (total_secs / 3600.0).floor() as u64;
        let rem = total_secs - (hh as f64) * 3600.0;
        let mm = (rem / 60.0).floor() as u64;
        let ss = rem - (mm as f64) * 60.0;
        format!("{prefix}{hh:02}:{mm:02}:{ss:05.2}")
    }
}

fn trim_float(x: f64) -> String {
    if !x.is_finite() {
        return format!("{x}");
    }
    if x == x.trunc() && x.abs() < 1e16 {
        format!("{}", x as i64)
    } else {
        let s = format!("{x:.6}");
        let trimmed = s.trim_end_matches('0').trim_end_matches('.');
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::Unit;

    #[test]
    fn hms_pace_result() {
        let v = Value::new(14383.8, Unit::SECONDS);
        assert_eq!(format_value(&v), "03:59:43.80");
    }

    #[test]
    fn hms_short() {
        let v = Value::new(549.0, Unit::SECONDS);
        assert_eq!(format_value(&v), "09:09.00");
    }

    #[test]
    fn hms_sub_minute() {
        let v = Value::new(43.8, Unit::SECONDS);
        assert_eq!(format_value(&v), "43.80");
    }

    #[test]
    fn dimensionless() {
        let v = Value::new(42.0, Unit::DIMENSIONLESS);
        assert_eq!(format_value(&v), "42");
    }

    #[test]
    fn meters_per_second() {
        let v = Value::new(3.333, Unit { m: 1, s: -1, ..Unit::DIMENSIONLESS });
        assert_eq!(format_value(&v), "3.333 m/s");
    }

    #[test]
    fn negative_time() {
        let v = Value::new(-90.0, Unit::SECONDS);
        assert_eq!(format_value(&v), "-01:30.00");
    }
}

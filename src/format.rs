use crate::ast::UnitExpr;
use crate::eval::Answer;
use crate::unit::UnitRegistry;
use crate::value::Value;

pub fn format(answer: &Answer) -> String {
    match answer {
        Answer::Bare(v) => format_value(v),
        Answer::Assigned { name, value } => format!("{name} = {}", format_value(value)),
    }
}

pub fn format_value(v: &Value) -> String {
    // For pure-time values: HMS unless the user explicitly converted to a
    // single non-second time atom like `hr` or `min`. Composite displays that
    // happen to reduce to seconds (e.g. `miles*min/mile` from the canonical
    // pace expression) are noise from arithmetic propagation, not intent.
    let display_overrides_hms = v
        .display
        .as_ref()
        .map(is_explicit_time_unit)
        .unwrap_or(false);

    if v.unit.is_pure_time() && v.mag.is_finite() && !display_overrides_hms {
        return format_hms(v.mag);
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
    // A single atom with exponent 1 whose name is a non-second time unit.
    // `s` is excluded so that `9:09 in s` still renders as HMS.
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

/// Try to render in the value's preferred display unit. Returns None if the
/// hint doesn't resolve, doesn't dimensionally match the value, or simplifies
/// to dimensionless (in which case base-unit rendering is clearer).
fn render_with_display(v: &Value) -> Option<String> {
    let display = v.display.as_ref()?;
    let registry = UnitRegistry::standard();
    let (display_unit, factor) = registry.resolve(display).ok()?;
    if display_unit != v.unit {
        return None;
    }
    if factor == 0.0 || !factor.is_finite() {
        return None;
    }
    let mag = trim_float(v.mag / factor);
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
        // Show up to 6 significant decimal places, trimming trailing zeros.
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

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

    // Try simplifying first. Arithmetic propagates display as a Mul/Div
    // tree of operand displays; for `time / pace` (= `hours / (mins/mile)`)
    // the time atoms cancel, leaving just `mile` as the surviving display.
    // Without this pass the user sees `0.3 hours/mins/mile` instead of
    // `18 mile`.
    let simplified = simplify_display(display, &registry);
    let to_use: &UnitExpr = simplified.as_ref().unwrap_or(display);

    let r = registry.resolve(to_use).ok()?;
    if r.unit != v.unit {
        return None;
    }
    if r.factor == 0.0 || !r.factor.is_finite() {
        return None;
    }
    let mag = trim_float(v.mag / r.factor);
    let unit_str = render_unit_expr(to_use);
    if unit_str.is_empty() {
        Some(mag)
    } else {
        Some(format!("{mag} {unit_str}"))
    }
}

/// If the display tree has atoms that cancel along a base dimension (e.g.
/// `hours` and `mins` both contributing to time with net exponent zero),
/// return a reduced UnitExpr with those atoms dropped. Their factor
/// contribution stays implicitly via the resolution path.
///
/// Returns `None` when no simplification is possible.
fn simplify_display(display: &UnitExpr, registry: &UnitRegistry) -> Option<UnitExpr> {
    use std::collections::HashMap;

    let mut occurrences: Vec<(String, i32)> = Vec::new();
    collect_atoms(display, 1, &mut occurrences);

    // Aggregate same-named atoms.
    let mut by_name: HashMap<String, i32> = HashMap::new();
    for (name, exp) in occurrences {
        *by_name.entry(name).or_insert(0) += exp;
    }
    by_name.retain(|_, exp| *exp != 0);

    // Group by base dimension. Drop a group when (a) it has more than one
    // distinct atom and (b) its total exponent sums to 0 — that's the
    // "fully canceled" case where the atoms exist only as a constant
    // factor. Mixed groups with non-zero net (e.g. mile²/foot) are kept
    // intact so user intent isn't erased.
    let mut by_dim: HashMap<crate::unit::Unit, Vec<(String, i32)>> = HashMap::new();
    for (name, exp) in &by_name {
        let def = registry.lookup_atom(name)?;
        by_dim.entry(def.unit).or_default().push((name.clone(), *exp));
    }

    let mut surviving: Vec<(String, i32)> = Vec::new();
    for (_, group) in by_dim {
        let net: i32 = group.iter().map(|(_, e)| *e).sum();
        if net == 0 && group.len() > 1 {
            continue;
        }
        surviving.extend(group);
    }

    if surviving.is_empty() {
        // All atoms cancelled — display is dimensionless. Caller will
        // fall through to base-unit (empty) rendering.
        return None;
    }

    // Always reconstruct in canonical numer/denom form, even when nothing
    // cancelled. `5 km / (mins/mile)` produces a display of
    // `km / (mins/mile)` which renders as `km/mins/mile` (ambiguous);
    // canonicalizing flattens it to `km*mile/mins` for clarity.

    // Sort: positives first, then negatives, alphabetical within. Builds
    // tidy `mile` or `m^2/s` rather than `s^-1*m^2`.
    surviving.sort_by(|a, b| b.1.signum().cmp(&a.1.signum()).then(a.0.cmp(&b.0)));

    // Reconstruct as a Div(numer, denom) tree where possible — render
    // as "mile" or "m^2/s" rather than "m^2*s^-1".
    let (positives, negatives): (Vec<_>, Vec<_>) =
        surviving.into_iter().partition(|(_, e)| *e > 0);

    fn build_mul(items: Vec<(String, i32)>) -> Option<UnitExpr> {
        let mut iter = items.into_iter();
        let (n, e) = iter.next()?;
        let mut acc = UnitExpr::Atom(n, e.abs());
        for (n, e) in iter {
            acc = UnitExpr::Mul(Box::new(acc), Box::new(UnitExpr::Atom(n, e.abs())));
        }
        Some(acc)
    }

    let numer = build_mul(positives);
    let denom = build_mul(negatives);
    match (numer, denom) {
        (Some(n), None) => Some(n),
        (Some(n), Some(d)) => Some(UnitExpr::Div(Box::new(n), Box::new(d))),
        (None, Some(d)) => Some(UnitExpr::Div(
            Box::new(UnitExpr::Atom("1".to_string(), 0)),
            Box::new(d),
        )),
        (None, None) => None,
    }
}

fn collect_atoms(u: &UnitExpr, sign: i32, out: &mut Vec<(String, i32)>) {
    match u {
        UnitExpr::Atom(name, exp) => out.push((name.clone(), exp * sign)),
        UnitExpr::Mul(a, b) => {
            collect_atoms(a, sign, out);
            collect_atoms(b, sign, out);
        }
        UnitExpr::Div(a, b) => {
            collect_atoms(a, sign, out);
            collect_atoms(b, -sign, out);
        }
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

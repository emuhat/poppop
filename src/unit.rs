use std::collections::HashMap;

use crate::ast::UnitExpr;
use crate::error::Error;

/// 7-dimensional SI base vector. m=length, s=time, kg=mass, a=current,
/// k=temperature, mol=substance, cd=luminous intensity. We don't model
/// dimensionless angle, information, or counts as dimensions — pint declares
/// them with `[]` and we map those to `DIMENSIONLESS`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Unit {
    pub m: i32,
    pub s: i32,
    pub kg: i32,
    pub a: i32,
    pub k: i32,
    pub mol: i32,
    pub cd: i32,
}

impl Unit {
    pub const DIMENSIONLESS: Unit = Unit { m: 0, s: 0, kg: 0, a: 0, k: 0, mol: 0, cd: 0 };
    pub const METERS:        Unit = Unit { m: 1, s: 0, kg: 0, a: 0, k: 0, mol: 0, cd: 0 };
    pub const SECONDS:       Unit = Unit { m: 0, s: 1, kg: 0, a: 0, k: 0, mol: 0, cd: 0 };
    pub const KILOGRAMS:     Unit = Unit { m: 0, s: 0, kg: 1, a: 0, k: 0, mol: 0, cd: 0 };
    pub const AMPERES:       Unit = Unit { m: 0, s: 0, kg: 0, a: 1, k: 0, mol: 0, cd: 0 };
    pub const KELVIN:        Unit = Unit { m: 0, s: 0, kg: 0, a: 0, k: 1, mol: 0, cd: 0 };
    pub const MOLES:         Unit = Unit { m: 0, s: 0, kg: 0, a: 0, k: 0, mol: 1, cd: 0 };
    pub const CANDELAS:      Unit = Unit { m: 0, s: 0, kg: 0, a: 0, k: 0, mol: 0, cd: 1 };

    pub fn mul(&self, o: &Self) -> Self {
        Unit {
            m: self.m + o.m,
            s: self.s + o.s,
            kg: self.kg + o.kg,
            a: self.a + o.a,
            k: self.k + o.k,
            mol: self.mol + o.mol,
            cd: self.cd + o.cd,
        }
    }

    pub fn div(&self, o: &Self) -> Self {
        Unit {
            m: self.m - o.m,
            s: self.s - o.s,
            kg: self.kg - o.kg,
            a: self.a - o.a,
            k: self.k - o.k,
            mol: self.mol - o.mol,
            cd: self.cd - o.cd,
        }
    }

    pub fn pow(&self, n: i32) -> Self {
        Unit {
            m: self.m * n,
            s: self.s * n,
            kg: self.kg * n,
            a: self.a * n,
            k: self.k * n,
            mol: self.mol * n,
            cd: self.cd * n,
        }
    }

    pub fn is_dimensionless(&self) -> bool {
        *self == Unit::DIMENSIONLESS
    }

    pub fn is_pure_time(&self) -> bool {
        *self == Unit::SECONDS
    }

    pub fn render(&self) -> String {
        if self.is_dimensionless() {
            return String::new();
        }
        let entries = [
            ("m", self.m),
            ("s", self.s),
            ("kg", self.kg),
            ("A", self.a),
            ("K", self.k),
            ("mol", self.mol),
            ("cd", self.cd),
        ];
        let mut num: Vec<(&str, i32)> = Vec::new();
        let mut den: Vec<(&str, i32)> = Vec::new();
        for (name, exp) in entries {
            if exp > 0 {
                num.push((name, exp));
            } else if exp < 0 {
                den.push((name, -exp));
            }
        }
        let render_part = |parts: &[(&str, i32)]| -> String {
            parts
                .iter()
                .map(|(n, e)| if *e == 1 { (*n).to_string() } else { format!("{n}^{e}") })
                .collect::<Vec<_>>()
                .join("*")
        };
        match (num.is_empty(), den.is_empty()) {
            (true, true) => String::new(),
            (false, true) => render_part(&num),
            (true, false) => format!("1/{}", render_part(&den)),
            (false, false) => format!("{}/{}", render_part(&num), render_part(&den)),
        }
    }
}

/// Factor for an atom name interpreted as a time-base atom (seconds equivalence).
/// Returns Some(factor) for known time atoms (`s`, `sec`, `min`, `hour`, `hr`) and
/// None for everything else. Kept in sync with `UnitRegistry::standard()` below.
pub fn time_atom_factor(name: &str) -> Option<f64> {
    match name {
        "s" | "sec" | "second" | "seconds" => Some(1.0),
        "min" | "minute" | "minutes" | "mins" => Some(60.0),
        "hour" | "hours" | "hr" | "h" => Some(3600.0),
        _ => None,
    }
}

/// Cumulative factor of just the time-base atoms in a UnitExpr — used by the
/// parser when lowering `H:MM:SS <unit>` so `9:09 min/mile` becomes the same
/// physical quantity as `9:09 s/mile`.
pub fn unit_time_factor(expr: &UnitExpr) -> f64 {
    match expr {
        UnitExpr::Atom(name, exp) => {
            time_atom_factor(name).map(|f| f.powi(*exp)).unwrap_or(1.0)
        }
        UnitExpr::Mul(a, b) => unit_time_factor(a) * unit_time_factor(b),
        UnitExpr::Div(a, b) => unit_time_factor(a) / unit_time_factor(b),
    }
}

pub struct UnitRegistry {
    atoms: HashMap<String, (Unit, f64)>,
    /// SI prefixes loaded from pint. Keyed by the full prefix word ("kilo")
    /// AND the symbol form ("k"). Value is the multiplier.
    prefixes: HashMap<String, f64>,
    /// Names we've intentionally excluded — temperature scales with non-zero
    /// offsets (celsius, fahrenheit, rankine, …). These would silently give
    /// wrong answers under our linear-conversion model, so we surface them
    /// as a specific error instead.
    excluded_offsets: std::collections::HashSet<String>,
}

impl UnitRegistry {
    pub fn new_empty() -> Self {
        UnitRegistry {
            atoms: HashMap::new(),
            prefixes: HashMap::new(),
            excluded_offsets: std::collections::HashSet::new(),
        }
    }

    pub fn add_excluded_offset(&mut self, name: &str) {
        self.excluded_offsets.insert(name.to_string());
    }

    pub fn is_excluded_offset(&self, name: &str) -> bool {
        self.excluded_offsets.contains(name)
    }

    pub fn standard() -> Self {
        let mut reg = UnitRegistry::new_empty();
        reg.load_pint_default();
        reg
    }

    pub fn add_atom(&mut self, name: &str, unit: Unit, factor: f64) {
        self.atoms.insert(name.to_string(), (unit, factor));
    }

    pub fn atom_count(&self) -> usize {
        self.atoms.len()
    }

    pub fn add_prefix(&mut self, name: &str, factor: f64) {
        self.prefixes.insert(name.to_string(), factor);
    }

    pub fn lookup_atom(&self, name: &str) -> Option<(Unit, f64)> {
        if let Some((u, f)) = self.atoms.get(name) {
            return Some((*u, *f));
        }
        // Try prefix expansion: if the name starts with a known prefix and the
        // remainder is an atom, combine them. Longer prefix matches first.
        let mut keys: Vec<&String> = self.prefixes.keys().collect();
        keys.sort_by(|a, b| b.len().cmp(&a.len()));
        for prefix in keys {
            if let Some(rest) = name.strip_prefix(prefix.as_str()) {
                if let Some((u, f)) = self.atoms.get(rest) {
                    let scale = self.prefixes[prefix];
                    return Some((*u, *f * scale));
                }
            }
        }
        None
    }

    pub fn resolve(&self, expr: &UnitExpr) -> Result<(Unit, f64), Error> {
        match expr {
            UnitExpr::Atom(name, exp) => {
                let (u, f) = self.lookup_atom(name).ok_or_else(|| {
                    if self.is_excluded_offset(name) {
                        Error::OffsetUnit(name.clone())
                    } else {
                        Error::UnknownUnit(name.clone())
                    }
                })?;
                Ok((u.pow(*exp), f.powi(*exp)))
            }
            UnitExpr::Mul(a, b) => {
                let (ua, fa) = self.resolve(a)?;
                let (ub, fb) = self.resolve(b)?;
                Ok((ua.mul(&ub), fa * fb))
            }
            UnitExpr::Div(a, b) => {
                let (ua, fa) = self.resolve(a)?;
                let (ub, fb) = self.resolve(b)?;
                Ok((ua.div(&ub), fa / fb))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mul_div_pow() {
        let m = Unit::METERS;
        let s = Unit::SECONDS;
        assert_eq!(m.mul(&s), Unit { m: 1, s: 1, ..Unit::DIMENSIONLESS });
        assert_eq!(m.div(&s), Unit { m: 1, s: -1, ..Unit::DIMENSIONLESS });
        assert_eq!(m.pow(2), Unit { m: 2, ..Unit::DIMENSIONLESS });
    }

    #[test]
    fn render() {
        assert_eq!(Unit::DIMENSIONLESS.render(), "");
        assert_eq!(Unit::METERS.render(), "m");
        assert_eq!(Unit { m: 1, s: -1, ..Unit::DIMENSIONLESS }.render(), "m/s");
        assert_eq!(Unit { m: 2, ..Unit::DIMENSIONLESS }.render(), "m^2");
        assert_eq!(Unit { s: -1, ..Unit::DIMENSIONLESS }.render(), "1/s");
        assert_eq!(Unit { m: 1, s: -2, ..Unit::DIMENSIONLESS }.render(), "m/s^2");
    }

    #[test]
    fn resolve_simple() {
        let reg = UnitRegistry::standard();
        let (u, f) = reg.resolve(&UnitExpr::Atom("mile".into(), 1)).unwrap();
        assert_eq!(u, Unit::METERS);
        assert!((f - 1609.344).abs() < 1e-3, "got {}", f);
    }

    #[test]
    fn resolve_composite_mph() {
        let reg = UnitRegistry::standard();
        let mph = UnitExpr::Div(
            Box::new(UnitExpr::Atom("mile".into(), 1)),
            Box::new(UnitExpr::Atom("hour".into(), 1)),
        );
        let (u, f) = reg.resolve(&mph).unwrap();
        assert_eq!(u, Unit { m: 1, s: -1, ..Unit::DIMENSIONLESS });
        assert!((f - (1609.344 / 3600.0)).abs() < 1e-9);
    }

    #[test]
    fn resolve_unknown() {
        let reg = UnitRegistry::standard();
        let e = reg.resolve(&UnitExpr::Atom("blorp".into(), 1)).unwrap_err();
        assert!(matches!(e, Error::UnknownUnit(_)));
    }

    #[test]
    fn prefix_expansion() {
        // kilometer = kilo + meter, even though we don't define "kilometer" directly.
        let reg = UnitRegistry::standard();
        let (u, f) = reg.resolve(&UnitExpr::Atom("kilometer".into(), 1)).unwrap();
        assert_eq!(u, Unit::METERS);
        assert!((f - 1000.0).abs() < 1e-9);
    }
}

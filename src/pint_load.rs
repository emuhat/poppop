//! Loader for the bundled pint definition files (BSD-3-Clause, see
//! data/PINT_LICENSE). At registry init we walk the files and register
//! every prefix and atom we can resolve. Definitions we can't parse
//! (offset units, contexts, system blocks, fractional dim powers)
//! are silently skipped — better to drop a few atoms than refuse to boot.

use crate::unit::{Unit, UnitRegistry};

const DEFAULT_EN: &str = include_str!("../data/default_en.txt");
const CONSTANTS_EN: &str = include_str!("../data/constants_en.txt");

impl UnitRegistry {
    pub(crate) fn load_pint_default(&mut self) {
        // Pint's actual layout: default_en.txt declares prefixes + base units,
        // then `@import constants_en.txt` mid-file, then derived units that
        // reference both. To replicate without parsing the import directive,
        // we make a few passes: each pass registers what it can, lines that
        // failed get retried until no progress is made.
        let mut last_count = 0;
        for _ in 0..10 {
            load_file(self, DEFAULT_EN);
            load_file(self, CONSTANTS_EN);
            let count = self.atom_count();
            if count == last_count {
                break;
            }
            last_count = count;
        }
        // Common composite aliases pint doesn't ship as atoms.
        if let (Some((mu, mf)), Some((hu, hf))) =
            (self.lookup_atom("mile"), self.lookup_atom("hour"))
        {
            self.add_atom("mph", mu.div(&hu), mf / hf);
        }
        if let (Some((ku, kf)), Some((hu, hf))) =
            (self.lookup_atom("kilometer"), self.lookup_atom("hour"))
        {
            self.add_atom("kph", ku.div(&hu), kf / hf);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Skip {
    None,
    UntilEnd,
}

fn load_file(reg: &mut UnitRegistry, src: &str) {
    let mut skip = Skip::None;
    let mut buffer = String::new();

    for raw_line in src.lines() {
        let stripped = strip_comment(raw_line);
        if stripped.trim().is_empty() {
            continue;
        }
        if let Some(rest) = stripped.strip_suffix('\\') {
            buffer.push_str(rest);
            continue;
        }
        buffer.push_str(stripped);
        let line = buffer.trim().to_string();
        buffer.clear();

        if skip == Skip::UntilEnd {
            if line == "@end" {
                skip = Skip::None;
            }
            continue;
        }

        if line.starts_with("@defaults")
            || line.starts_with("@context")
            || line.starts_with("@system")
        {
            skip = Skip::UntilEnd;
            continue;
        }
        if line.starts_with("@import")
            || line.starts_with("@alias")
            || line.starts_with("@group")
            || line.starts_with("@end")
        {
            // @group ... @end wraps regular definitions; we leave the inner
            // definitions to be parsed and just ignore the wrapping lines.
            continue;
        }
        if line.starts_with('[') {
            // Derived dimension declaration like `[density] = [mass] / [volume]`.
            continue;
        }

        let _ = parse_definition(reg, &line);
    }
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

fn parse_definition(reg: &mut UnitRegistry, line: &str) -> Result<(), ()> {
    let parts: Vec<&str> = line.split('=').map(str::trim).collect();
    if parts.len() < 2 {
        return Err(());
    }
    let head = parts[0];
    let body = parts[1];
    let aliases: Vec<&str> = parts[2..]
        .iter()
        .copied()
        .filter(|a| !a.is_empty() && *a != "_")
        .collect();

    if let Some(prefix) = head.strip_suffix('-') {
        let factor = parse_prefix_body(body)?;
        reg.add_prefix(prefix, factor);
        for alias in &aliases {
            if let Some(p) = alias.strip_suffix('-') {
                reg.add_prefix(p, factor);
            }
        }
        return Ok(());
    }

    if !is_simple_ident(head) {
        return Err(());
    }

    // Strip `; offset: <n>` annotation. If the offset is non-zero, the unit
    // is an affine temperature scale (celsius/fahrenheit/rankine) — our
    // linear conversion model can't represent it. Register the names as
    // intentionally excluded so the user gets a clear error rather than
    // a silently-wrong answer.
    let (body, offset_state) = strip_offset(body);
    if matches!(offset_state, OffsetState::NonZero) {
        mark_offset_excluded(reg, head, &aliases);
        return Ok(());
    }

    if body.starts_with('[') {
        let dim = body.trim();
        let unit = match dim {
            "[length]" => Unit::METERS,
            "[time]" => Unit::SECONDS,
            "[mass]" => Unit::KILOGRAMS,
            "[current]" => Unit::AMPERES,
            "[temperature]" => Unit::KELVIN,
            "[substance]" => Unit::MOLES,
            "[luminosity]" => Unit::CANDELAS,
            "[]" => Unit::DIMENSIONLESS,
            _ => return Err(()),
        };
        // pint's `gram = [mass] = g` quirk: gram is the mass base in pint, but
        // our SI base is kg = 1000 g, so gram has factor 0.001.
        let factor = if head == "gram" { 0.001 } else { 1.0 };
        add_with_plural(reg, head, unit, factor);
        for alias in &aliases {
            if is_simple_ident(alias) {
                add_with_plural(reg, alias, unit, factor);
            }
        }
        return Ok(());
    }

    let (unit, factor) = eval_pint_expr(reg, body)?;
    add_with_plural(reg, head, unit, factor);
    for alias in &aliases {
        if is_simple_ident(alias) {
            add_with_plural(reg, alias, unit, factor);
        }
    }
    Ok(())
}

/// Pint auto-pluralizes by adding `s`. Replicate that here so `miles`,
/// `seconds`, `meters` all resolve.
fn add_with_plural(reg: &mut UnitRegistry, name: &str, unit: Unit, factor: f64) {
    reg.add_atom(name, unit, factor);
    if !name.ends_with('s') {
        reg.add_atom(&format!("{name}s"), unit, factor);
    }
}

enum OffsetState {
    None,
    Zero,
    NonZero,
}

/// Pull off pint's `; offset: <expr>` annotation. We only care about
/// whether the offset is exactly zero (linear-equivalent, OK to register)
/// or anything else (exclude). The actual value isn't useful since our
/// conversion model can't represent affine units.
fn strip_offset(body: &str) -> (&str, OffsetState) {
    let Some(idx) = body.find(';') else { return (body, OffsetState::None) };
    let (left, right) = body.split_at(idx);
    let right = right[1..].trim();
    let Some(rest) = right.strip_prefix("offset:") else {
        return (body, OffsetState::None);
    };
    let rest = rest.trim();
    let state = if rest == "0" || rest.parse::<f64>().map(|v| v == 0.0).unwrap_or(false) {
        OffsetState::Zero
    } else {
        OffsetState::NonZero
    };
    (left.trim(), state)
}

fn mark_offset_excluded(reg: &mut UnitRegistry, head: &str, aliases: &[&str]) {
    reg.add_excluded_offset(head);
    if !head.ends_with('s') {
        reg.add_excluded_offset(&format!("{head}s"));
    }
    for alias in aliases {
        if is_simple_ident(alias) {
            reg.add_excluded_offset(alias);
            if !alias.ends_with('s') {
                reg.add_excluded_offset(&format!("{alias}s"));
            }
        }
    }
}

fn is_simple_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_prefix_body(body: &str) -> Result<f64, ()> {
    let body = body.trim();
    if let Ok(n) = body.parse::<f64>() {
        return Ok(n);
    }
    let dummy = UnitRegistry::new_empty();
    let (_, f) = eval_pint_expr(&dummy, body)?;
    Ok(f)
}

// ─── pint expression evaluator ──────────────────────────────────────────────

struct State<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> State<'a> {
    fn new(src: &'a str) -> Self {
        State { src: src.as_bytes(), pos: 0 }
    }
    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }
    fn bump(&mut self) -> Option<u8> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }
    fn at_str(&self, s: &str) -> bool {
        self.src[self.pos..].starts_with(s.as_bytes())
    }
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
    }
    fn done(&self) -> bool {
        self.pos >= self.src.len()
    }
}

fn eval_pint_expr(reg: &UnitRegistry, expr: &str) -> Result<(Unit, f64), ()> {
    let mut s = State::new(expr);
    let v = parse_addsub(&mut s, reg)?;
    s.skip_ws();
    if !s.done() {
        return Err(());
    }
    Ok(v)
}

fn parse_addsub(s: &mut State, reg: &UnitRegistry) -> Result<(Unit, f64), ()> {
    let mut left = parse_muldiv(s, reg)?;
    loop {
        s.skip_ws();
        match s.peek() {
            Some(b'+') => {
                s.bump();
                let right = parse_muldiv(s, reg)?;
                if left.0 != right.0 {
                    return Err(());
                }
                left.1 += right.1;
            }
            Some(b'-') => {
                s.bump();
                let right = parse_muldiv(s, reg)?;
                if left.0 != right.0 {
                    return Err(());
                }
                left.1 -= right.1;
            }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_muldiv(s: &mut State, reg: &UnitRegistry) -> Result<(Unit, f64), ()> {
    let mut left = parse_power(s, reg)?;
    loop {
        s.skip_ws();
        if s.at_str("**") || s.peek() == Some(b'^') {
            // power belongs to parse_power, not muldiv
            break;
        }
        match s.peek() {
            Some(b'*') => {
                s.bump();
                let right = parse_power(s, reg)?;
                left = (left.0.mul(&right.0), left.1 * right.1);
            }
            Some(b'/') => {
                s.bump();
                let right = parse_power(s, reg)?;
                left = (left.0.div(&right.0), left.1 / right.1);
            }
            // Implicit multiplication: `299792458 m/s` ⇒ `299792458 * m/s`.
            Some(c) if c.is_ascii_alphabetic() || c == b'_' || c == b'(' || c >= 0x80 => {
                let right = parse_power(s, reg)?;
                left = (left.0.mul(&right.0), left.1 * right.1);
            }
            _ => break,
        }
    }
    Ok(left)
}

fn parse_power(s: &mut State, reg: &UnitRegistry) -> Result<(Unit, f64), ()> {
    let base = parse_unary(s, reg)?;
    s.skip_ws();
    let consumed = if s.at_str("**") {
        s.pos += 2;
        true
    } else if s.peek() == Some(b'^') {
        s.bump();
        true
    } else {
        false
    };
    if consumed {
        let exp = parse_unary(s, reg)?;
        if !exp.0.is_dimensionless() {
            return Err(());
        }
        let n = exp.1;
        let int_exp = n.round() as i32;
        let is_int = (n - int_exp as f64).abs() < 1e-9;
        let dim = if is_int {
            base.0.pow(int_exp)
        } else if base.0.is_dimensionless() {
            Unit::DIMENSIONLESS
        } else {
            return Err(());
        };
        let mag = base.1.powf(n);
        return Ok((dim, mag));
    }
    Ok(base)
}

fn parse_unary(s: &mut State, reg: &UnitRegistry) -> Result<(Unit, f64), ()> {
    s.skip_ws();
    if s.peek() == Some(b'-') {
        s.bump();
        let inner = parse_unary(s, reg)?;
        return Ok((inner.0, -inner.1));
    }
    if s.peek() == Some(b'+') {
        s.bump();
        return parse_unary(s, reg);
    }
    parse_atom(s, reg)
}

fn parse_atom(s: &mut State, reg: &UnitRegistry) -> Result<(Unit, f64), ()> {
    s.skip_ws();
    let Some(c) = s.peek() else { return Err(()) };
    if c == b'(' {
        s.bump();
        let v = parse_addsub(s, reg)?;
        s.skip_ws();
        if s.peek() != Some(b')') {
            return Err(());
        }
        s.bump();
        return Ok(v);
    }
    if c.is_ascii_digit() || c == b'.' {
        return Ok((Unit::DIMENSIONLESS, parse_number(s)?));
    }
    parse_identifier(s, reg)
}

fn parse_number(s: &mut State) -> Result<f64, ()> {
    let start = s.pos;
    while let Some(c) = s.peek() {
        if c.is_ascii_digit() || c == b'.' {
            s.bump();
        } else {
            break;
        }
    }
    if matches!(s.peek(), Some(b'e' | b'E')) {
        s.bump();
        if matches!(s.peek(), Some(b'+' | b'-')) {
            s.bump();
        }
        while let Some(c) = s.peek() {
            if c.is_ascii_digit() {
                s.bump();
            } else {
                break;
            }
        }
    }
    let text = std::str::from_utf8(&s.src[start..s.pos]).map_err(|_| ())?;
    text.parse::<f64>().map_err(|_| ())
}

fn parse_identifier(s: &mut State, reg: &UnitRegistry) -> Result<(Unit, f64), ()> {
    let start = s.pos;
    while let Some(c) = s.peek() {
        if c.is_ascii_alphanumeric() || c == b'_' {
            s.bump();
        } else if c >= 0x80 {
            let ch = std::str::from_utf8(&s.src[s.pos..])
                .ok()
                .and_then(|s| s.chars().next())
                .ok_or(())?;
            s.pos += ch.len_utf8();
        } else {
            break;
        }
    }
    if start == s.pos {
        return Err(());
    }
    let name = std::str::from_utf8(&s.src[start..s.pos]).map_err(|_| ())?;
    if name == "pi" || name == "π" {
        return Ok((Unit::DIMENSIONLESS, std::f64::consts::PI));
    }
    if let Some(v) = reg.lookup_atom(name) {
        return Ok(v);
    }
    Err(())
}

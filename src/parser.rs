use pest::Parser as PestParser;
use pest::iterators::Pair;
use pest_derive::Parser;

use crate::ast::*;
use crate::error::Error;
use crate::unit::unit_time_factor;

#[derive(Parser)]
#[grammar = "grammar.pest"]
struct PoppopParser;

pub fn parse(input: &str) -> Result<Statement, Error> {
    let mut pairs = PoppopParser::parse(Rule::program, input)
        .map_err(|e| Error::Parse(e.to_string()))?;
    let program = pairs.next().unwrap();
    let statement = program.into_inner().next().unwrap();
    parse_statement(statement)
}

fn parse_statement(pair: Pair<Rule>) -> Result<Statement, Error> {
    let inner = pair.into_inner().next().unwrap();
    match inner.as_rule() {
        Rule::assignment => {
            let mut it = inner.into_inner();
            let name = it.next().unwrap().as_str().to_string();
            let expr = parse_expression(it.next().unwrap())?;
            Ok(Statement::Assign(name, expr))
        }
        Rule::expression => Ok(Statement::Expr(parse_expression(inner)?)),
        r => unreachable!("statement inner: {r:?}"),
    }
}

fn parse_expression(pair: Pair<Rule>) -> Result<Expr, Error> {
    let conv = pair.into_inner().next().unwrap();
    parse_conversion(conv)
}

fn parse_conversion(pair: Pair<Rule>) -> Result<Expr, Error> {
    let mut it = pair.into_inner();
    let mut e = parse_addsub(it.next().unwrap())?;
    while let Some(p) = it.next() {
        match p.as_rule() {
            Rule::in_kw => {
                let u = it.next().unwrap();
                let ue = parse_unit_expr(u)?;
                e = Expr::Convert(Box::new(e), ue);
            }
            r => unreachable!("conversion inner: {r:?}"),
        }
    }
    Ok(e)
}

fn parse_addsub(pair: Pair<Rule>) -> Result<Expr, Error> {
    let mut it = pair.into_inner();
    let mut left = parse_addsub_term(it.next().unwrap())?;
    while let Some(op_pair) = it.next() {
        let op = match op_pair.as_str() {
            "+" => Op::Add,
            "-" => Op::Sub,
            s => unreachable!("add_op: {s}"),
        };
        let right = parse_addsub_term(it.next().unwrap())?;
        left = Expr::Binary(Box::new(left), op, Box::new(right));
    }
    Ok(left)
}

fn parse_addsub_term(pair: Pair<Rule>) -> Result<Expr, Error> {
    let mut it = pair.into_inner();
    let first = it.next().unwrap();
    match first.as_rule() {
        Rule::neg_op => {
            let muldiv = it.next().unwrap();
            let inner = parse_muldiv(muldiv)?;
            Ok(Expr::Unary(UnaryOp::Neg, Box::new(inner)))
        }
        Rule::muldiv => parse_muldiv(first),
        r => unreachable!("addsub_term inner: {r:?}"),
    }
}

fn parse_muldiv(pair: Pair<Rule>) -> Result<Expr, Error> {
    let mut it = pair.into_inner();
    let mut left = parse_power(it.next().unwrap())?;
    while let Some(p) = it.next() {
        let (op, right) = match p.as_rule() {
            Rule::mul_op => {
                let op = match p.as_str() {
                    "*" => Op::Mul,
                    "/" => Op::Div,
                    s => unreachable!("mul_op: {s}"),
                };
                let r = parse_power(it.next().unwrap())?;
                (op, r)
            }
            Rule::power => (Op::Mul, parse_power(p)?),
            r => unreachable!("muldiv inner: {r:?}"),
        };
        left = Expr::Binary(Box::new(left), op, Box::new(right));
    }
    Ok(left)
}

fn parse_power(pair: Pair<Rule>) -> Result<Expr, Error> {
    let mut it = pair.into_inner();
    let base = parse_atom(it.next().unwrap())?;
    if let Some(exp_pair) = it.next() {
        let exp = parse_pow_exp(exp_pair)?;
        return Ok(Expr::Binary(Box::new(base), Op::Pow, Box::new(exp)));
    }
    Ok(base)
}

fn parse_pow_exp(pair: Pair<Rule>) -> Result<Expr, Error> {
    let mut it = pair.into_inner();
    let first = it.next().unwrap();
    match first.as_rule() {
        Rule::neg_op => {
            let atom = it.next().unwrap();
            let inner = parse_atom(atom)?;
            Ok(Expr::Unary(UnaryOp::Neg, Box::new(inner)))
        }
        Rule::atom => parse_atom(first),
        r => unreachable!("pow_exp inner: {r:?}"),
    }
}

fn parse_atom(pair: Pair<Rule>) -> Result<Expr, Error> {
    let inner = pair.into_inner().next().unwrap();
    match inner.as_rule() {
        Rule::time_with_unit => parse_time_with_unit(inner),
        Rule::number => {
            let n: f64 = inner.as_str().parse().unwrap();
            Ok(Expr::Number(n, None))
        }
        Rule::call => parse_call(inner),
        Rule::var => Ok(Expr::Var(
            inner.into_inner().next().unwrap().as_str().to_string(),
        )),
        Rule::paren => parse_expression(inner.into_inner().next().unwrap()),
        r => unreachable!("atom inner: {r:?}"),
    }
}

fn parse_time_with_unit(pair: Pair<Rule>) -> Result<Expr, Error> {
    let mut it = pair.into_inner();
    let total_seconds = parse_time_literal_seconds(it.next().unwrap());
    // Bare time literal: lowers to a Number with magnitude in seconds and unit "s".
    // Time literal + trailing unit (e.g. `9:09 min/mile`): the seconds value is
    // re-expressed in the trailing unit's time component so the *physical* quantity
    // matches the bare reading. For `9:09 min/mile`, time component is `min` (factor 60),
    // so 549 / 60 = 9.15 → Number(9.15, min/mile). Eval then resolves min/mile to
    // base s/m at 9.15 * 60/1609.344 = 0.3411 s/m, which matches `9:09 / mile`.
    if let Some(u_pair) = it.next() {
        let u = parse_unit_expr(u_pair)?;
        let tf = unit_time_factor(&u);
        Ok(Expr::Number(total_seconds / tf, Some(u)))
    } else {
        Ok(Expr::Number(
            total_seconds,
            Some(UnitExpr::Atom("s".to_string(), 1)),
        ))
    }
}

fn parse_time_literal_seconds(pair: Pair<Rule>) -> f64 {
    let form = pair.into_inner().next().unwrap();
    match form.as_rule() {
        Rule::hms_form => {
            let mut it = form.into_inner();
            let h: f64 = it.next().unwrap().as_str().parse().unwrap();
            let m: f64 = it.next().unwrap().as_str().parse().unwrap();
            let s: f64 = it.next().unwrap().as_str().parse().unwrap();
            h * 3600.0 + m * 60.0 + s
        }
        Rule::ms_form => {
            let mut it = form.into_inner();
            let m: f64 = it.next().unwrap().as_str().parse().unwrap();
            let s: f64 = it.next().unwrap().as_str().parse().unwrap();
            m * 60.0 + s
        }
        r => unreachable!("time_literal inner: {r:?}"),
    }
}

fn parse_call(pair: Pair<Rule>) -> Result<Expr, Error> {
    let mut it = pair.into_inner();
    let name = it.next().unwrap().as_str().to_string();
    let mut args = Vec::new();
    for arg in it {
        args.push(parse_expression(arg)?);
    }
    Ok(Expr::Call(name, args))
}

fn parse_unit_expr(pair: Pair<Rule>) -> Result<UnitExpr, Error> {
    let mut it = pair.into_inner();
    let mut left = parse_unit_factor(it.next().unwrap())?;
    while let Some(op_pair) = it.next() {
        let op = op_pair.as_str();
        let right = parse_unit_factor(it.next().unwrap())?;
        left = match op {
            "*" => UnitExpr::Mul(Box::new(left), Box::new(right)),
            "/" => UnitExpr::Div(Box::new(left), Box::new(right)),
            s => unreachable!("unit_op: {s}"),
        };
    }
    Ok(left)
}

fn parse_unit_factor(pair: Pair<Rule>) -> Result<UnitExpr, Error> {
    let mut it = pair.into_inner();
    let name = it.next().unwrap().as_str().to_string();
    let exp: i32 = if let Some(e_pair) = it.next() {
        e_pair.as_str().parse().unwrap()
    } else {
        1
    };
    Ok(UnitExpr::Atom(name, exp))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Statement {
        parse(s).unwrap_or_else(|e| panic!("parse error on {s:?}: {e}"))
    }

    #[test]
    fn bare_number() {
        match p("42") {
            Statement::Expr(Expr::Number(n, None)) => assert_eq!(n, 42.0),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn decimal() {
        match p("2.5") {
            Statement::Expr(Expr::Number(n, None)) => assert!((n - 2.5).abs() < 1e-12),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn assignment_with_unit() {
        // `x = 42 mph` now parses as `x = 42 * mph` (units-as-values).
        match p("x = 42 mph") {
            Statement::Assign(name, Expr::Binary(_, Op::Mul, _)) => assert_eq!(name, "x"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn time_literal_ms() {
        match p("9:09") {
            Statement::Expr(Expr::Number(n, Some(UnitExpr::Atom(u, 1)))) => {
                assert!((n - 549.0).abs() < 1e-9);
                assert_eq!(u, "s");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn time_literal_hms() {
        match p("1:23:45") {
            Statement::Expr(Expr::Number(n, Some(UnitExpr::Atom(u, 1)))) => {
                assert!((n - (3600.0 + 23.0 * 60.0 + 45.0)).abs() < 1e-9);
                assert_eq!(u, "s");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn time_literal_fractional() {
        match p("9:09.5") {
            Statement::Expr(Expr::Number(n, Some(_))) => {
                assert!((n - 549.5).abs() < 1e-9);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn paren_and_binary() {
        match p("(x + y) * 2") {
            Statement::Expr(Expr::Binary(_, Op::Mul, _)) => {}
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn call() {
        match p("sqrt(16)") {
            Statement::Expr(Expr::Call(name, args)) => {
                assert_eq!(name, "sqrt");
                assert_eq!(args.len(), 1);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn conversion() {
        match p("x in m/s") {
            Statement::Expr(Expr::Convert(_, UnitExpr::Div(_, _))) => {}
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn pace_expression() {
        // 26.2 miles * 9:09 min/mile — the canonical example.
        let s = p("26.2 miles * 9:09 min/mile");
        match s {
            Statement::Expr(Expr::Binary(_, Op::Mul, _)) => {}
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn keyword_in_not_var() {
        // `in` shouldn't parse as a variable identifier.
        assert!(parse("in").is_err());
    }

    #[test]
    fn implicit_mult_div_then_unit() {
        // 42/4 mph parses as ((42/4) * mph) — three Binary nodes nested.
        // Outer must be Mul (with mph on right).
        let s = p("42/4 mph");
        match s {
            Statement::Expr(Expr::Binary(_, Op::Mul, ref rhs)) => {
                match rhs.as_ref() {
                    Expr::Var(name) => assert_eq!(name, "mph"),
                    other => panic!("rhs got {other:?}"),
                }
            }
            other => panic!("got {other:?}"),
        }
    }
}

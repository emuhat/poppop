#[derive(Debug, Clone)]
pub enum Statement {
    Assign(String, Expr),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Number(f64, Option<UnitExpr>),
    /// `YYYY-MM-DD` date literal. Stored as components; the eval phase
    /// converts to `chrono::NaiveDateTime` at UTC midnight.
    DateLiteral(i32, u32, u32),
    Var(String),
    Binary(Box<Expr>, Op, Box<Expr>),
    Unary(UnaryOp, Box<Expr>),
    Call(String, Vec<Expr>),
    Convert(Box<Expr>, UnitExpr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
}

#[derive(Debug, Clone)]
pub enum UnitExpr {
    Atom(String, i32),
    Mul(Box<UnitExpr>, Box<UnitExpr>),
    Div(Box<UnitExpr>, Box<UnitExpr>),
}

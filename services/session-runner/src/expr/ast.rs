//! The expression AST. Every node carries a byte [`Span`] into the source
//! so both static-check and evaluation errors can point at the offending
//! token.

use super::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnOp {
    Not,
    Neg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub(crate) fn sym(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }

    pub(crate) fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Expr {
    Null(Span),
    Bool(bool, Span),
    Num(f64, Span),
    Str(String, Span),
    /// A bare identifier in value position (a namespace root after
    /// static checking).
    Ident(String, Span),
    Member {
        obj: Box<Expr>,
        field: String,
        span: Span,
    },
    Index {
        obj: Box<Expr>,
        idx: Box<Expr>,
        span: Span,
    },
    Call {
        func: String,
        func_span: Span,
        args: Vec<Expr>,
        span: Span,
    },
    Unary {
        op: UnOp,
        rhs: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    Cond {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    pub(crate) fn span(&self) -> Span {
        match self {
            Expr::Null(s)
            | Expr::Bool(_, s)
            | Expr::Num(_, s)
            | Expr::Str(_, s)
            | Expr::Ident(_, s) => *s,
            Expr::Member { span, .. }
            | Expr::Index { span, .. }
            | Expr::Call { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Cond { span, .. } => *span,
        }
    }

    /// Collects every bare identifier in the tree into `roots`, keeping
    /// the span of each name's first occurrence (in tree order). After
    /// static checking these are exactly the namespace roots the
    /// expression reads (function names live in `Call::func`, not in
    /// `Ident` nodes).
    pub(crate) fn collect_idents<'a>(
        &'a self,
        roots: &mut std::collections::BTreeMap<&'a str, Span>,
    ) {
        match self {
            Expr::Null(_) | Expr::Bool(_, _) | Expr::Num(_, _) | Expr::Str(_, _) => {}
            Expr::Ident(name, span) => {
                roots.entry(name).or_insert(*span);
            }
            Expr::Member { obj, .. } => obj.collect_idents(roots),
            Expr::Index { obj, idx, .. } => {
                obj.collect_idents(roots);
                idx.collect_idents(roots);
            }
            Expr::Call { args, .. } => {
                for a in args {
                    a.collect_idents(roots);
                }
            }
            Expr::Unary { rhs, .. } => rhs.collect_idents(roots),
            Expr::Binary { lhs, rhs, .. } => {
                lhs.collect_idents(roots);
                rhs.collect_idents(roots);
            }
            Expr::Cond {
                cond, then, els, ..
            } => {
                cond.collect_idents(roots);
                then.collect_idents(roots);
                els.collect_idents(roots);
            }
        }
    }

    /// Smart constructor for unary expressions: folds `-` over a numeric
    /// literal so `-3` and `- -3` produce literal nodes, matching the
    /// grammar pin that `-` is an operator while number literals are
    /// unsigned.
    pub(crate) fn unary(op: UnOp, rhs: Expr, span: Span) -> Expr {
        if let (UnOp::Neg, Expr::Num(n, _)) = (op, &rhs) {
            return Expr::Num(-n, span);
        }
        Expr::Unary {
            op,
            rhs: Box::new(rhs),
            span,
        }
    }

    /// Canonical s-expression form. Grouping is structural; spans are
    /// ignored.
    pub(crate) fn canon(&self) -> String {
        match self {
            Expr::Null(_) => "null".into(),
            Expr::Bool(b, _) => b.to_string(),
            Expr::Num(n, _) => format!("{n:?}"),
            Expr::Str(s, _) => format!("{s:?}"),
            Expr::Ident(name, _) => name.clone(),
            Expr::Member { obj, field, .. } => format!("(. {} {})", obj.canon(), field),
            Expr::Index { obj, idx, .. } => format!("([] {} {})", obj.canon(), idx.canon()),
            Expr::Call { func, args, .. } => {
                let mut out = format!("(call {func}");
                for a in args {
                    out.push(' ');
                    out.push_str(&a.canon());
                }
                out.push(')');
                out
            }
            Expr::Unary { op, rhs, .. } => {
                let sym = match op {
                    UnOp::Not => "!",
                    UnOp::Neg => "neg",
                };
                format!("({sym} {})", rhs.canon())
            }
            Expr::Binary { op, lhs, rhs, .. } => {
                format!("({} {} {})", op.sym(), lhs.canon(), rhs.canon())
            }
            Expr::Cond {
                cond, then, els, ..
            } => format!("(?: {} {} {})", cond.canon(), then.canon(), els.canon()),
        }
    }
}

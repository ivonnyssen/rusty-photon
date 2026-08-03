//! The expression AST. Every node carries a byte [`Span`] into the source
//! so both static-check and evaluation errors can point at the offending
//! token.

use super::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
}

/// A binary operator. Each variant's `serialize` string is that operator's
/// surface syntax — the lexer accepts it, the printer emits it, and parse and
/// evaluation diagnostics quote it — so it is a contract with the workflow
/// documents in the field, not a formatting detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr, strum::VariantArray)]
pub enum BinOp {
    #[strum(serialize = "+")]
    Add,
    #[strum(serialize = "-")]
    Sub,
    #[strum(serialize = "*")]
    Mul,
    #[strum(serialize = "/")]
    Div,
    #[strum(serialize = "%")]
    Rem,
    #[strum(serialize = "==")]
    Eq,
    #[strum(serialize = "!=")]
    Ne,
    #[strum(serialize = "<")]
    Lt,
    #[strum(serialize = "<=")]
    Le,
    #[strum(serialize = ">")]
    Gt,
    #[strum(serialize = ">=")]
    Ge,
    #[strum(serialize = "&&")]
    And,
    #[strum(serialize = "||")]
    Or,
}

impl BinOp {
    /// The operator's surface symbol.
    pub(crate) fn sym(self) -> &'static str {
        self.into()
    }

    pub(crate) const fn is_comparison(self) -> bool {
        matches!(
            self,
            Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Null(Span),
    Bool(bool, Span),
    Num(f64, Span),
    Str(String, Span),
    /// A bare identifier in value position (a namespace root after
    /// static checking).
    Ident(String, Span),
    Member {
        obj: Box<Self>,
        field: String,
        span: Span,
    },
    Index {
        obj: Box<Self>,
        idx: Box<Self>,
        span: Span,
    },
    Call {
        func: String,
        func_span: Span,
        args: Vec<Self>,
        span: Span,
    },
    Unary {
        op: UnOp,
        rhs: Box<Self>,
        span: Span,
    },
    Binary {
        op: BinOp,
        lhs: Box<Self>,
        rhs: Box<Self>,
        span: Span,
    },
    Cond {
        cond: Box<Self>,
        then: Box<Self>,
        els: Box<Self>,
        span: Span,
    },
}

impl Expr {
    pub(crate) const fn span(&self) -> Span {
        match self {
            Self::Null(s)
            | Self::Bool(_, s)
            | Self::Num(_, s)
            | Self::Str(_, s)
            | Self::Ident(_, s) => *s,
            Self::Member { span, .. }
            | Self::Index { span, .. }
            | Self::Call { span, .. }
            | Self::Unary { span, .. }
            | Self::Binary { span, .. }
            | Self::Cond { span, .. } => *span,
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
            Self::Null(_) | Self::Bool(_, _) | Self::Num(_, _) | Self::Str(_, _) => {}
            Self::Ident(name, span) => {
                roots.entry(name).or_insert(*span);
            }
            Self::Member { obj, .. } => obj.collect_idents(roots),
            Self::Index { obj, idx, .. } => {
                obj.collect_idents(roots);
                idx.collect_idents(roots);
            }
            Self::Call { args, .. } => {
                for a in args {
                    a.collect_idents(roots);
                }
            }
            Self::Unary { rhs, .. } => rhs.collect_idents(roots),
            Self::Binary { lhs, rhs, .. } => {
                lhs.collect_idents(roots);
                rhs.collect_idents(roots);
            }
            Self::Cond {
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
    pub(crate) fn unary(op: UnOp, rhs: Self, span: Span) -> Self {
        if let (UnOp::Neg, Self::Num(n, _)) = (op, &rhs) {
            return Self::Num(-n, span);
        }
        Self::Unary {
            op,
            rhs: Box::new(rhs),
            span,
        }
    }

    /// Canonical s-expression form. Grouping is structural; spans are
    /// ignored.
    pub(crate) fn canon(&self) -> String {
        match self {
            Self::Null(_) => "null".into(),
            Self::Bool(b, _) => b.to_string(),
            Self::Num(n, _) => format!("{n:?}"),
            Self::Str(s, _) => format!("{s:?}"),
            Self::Ident(name, _) => name.clone(),
            Self::Member { obj, field, .. } => format!("(. {} {})", obj.canon(), field),
            Self::Index { obj, idx, .. } => format!("([] {} {})", obj.canon(), idx.canon()),
            Self::Call { func, args, .. } => {
                let mut out = format!("(call {func}");
                for a in args {
                    out.push(' ');
                    out.push_str(&a.canon());
                }
                out.push(')');
                out
            }
            Self::Unary { op, rhs, .. } => {
                let sym = match op {
                    UnOp::Not => "!",
                    UnOp::Neg => "neg",
                };
                format!("({sym} {})", rhs.canon())
            }
            Self::Binary { op, lhs, rhs, .. } => {
                format!("({} {} {})", op.sym(), lhs.canon(), rhs.canon())
            }
            Self::Cond {
                cond, then, els, ..
            } => format!("(?: {} {} {})", cond.canon(), then.canon(), els.canon()),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// Every [`BinOp`] variant in declaration order, paired with the exact
    /// symbol it renders as. These symbols are the expression language's
    /// surface syntax — the lexer accepts them, the printer emits them, and
    /// parse and evaluation diagnostics quote them — so each one is a
    /// contract with the workflow documents in the field, not a formatting
    /// detail.
    const SYMBOLS: &[(BinOp, &str)] = &[
        (BinOp::Add, "+"),
        (BinOp::Sub, "-"),
        (BinOp::Mul, "*"),
        (BinOp::Div, "/"),
        (BinOp::Rem, "%"),
        (BinOp::Eq, "=="),
        (BinOp::Ne, "!="),
        (BinOp::Lt, "<"),
        (BinOp::Le, "<="),
        (BinOp::Gt, ">"),
        (BinOp::Ge, ">="),
        (BinOp::And, "&&"),
        (BinOp::Or, "||"),
    ];

    /// Declaration index of a variant, matched exhaustively with no wildcard
    /// arm: a new [`BinOp`] variant stops this compiling until its symbol is
    /// pinned in [`SYMBOLS`].
    fn declaration_index(op: BinOp) -> usize {
        match op {
            BinOp::Add => 0,
            BinOp::Sub => 1,
            BinOp::Mul => 2,
            BinOp::Div => 3,
            BinOp::Rem => 4,
            BinOp::Eq => 5,
            BinOp::Ne => 6,
            BinOp::Lt => 7,
            BinOp::Le => 8,
            BinOp::Gt => 9,
            BinOp::Ge => 10,
            BinOp::And => 11,
            BinOp::Or => 12,
        }
    }

    #[test]
    fn test_binop_sym_renders_the_pinned_symbol() {
        for (op, want) in SYMBOLS {
            assert_eq!(op.sym(), *want, "symbol for {op:?}");
        }
    }

    #[test]
    fn test_binop_symbol_table_covers_every_variant_in_order() {
        assert_eq!(SYMBOLS.len(), 13);
        for (i, (op, _)) in SYMBOLS.iter().enumerate() {
            assert_eq!(declaration_index(*op), i, "table position of {op:?}");
        }
    }
}

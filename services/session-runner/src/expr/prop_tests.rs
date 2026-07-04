//! Property tests: the pretty-print ↔ parse round-trip (grammar and
//! printer agree on precedence and lexical form) and no-panic robustness
//! over arbitrary input. The dedicated cargo-fuzz target in `fuzz/`
//! extends the no-panic property with coverage-guided input generation.

use chrono::{DateTime, TimeZone, Utc};
use proptest::prelude::*;
use serde_json::json;

use super::ast::{BinOp, Expr, UnOp};
use super::check::NAMESPACES;
use super::print::print;
use super::{EvalContext, Expression, Span};

fn sp() -> Span {
    Span::new(0, 0)
}

fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 3, 4, 0, 0).unwrap()
}

const BINOPS: &[BinOp] = &[
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::Rem,
    BinOp::Eq,
    BinOp::Ne,
    BinOp::Lt,
    BinOp::Le,
    BinOp::Gt,
    BinOp::Ge,
    BinOp::And,
    BinOp::Or,
];

const UNOPS: &[UnOp] = &[UnOp::Not, UnOp::Neg];

/// Fixed-arity single-argument functions (argument type is a runtime
/// concern; any expression is statically valid).
const FN1: &[&str] = &[
    "abs",
    "floor",
    "ceil",
    "round",
    "seconds",
    "humantime",
    "seconds_until",
];

fn call(func: &str, args: Vec<Expr>) -> Expr {
    Expr::Call {
        func: func.to_owned(),
        func_span: sp(),
        args,
        span: sp(),
    }
}

fn arb_field() -> impl Strategy<Value = String> {
    "[a-z_][a-z0-9_]{0,8}".prop_filter("reserved word", |s| {
        !matches!(s.as_str(), "null" | "true" | "false")
    })
}

fn arb_root() -> impl Strategy<Value = Expr> {
    prop::sample::select(NAMESPACES).prop_map(|ns| Expr::Ident(ns.to_owned(), sp()))
}

fn arb_num() -> impl Strategy<Value = f64> {
    prop_oneof![
        any::<f64>().prop_filter("finite", |f| f.is_finite()),
        -1.0e3..1.0e3,
    ]
}

fn arb_leaf() -> BoxedStrategy<Expr> {
    prop_oneof![
        Just(Expr::Null(sp())),
        any::<bool>().prop_map(|b| Expr::Bool(b, sp())),
        arb_num().prop_map(|n| Expr::Num(n, sp())),
        ".{0,12}".prop_map(|s| Expr::Str(s, sp())),
        any::<String>().prop_map(|s| Expr::Str(s, sp())),
        arb_root(),
    ]
    .boxed()
}

#[derive(Clone, Debug)]
enum Step {
    Field(String),
    IdxNum(f64),
    IdxStr(String),
}

fn arb_path() -> impl Strategy<Value = Expr> {
    let step = prop_oneof![
        arb_field().prop_map(Step::Field),
        (0.0..10.0f64).prop_map(Step::IdxNum),
        ".{0,6}".prop_map(Step::IdxStr),
    ];
    (arb_root(), prop::collection::vec(step, 0..3)).prop_map(|(root, steps)| {
        steps.into_iter().fold(root, |obj, s| match s {
            Step::Field(f) => Expr::Member {
                obj: Box::new(obj),
                field: f,
                span: sp(),
            },
            Step::IdxNum(n) => Expr::Index {
                obj: Box::new(obj),
                idx: Box::new(Expr::Num(n, sp())),
                span: sp(),
            },
            Step::IdxStr(s) => Expr::Index {
                obj: Box::new(obj),
                idx: Box::new(Expr::Str(s, sp())),
                span: sp(),
            },
        })
    })
}

/// Statically-valid ASTs (namespace roots, known functions and arities,
/// `has()` gets a path), built through the same smart constructor the
/// parser uses so negative literals fold identically.
fn arb_expr() -> BoxedStrategy<Expr> {
    arb_leaf()
        .prop_recursive(4, 32, 3, |inner| {
            prop_oneof![
                (inner.clone(), arb_field()).prop_map(|(o, f)| Expr::Member {
                    obj: Box::new(o),
                    field: f,
                    span: sp(),
                }),
                (inner.clone(), inner.clone()).prop_map(|(o, i)| Expr::Index {
                    obj: Box::new(o),
                    idx: Box::new(i),
                    span: sp(),
                }),
                (prop::sample::select(UNOPS), inner.clone()).prop_map(|(op, r)| Expr::unary(
                    op,
                    r,
                    sp()
                )),
                (prop::sample::select(BINOPS), inner.clone(), inner.clone()).prop_map(
                    |(op, l, r)| Expr::Binary {
                        op,
                        lhs: Box::new(l),
                        rhs: Box::new(r),
                        span: sp(),
                    }
                ),
                (inner.clone(), inner.clone(), inner.clone()).prop_map(|(c, t, e)| Expr::Cond {
                    cond: Box::new(c),
                    then: Box::new(t),
                    els: Box::new(e),
                    span: sp(),
                }),
                (prop::sample::select(FN1), inner.clone()).prop_map(|(f, a)| call(f, vec![a])),
                (inner.clone(), inner.clone(), inner.clone())
                    .prop_map(|(a, b, c)| call("clamp", vec![a, b, c])),
                (
                    prop::sample::select(&["min", "max"][..]),
                    prop::collection::vec(inner.clone(), 2..4)
                )
                    .prop_map(|(f, args)| call(f, args)),
                arb_path().prop_map(|p| call("has", vec![p])),
            ]
        })
        .boxed()
}

proptest! {
    /// The printer emits source the parser maps back to the same tree.
    #[test]
    fn prop_print_parse_round_trip(e in arb_expr()) {
        let src = print(&e);
        match Expression::parse(&src) {
            Ok(parsed) => prop_assert_eq!(parsed.canon(), e.canon(), "printed source: {:?}", src),
            Err(err) => prop_assert!(false, "print produced unparseable source {:?}: {}", src, err),
        }
    }

    /// The parser is total: any string yields Ok or Err, never a panic.
    #[test]
    fn prop_parse_never_panics_on_arbitrary_input(s in any::<String>()) {
        let _ = Expression::parse(&s);
    }

    /// Same, but biased toward grammar-dense character soup so mutations
    /// land near real syntax instead of in the unicode wilderness.
    #[test]
    fn prop_parse_never_panics_on_grammar_dense_input(
        s in r#"[ a-z0-9_.()\[\]'"+*/%<>=!&|?:,$-]{0,48}"#
    ) {
        let _ = Expression::parse(&s);
    }

    /// Evaluation is total over parseable input: Ok or Err, never a panic.
    #[test]
    fn prop_eval_never_panics(e in arb_expr()) {
        let src = print(&e);
        if let Ok(parsed) = Expression::parse(&src) {
            let session = json!({"x": 1.5, "arr": [1, 2, 3], "s": "str", "o": {"k": true}});
            let ctx = EvalContext {
                session: Some(&session),
                ..EvalContext::new(fixed_now())
            };
            let _ = parsed.eval(&ctx);
        }
    }
}

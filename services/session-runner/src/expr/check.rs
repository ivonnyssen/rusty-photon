//! Static (load-time) semantic checks over the parsed AST: namespace
//! roots, known functions and arities, `has()` path arguments.
//!
//! Field-name lexical rules and reserved words are already enforced by
//! construction — the parser only builds `Member` nodes from identifier
//! tokens, which the lexer restricts to ASCII and the parser rejects for
//! `null` / `true` / `false`.

use super::ast::Expr;
use super::ExprError;

pub(crate) const NAMESPACES: &[&str] = &["params", "session", "result", "event", "error"];

pub(crate) struct FuncSig {
    pub(crate) name: &'static str,
    pub(crate) min_args: usize,
    pub(crate) max_args: usize,
}

/// The v1 function set per `docs/services/session-runner.md` § Expressions.
pub(crate) const FUNCTIONS: &[FuncSig] = &[
    FuncSig {
        name: "abs",
        min_args: 1,
        max_args: 1,
    },
    FuncSig {
        name: "min",
        min_args: 2,
        max_args: usize::MAX,
    },
    FuncSig {
        name: "max",
        min_args: 2,
        max_args: usize::MAX,
    },
    FuncSig {
        name: "clamp",
        min_args: 3,
        max_args: 3,
    },
    FuncSig {
        name: "floor",
        min_args: 1,
        max_args: 1,
    },
    FuncSig {
        name: "ceil",
        min_args: 1,
        max_args: 1,
    },
    FuncSig {
        name: "round",
        min_args: 1,
        max_args: 1,
    },
    FuncSig {
        name: "seconds",
        min_args: 1,
        max_args: 1,
    },
    FuncSig {
        name: "humantime",
        min_args: 1,
        max_args: 1,
    },
    FuncSig {
        name: "has",
        min_args: 1,
        max_args: 1,
    },
    FuncSig {
        name: "seconds_until",
        min_args: 1,
        max_args: 1,
    },
];

/// Comma-separated list of the built-in function names, for diagnostics.
pub(crate) fn function_names() -> String {
    FUNCTIONS
        .iter()
        .map(|f| f.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A namespace path: an identifier root followed by member/index steps.
fn is_path(expr: &Expr) -> bool {
    match expr {
        Expr::Ident(..) => true,
        Expr::Member { obj, .. } | Expr::Index { obj, .. } => is_path(obj),
        _ => false,
    }
}

pub(crate) fn check(expr: &Expr) -> Result<(), ExprError> {
    match expr {
        Expr::Null(..) | Expr::Bool(..) | Expr::Num(..) | Expr::Str(..) => Ok(()),
        Expr::Ident(name, span) => {
            if !NAMESPACES.contains(&name.as_str()) {
                let hint = if FUNCTIONS.iter().any(|f| f.name == name) {
                    format!("`{name}` is a function — call it: {name}(…)")
                } else {
                    format!(
                        "unknown name `{name}` — expressions read the namespaces {}",
                        NAMESPACES.join(", ")
                    )
                };
                return Err(ExprError::parse(hint, *span));
            }
            Ok(())
        }
        Expr::Member { obj, .. } => check(obj),
        Expr::Index { obj, idx, .. } => {
            check(obj)?;
            check(idx)
        }
        Expr::Call {
            func,
            func_span,
            args,
            span,
        } => {
            let Some(sig) = FUNCTIONS.iter().find(|f| f.name == func) else {
                return Err(ExprError::parse(
                    format!(
                        "unknown function `{func}` — available: {}",
                        function_names()
                    ),
                    *func_span,
                ));
            };
            if args.len() < sig.min_args || args.len() > sig.max_args {
                let expected = if sig.max_args == usize::MAX {
                    format!("at least {}", sig.min_args)
                } else if sig.min_args == sig.max_args {
                    format!("{}", sig.min_args)
                } else {
                    format!("{}..{}", sig.min_args, sig.max_args)
                };
                return Err(ExprError::parse(
                    format!("`{func}` takes {expected} argument(s), got {}", args.len()),
                    *span,
                ));
            }
            if func == "has" {
                match args.first() {
                    Some(arg) if is_path(arg) => {}
                    Some(arg) => {
                        return Err(ExprError::parse(
                            "`has()` takes a namespace path like has(session.x)",
                            arg.span(),
                        ));
                    }
                    None => {
                        return Err(ExprError::parse(
                            "`has()` takes a namespace path like has(session.x)",
                            *span,
                        ));
                    }
                }
            }
            for a in args {
                check(a)?;
            }
            Ok(())
        }
        Expr::Unary { rhs, .. } => check(rhs),
        Expr::Binary { lhs, rhs, .. } => {
            check(lhs)?;
            check(rhs)
        }
        Expr::Cond {
            cond, then, els, ..
        } => {
            check(cond)?;
            check(then)?;
            check(els)
        }
    }
}

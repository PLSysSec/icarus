// vim: set tw=99 ts=4 sts=4 sw=4 et:

use std::error::Error;
use std::fmt;

use codespan_reporting::diagnostic::Diagnostic;
use enumset::{EnumSet, EnumSetType};
use thiserror::Error;

use cachet_util::fmt_join_or;

use crate::FrontendError;
use crate::ast::{FileId, Ident, Path, Span, Spanned, labels};

// TODO(spinda): Break up TypeCheckError like ResolveError.
#[derive(Debug, Error)]
pub enum TypeCheckError {
    #[error("found cycle in subtype relationships")]
    SubtypeCycle {
        first_cycle_type: Spanned<Ident>,
        other_cycle_types: Vec<Spanned<Ident>>,
    },
    #[error("recursive calls are not allowed")]
    CallCycle {
        first_cycle_callable: Spanned<Path>,
        other_cycle_callables: Vec<Spanned<Path>>,
    },
    #[error("op `{op}` can't have out-parameter `{out_param}`")]
    OpHasOutParam { op: Path, out_param: Spanned<Ident> },
    #[error("op `{op}` can't have explicit emits annotation")]
    OpHasExplicitEmits { op: Spanned<Path> },
    #[error("op `{op}` can't return a value")]
    OpReturnsValue { op: Path, ret_span: Span },
    #[error("op `{op}` is missing a body")]
    MissingOpBody { op: Path, body_span: Span },
    #[error("function `{fn_}` with explicit emits is missing a body")]
    MissingEmitsFnBody { fn_: Spanned<Path> },
    #[error("can't jump to label `{label}`")]
    GotoIrMismatch {
        label: Spanned<Ident>,
        callable: Option<Path>,
        expected_ir: Option<Spanned<Ident>>,
        found_ir: Ident,
    },
    #[error("can't bind label `{label}`")]
    BindIrMismatch {
        label: Spanned<Ident>,
        callable: Option<Path>,
        expected_ir: Option<Spanned<Ident>>,
        found_ir: Ident,
    },
    #[error("can't emit op `{op}`")]
    EmitIrMismatch {
        op: Spanned<Path>,
        callable: Option<Path>,
        expected_ir: Option<Spanned<Ident>>,
        found_ir: Ident,
    },
    #[error("can't call function `{fn_}`")]
    EmitsFnIrMismatch {
        fn_: Spanned<Path>,
        callable: Option<Path>,
        expected_ir: Option<Spanned<Ident>>,
        found_ir: Ident,
    },
    #[error("mismatched types")]
    TypeMismatch {
        expected_type: Ident,
        found_type: Ident,
        span: Span,
    },
    #[error("casting `{source_type}` to `{target_type}` is invalid")]
    InvalidCast {
        source_type: Ident,
        target_type: Ident,
        expr_span: Span,
    },
    #[error(
        "calls to `{target}` are unsafe and can only appear inside an unsafe function, op, or \
        block"
    )]
    UnsafeCallInSafeContext {
        target: Spanned<Path>,
        target_defined_at: Span,
    },
    #[error(
        "casts from `{source_type}` to subtype `{target_type}` are unsafe and can only appear \
        inside an unsafe function, op, or block"
    )]
    UnsafeCastInSafeContext {
        source_type: Ident,
        target_type: Spanned<Ident>,
    },
    #[error("wrong number of arguments to `{target}`")]
    ArgCountMismatch {
        expected_arg_count: usize,
        found_arg_count: usize,
        target: Spanned<Path>,
        target_defined_at: Span,
        args_span: Span,
    },
    #[error("mismatched argument kind for parameter `{param}` to `{target}`")]
    ArgKindMismatch {
        expected_arg_kind: ArgKind,
        found_arg_kinds: EnumSet<ArgKind>,
        arg_span: Span,
        target: Path,
        param: Spanned<Ident>,
    },
    #[error("mismatched argument type for parameter `{param}` to `{target}`")]
    ArgTypeMismatch {
        expected_type: Ident,
        found_type: Ident,
        arg_span: Span,
        target: Path,
        param: Spanned<Ident>,
    },
    #[error("IR of label argument doesn't match parameter `{param}` to `{target}`")]
    ArgIrMismatch {
        expected_ir: Ident,
        found_ir: Ident,
        arg_span: Span,
        target: Path,
        param: Spanned<Ident>,
    },
    #[error("left- and right-hand sides of operator have mismatched types")]
    BinaryOperatorTypeMismatch {
        operator_span: Span,
        lhs_span: Span,
        lhs_type: Ident,
        rhs_span: Span,
        rhs_type: Ident,
    },
    // TODO(spinda): Set up operator type categories: numeric, signed numeric,
    // integral.
    #[error("mismatched operand type for numeric operator")]
    NumericOperatorTypeMismatch {
        operand_span: Span,
        operand_type: Ident,
        operator_span: Span,
    },
    #[error("mismatched value type for assignment to `{lhs}`")]
    AssignTypeMismatch {
        expected_type: Ident,
        found_type: Ident,
        lhs: Path,
        lhs_defined_at: Option<Span>,
        rhs_span: Span,
    },
    #[error("can't access field `{field}` of non-struct type `{parent_type}`")]
    NonStructFieldAccess {
        field: Spanned<Ident>,
        parent_span: Span,
        parent_type: Ident,
    },
    #[error("no field `{field}` on type `{parent_type}`")]
    FieldNotFound {
        field: Spanned<Ident>,
        parent_type: Spanned<Ident>,
    },
    #[error("can't use label field `{field}` on type `{parent_type}` as a variable field")]
    LabelFieldUsedAsVarField {
        field: Spanned<Ident>,
        parent_type: Spanned<Ident>,
    },
    // todo: support this and remove the error
    #[error("attempted to set default value for mutable var")]
    MutGlobalVarWithValue { var: Spanned<Path> },
    // todo: support this and remove the error
    #[error("return statement found outside function")]
    ReturnOutsideCallable { span: Span },
    #[error("global var value is not constant")]
    NonConstGlobalVarItem { expr: Span },
}

impl FrontendError for TypeCheckError {
    fn span(&self) -> Span {
        match self {
            TypeCheckError::SubtypeCycle {
                first_cycle_type, ..
            } => first_cycle_type.span,
            TypeCheckError::CallCycle {
                first_cycle_callable,
                ..
            } => first_cycle_callable.span,
            TypeCheckError::OpHasOutParam { out_param, .. } => out_param.span,
            TypeCheckError::OpHasExplicitEmits { op, .. } => op.span,
            TypeCheckError::OpReturnsValue { ret_span, .. } => *ret_span,
            TypeCheckError::MissingOpBody { body_span, .. } => *body_span,
            TypeCheckError::MissingEmitsFnBody { fn_, .. } => fn_.span,
            TypeCheckError::GotoIrMismatch { label, .. } => label.span,
            TypeCheckError::BindIrMismatch { label, .. } => label.span,
            TypeCheckError::EmitIrMismatch { op, .. } => op.span,
            TypeCheckError::EmitsFnIrMismatch { fn_, .. } => fn_.span,
            TypeCheckError::TypeMismatch {
                span: expr_span, ..
            } => *expr_span,
            TypeCheckError::InvalidCast { expr_span, .. } => *expr_span,
            TypeCheckError::UnsafeCallInSafeContext { target, .. } => target.span,
            TypeCheckError::UnsafeCastInSafeContext { target_type, .. } => target_type.span,
            TypeCheckError::ArgCountMismatch { target, .. } => target.span,
            TypeCheckError::ArgKindMismatch { arg_span, .. } => *arg_span,
            TypeCheckError::ArgTypeMismatch { arg_span, .. } => *arg_span,
            TypeCheckError::ArgIrMismatch { arg_span, .. } => *arg_span,
            TypeCheckError::BinaryOperatorTypeMismatch { operator_span, .. } => *operator_span,
            TypeCheckError::NumericOperatorTypeMismatch { operand_span, .. } => *operand_span,
            TypeCheckError::AssignTypeMismatch { rhs_span, .. } => *rhs_span,
            TypeCheckError::NonStructFieldAccess { field, .. } => field.span,
            TypeCheckError::FieldNotFound { field, .. } => field.span,
            TypeCheckError::LabelFieldUsedAsVarField { field, .. } => field.span,
            TypeCheckError::MutGlobalVarWithValue { var, .. } => var.span,
            TypeCheckError::ReturnOutsideCallable { span, .. } => *span,
            TypeCheckError::NonConstGlobalVarItem { expr, .. } => *expr,
        }
    }

    fn build_diagnostic(&self) -> Diagnostic<FileId> {
        let msg = match self {
            TypeCheckError::SubtypeCycle {
                first_cycle_type,
                other_cycle_types,
            } => {
                format!(
                    "{}`{}` is a subtype of `{}`",
                    if other_cycle_types.is_empty() {
                        ""
                    } else {
                        "(1) "
                    },
                    first_cycle_type,
                    other_cycle_types.last().unwrap_or(first_cycle_type)
                )
            }
            TypeCheckError::CallCycle {
                first_cycle_callable,
                other_cycle_callables,
            } => {
                format!(
                    "{}`{}` calls `{}`",
                    if other_cycle_callables.is_empty() {
                        ""
                    } else {
                        "(1) "
                    },
                    other_cycle_callables.last().unwrap_or(first_cycle_callable),
                    first_cycle_callable
                )
            }
            TypeCheckError::OpHasOutParam { .. }
            | TypeCheckError::OpHasExplicitEmits { .. }
            | TypeCheckError::OpReturnsValue { .. }
            | TypeCheckError::MissingOpBody { .. } => {
                "allowed for functions but not for ops".to_owned()
            }
            TypeCheckError::MissingEmitsFnBody { .. } => {
                "allowed for functions without explicit emits annotation".to_owned()
            }
            TypeCheckError::GotoIrMismatch {
                callable,
                expected_ir,
                found_ir,
                ..
            } => match expected_ir {
                Some(expected_ir) => {
                    format!(
                        "expected `{}` label, found `{}` label",
                        expected_ir, found_ir
                    )
                }
                None => match callable {
                    Some(path) => format!(
                        "unexpected jump to `{}` label inside non-interpreter `{}`",
                        found_ir, path
                    ),
                    None => format!(
                        "unexpected jump to `{}` label inside non-interpreter",
                        found_ir
                    ),
                },
            },
            TypeCheckError::BindIrMismatch {
                callable,
                expected_ir,
                found_ir,
                ..
            } => match expected_ir {
                Some(expected_ir) => {
                    format!(
                        "expected `{}` label, found `{}` label",
                        expected_ir, found_ir
                    )
                }
                None => match callable {
                    Some(path) => format!(
                        "unexpected bind of `{}` label inside non-emitter `{}`",
                        found_ir, path
                    ),
                    None => format!("unexpected bind of `{}` label inside non-emitter", found_ir),
                },
            },
            TypeCheckError::EmitIrMismatch {
                callable,
                expected_ir,
                found_ir,
                ..
            } => match expected_ir {
                Some(expected_ir) => {
                    format!("expected `{}` op, found `{}` op", expected_ir, found_ir)
                }
                None => match callable {
                    Some(path) => format!(
                        "unexpected emit of `{}` label inside non-emitter `{}`",
                        found_ir, path
                    ),
                    None => format!("unexpected emit of `{}` label inside non-emitter", found_ir),
                },
            },
            TypeCheckError::EmitsFnIrMismatch {
                callable,
                expected_ir,
                found_ir,
                ..
            } => match expected_ir {
                Some(expected_ir) => {
                    format!(
                        "expected function that emits `{}` , found `{}`",
                        expected_ir, found_ir
                    )
                }
                None => match callable {
                    Some(path) => format!(
                        "unexpected call to function that emits `{}` inside non-emitter `{}`",
                        found_ir, path
                    ),
                    None => format!(
                        "unexpected call to function that emits `{}` inside non-emitter",
                        found_ir
                    ),
                },
            },
            TypeCheckError::TypeMismatch {
                expected_type,
                found_type,
                ..
            } => {
                format!("expected `{}`, found `{}`", expected_type, found_type)
            }
            TypeCheckError::InvalidCast {
                source_type,
                target_type,
                ..
            } => {
                format!(
                    "`{}` isn't a subtype of `{}', and vice-versa",
                    source_type, target_type
                )
            }
            TypeCheckError::UnsafeCallInSafeContext { .. } => "call to unsafe function".to_owned(),
            TypeCheckError::UnsafeCastInSafeContext { .. } => "unsafe cast".to_owned(),
            TypeCheckError::ArgCountMismatch {
                expected_arg_count, ..
            } => {
                format!(
                    "expected {} argument{}",
                    expected_arg_count,
                    if *expected_arg_count == 1 { "" } else { "s" }
                )
            }
            TypeCheckError::ArgKindMismatch {
                expected_arg_kind,
                found_arg_kinds,
                ..
            } => {
                format!(
                    "expected {}, found {}",
                    expected_arg_kind,
                    ArgKinds(found_arg_kinds)
                )
            }
            TypeCheckError::ArgTypeMismatch {
                expected_type: expected,
                found_type: found,
                ..
            }
            | TypeCheckError::ArgIrMismatch {
                expected_ir: expected,
                found_ir: found,
                ..
            } => {
                format!("expected `{}`, found `{}`", expected, found)
            }
            TypeCheckError::BinaryOperatorTypeMismatch { .. } => format!(""),
            TypeCheckError::NumericOperatorTypeMismatch { operand_type, .. } => {
                format!("expected numeric type, found `{}`", operand_type)
            }
            TypeCheckError::AssignTypeMismatch {
                expected_type,
                found_type,
                ..
            } => {
                format!("expected `{}`, found `{}`", expected_type, found_type)
            }
            TypeCheckError::NonStructFieldAccess { .. } => {
                format!("fields can only be accessed on structs")
            }
            TypeCheckError::FieldNotFound { .. } => {
                format!("no such field")
            }
            TypeCheckError::LabelFieldUsedAsVarField { .. } => {
                format!("expected a variable field")
            }
            TypeCheckError::MutGlobalVarWithValue { .. } => {
                format!("only immutable variables may have default values")
            }
            TypeCheckError::ReturnOutsideCallable { .. } => {
                format!("return statements may only occur inside fn's or op's")
            }
            TypeCheckError::NonConstGlobalVarItem { .. } => {
                format!("expression is not constant")
            }
        };

        let mut labels = Vec::from_iter(labels![Primary(self.span()) => msg]);

        match self {
            TypeCheckError::SubtypeCycle {
                first_cycle_type,
                other_cycle_types,
            } => {
                let mut prev_cycle_type = first_cycle_type;
                labels.extend(other_cycle_types.iter().enumerate().flat_map(
                    |(curr_cycle_type_index, curr_cycle_type)| {
                        let label = labels![
                            Secondary(curr_cycle_type.span) =>
                                format!(
                                    "({}) `{}` is a subtype of `{}`",
                                    curr_cycle_type_index + 2,
                                    curr_cycle_type,
                                    prev_cycle_type
                                )
                        ];
                        prev_cycle_type = curr_cycle_type;
                        label
                    },
                ));
            }
            TypeCheckError::CallCycle {
                first_cycle_callable,
                other_cycle_callables,
            } => {
                let mut prev_cycle_callable = first_cycle_callable;
                labels.extend(other_cycle_callables.iter().enumerate().flat_map(
                    |(curr_cycle_callable_index, curr_cycle_callable)| {
                        let label = labels![
                            Secondary(curr_cycle_callable.span) =>
                                format!(
                                    "({}) `{}` calls `{}`",
                                    curr_cycle_callable_index + 2,
                                    prev_cycle_callable,
                                    curr_cycle_callable
                                )
                        ];
                        prev_cycle_callable = curr_cycle_callable;
                        label
                    },
                ));
            }
            TypeCheckError::OpHasOutParam { .. } => (),
            TypeCheckError::OpHasExplicitEmits { .. } => (),
            TypeCheckError::OpReturnsValue { .. } => (),
            TypeCheckError::ReturnOutsideCallable { .. } => (),
            TypeCheckError::MissingOpBody { .. } => (),
            TypeCheckError::MissingEmitsFnBody { .. } => (),
            TypeCheckError::MutGlobalVarWithValue { .. } => (),
            TypeCheckError::GotoIrMismatch {
                callable,
                expected_ir,
                ..
            } => {
                if let (Some(expected_ir), Some(callable)) = (expected_ir, callable) {
                    labels.extend(labels![
                        Secondary(expected_ir.span) =>
                            format!(
                                "`{}` interprets `{}` ops",
                                callable, expected_ir
                            )
                    ]);
                }
            }
            TypeCheckError::BindIrMismatch {
                callable,
                expected_ir,
                ..
            }
            | TypeCheckError::EmitIrMismatch {
                callable,
                expected_ir,
                ..
            }
            | TypeCheckError::EmitsFnIrMismatch {
                callable,
                expected_ir,
                ..
            } => {
                if let (Some(expected_ir), Some(callable)) = (expected_ir, callable) {
                    labels.extend(labels![
                        Secondary(expected_ir.span) =>
                            format!(
                                "`{}` emits `{}` ops",
                                callable, expected_ir
                            )
                    ]);
                }
            }
            TypeCheckError::TypeMismatch { .. } => (),
            TypeCheckError::InvalidCast { .. } => (),
            TypeCheckError::UnsafeCallInSafeContext {
                target,
                target_defined_at,
                ..
            } => {
                labels.extend(labels![
                    Secondary(*target_defined_at)
                        => format!("function `{}` defined here", target)
                ]);
            }
            TypeCheckError::UnsafeCastInSafeContext { .. } => (),
            TypeCheckError::ArgCountMismatch {
                found_arg_count,
                target,
                target_defined_at,
                args_span,
                ..
            } => {
                labels.extend(labels![
                    Secondary(*args_span) =>
                        format!(
                            "found {} argument{}",
                            found_arg_count,
                            if *found_arg_count == 1 { "" } else { "s" }
                        ),
                    Secondary(*target_defined_at) =>
                        format!("function `{}` defined here", target)
                ]);
            }
            TypeCheckError::ArgKindMismatch { param, .. } => {
                labels.extend(labels![
                    Secondary(param.span) =>
                        format!("parameter `{}` defined here", param)
                ]);
            }
            TypeCheckError::ArgTypeMismatch { param, .. }
            | TypeCheckError::ArgIrMismatch { param, .. } => {
                labels.extend(labels![
                    Secondary(param.span) => format!("parameter `{}` defined here", param)
                ]);
            }
            TypeCheckError::BinaryOperatorTypeMismatch {
                lhs_span,
                lhs_type,
                rhs_span,
                rhs_type,
                ..
            } => {
                labels.extend(labels![
                    Secondary(*lhs_span) => format!("type `{}`", lhs_type),
                    Secondary(*rhs_span) => format!("type `{}`", rhs_type)
                ]);
            }
            TypeCheckError::NumericOperatorTypeMismatch { operator_span, .. } => {
                labels.extend(labels![
                    Secondary(*operator_span) => "operator is only defined for numeric types"
                ]);
            }
            TypeCheckError::AssignTypeMismatch {
                lhs,
                lhs_defined_at,
                ..
            } => {
                if let Some(lhs_defined_at) = lhs_defined_at {
                    labels.extend(labels![
                        Secondary(*lhs_defined_at) => format!("variable `{}` defined here", lhs)
                    ]);
                }
            }
            TypeCheckError::NonStructFieldAccess {
                parent_span,
                parent_type,
                ..
            } => labels.extend(labels![
                Secondary(*parent_span) => format!("expected struct type, found `{}`", parent_type)
            ]),
            TypeCheckError::FieldNotFound { parent_type, .. } => {
                labels.extend(labels![
                    Secondary(parent_type.span) =>
                        format!("type `{}` defined here", parent_type.value)
                ]);
            }
            TypeCheckError::LabelFieldUsedAsVarField { parent_type, .. } => {
                labels.extend(labels![
                    Secondary(parent_type.span) =>
                        format!("type `{}` defined here", parent_type.value)
                ]);
            }
            TypeCheckError::NonConstGlobalVarItem { .. } => (),
        }

        Diagnostic::error()
            .with_message(self.to_string())
            .with_labels(labels)
    }
}

#[derive(Debug)]
pub struct TypeCheckErrors(pub Vec<TypeCheckError>);

impl fmt::Display for TypeCheckErrors {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "type checking failed")
    }
}

impl Error for TypeCheckErrors {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self.0.first() {
            Some(error) => Some(error),
            None => None,
        }
    }
}

#[derive(Debug, EnumSetType, Hash)]
pub enum ArgKind {
    Expr,
    OutVar,
    Label,
    OutLabel,
}

impl fmt::Display for ArgKind {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self {
            ArgKind::Expr => write!(f, "expression"),
            ArgKind::OutVar => write!(f, "variable out-parameter"),
            ArgKind::Label => write!(f, "label"),
            ArgKind::OutLabel => write!(f, "label out-parameter"),
        }
    }
}

struct ArgKinds<'a>(&'a EnumSet<ArgKind>);

impl<'a> fmt::Display for ArgKinds<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt_join_or(f, self.0.iter(), |f, kind| kind.fmt(f))
    }
}

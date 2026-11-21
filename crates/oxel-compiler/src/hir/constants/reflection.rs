//! Compiler-owned queries over resolved source types and imported metadata.

use std::borrow::Cow;

use crate::{Diagnostic, OccurrenceKind, Span, syntax};

use super::super::{Checked, Checker, ExprKind, Field, ParameterKind, Struct, Structs, Type};

pub(in crate::hir) fn builtin_field_info() -> Struct<'static> {
    let span = Span::default();
    let fields = [
        ("name", Type::String),
        ("type_name", Type::String),
        ("readonly", Type::Bool),
    ];
    Struct {
        namespace: None,
        internal: false,
        name: "compiler.FieldInfo",
        builtin: true,
        interface: None,
        implementations: Vec::new(),
        span,
        fields: fields
            .iter()
            .map(|&(name, ty)| Field {
                name: Cow::Borrowed(name),
                ty,
                span,
            })
            .collect(),
        type_parameters: Vec::new(),
        bounds: Vec::new(),
        union: None,
        field_ids: fields
            .into_iter()
            .enumerate()
            .map(|(index, (name, _))| (name, index))
            .collect(),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum TypeQuery {
    TypeName,
    FieldNames,
    Fields,
    VariantNames,
}

impl TypeQuery {
    pub(in crate::hir) fn from_path(path: &syntax::Path<'_>) -> Option<Self> {
        match path.segments.as_slice() {
            [("compiler", _), ("type_name", _)] => Some(Self::TypeName),
            [("compiler", _), ("field_names", _)] => Some(Self::FieldNames),
            [("compiler", _), ("fields_of", _)] => Some(Self::Fields),
            [("compiler", _), ("variant_names", _)] => Some(Self::VariantNames),
            _ => None,
        }
    }

    pub(in crate::hir) fn lower<'src>(
        self,
        checker: &mut Checker<'_, 'src>,
        path: &syntax::Path<'src>,
        type_arguments: &[syntax::Path<'src>],
        args: &[syntax::Expr<'src>],
    ) -> Checked<(ExprKind, Type)> {
        let span = path.span;
        if !checker.comptime {
            return Err(Diagnostic::new(
                "E0501",
                span,
                "compiler type queries are only available during constant evaluation",
            ));
        }
        let [argument] = type_arguments else {
            return Err(Diagnostic::new(
                "E0205",
                span,
                "compiler type query expects one type argument",
            ));
        };
        if !args.is_empty() {
            return Err(Diagnostic::new(
                "E0205",
                span,
                "compiler type query takes no value arguments",
            ));
        }
        let types = checker.types;
        checker.track_type_path(argument);
        let ty = types.scoped_type(
            argument,
            checker.type_parameters,
            Some(checker.bounds),
            true,
            false,
        )?;
        checker.record_annotation(argument, ty);
        // Validate closed queries even when their enclosing function is never called.
        if !matches!(self, Self::TypeName)
            && types
                .substitute_parameters(ty, ParameterKind::Method, argument.span, |_| None)?
                .is_some()
        {
            self.visit_members::<Diagnostic>(types, ty, argument.span, |_, _| Ok(()))?;
        }
        let result = match self {
            Self::TypeName => Type::String,
            Self::Fields => types.array(types.field_info_type(), span)?,
            Self::FieldNames | Self::VariantNames => types.array(Type::String, span)?,
        };
        let (name, name_span) = path.segments[1];
        checker.record(name_span, None, OccurrenceKind::Function, || {
            format!(
                "comptime fn compiler.{name}<T>() -> {}",
                types.describe(result)
            )
        });
        Ok((
            ExprKind::TypeQuery {
                query: self,
                ty,
                argument: argument.span,
            },
            result,
        ))
    }

    pub(super) fn visit_members<E: From<Diagnostic>>(
        self,
        types: &Structs<'_, '_>,
        ty: Type,
        span: Span,
        mut visit: impl FnMut(&str, Option<(Type, bool)>) -> Result<(), E>,
    ) -> Result<(), E> {
        if matches!(self, Self::VariantNames) {
            let Some(union) = types
                .definition_of(ty)
                .and_then(|id| types.definitions[id].union.as_ref())
            else {
                return Err(Diagnostic::new(
                    "E0501",
                    span,
                    "compiler.variant_names requires an Oxel enum",
                )
                .into());
            };
            for name in union.variant_names() {
                visit(name, None)?;
            }
            return Ok(());
        }

        let records = matches!(self, Self::Fields);
        let source = types.definition_of(ty).map(|id| &types.definitions[id]);
        if let Some(source) = source.filter(|def| def.union.is_none()) {
            for field in &source.fields {
                let details = if records {
                    Some((types.field_type(ty, field.ty, span)?, true))
                } else {
                    None
                };
                visit(&field.name, details)?;
            }
        } else if let Some(id) = types
            .ancestor_id(ty)
            .or_else(|| types.references.builtin_receiver(ty.non_nullable()))
        {
            let owner = types.owner_arguments(Some(ty));
            let owner = owner.as_deref().unwrap_or_default();
            for field in types.references.declared_instance_fields(id) {
                let details = if records {
                    let raw = field.ty.ok_or_else(|| {
                        Diagnostic::new(
                            "E0501",
                            span,
                            format!("unsupported CLR field type for '{}'", field.name),
                        )
                    })?;
                    Some((
                        types.import_signature_type(raw, owner, span)?,
                        field.readonly,
                    ))
                } else {
                    None
                };
                visit(&field.name, details)?;
            }
        } else {
            return Err(Diagnostic::new(
                "E0501",
                span,
                "compiler field queries require an Oxel struct or an imported CLR type",
            )
            .into());
        }
        Ok(())
    }
}

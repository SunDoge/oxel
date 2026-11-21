mod patterns;

use super::*;

pub(crate) struct Union<'src> {
    variants: Vec<Variant<'src>>,
    ids: HashMap<&'src str, usize>,
    pub(super) zero_variant: Option<usize>,
}

impl<'src> Union<'src> {
    pub(super) fn variant_names(&self) -> impl Iterator<Item = &'src str> + '_ {
        self.variants.iter().map(|variant| variant.name)
    }

    fn discriminant(&self) -> Discriminant {
        Discriminant::Tag {
            zero_variant: self.zero_variant,
        }
    }
}

struct Variant<'src> {
    name: &'src str,
    span: Span,
    fields: std::ops::Range<usize>,
}

pub(super) fn builtin_option() -> Struct<'static> {
    let span = Span::default();
    Struct {
        namespace: None,
        internal: false,
        name: "Option",
        builtin: true,
        interface: None,
        implementations: Vec::new(),
        fields: vec![
            Field {
                name: Cow::Borrowed("$tag"),
                ty: Type::I32,
                span,
            },
            Field {
                name: Cow::Borrowed("Some$0"),
                ty: Type::TypeParameter(0),
                span,
            },
        ],
        type_parameters: vec!["T"],
        bounds: Vec::new(),
        union: Some(Union {
            variants: vec![
                Variant {
                    name: "None",
                    span,
                    fields: 1..1,
                },
                Variant {
                    name: "Some",
                    span,
                    fields: 1..2,
                },
            ],
            ids: HashMap::from([("None", 0), ("Some", 1)]),
            zero_variant: Some(0),
        }),
        span,
        field_ids: HashMap::new(),
    }
}

impl<'src> Union<'src> {
    pub(super) fn collect(
        types: &Structs<'src, '_>,
        declaration: &syntax::TypeDefinition<'src>,
        source: &[syntax::Variant<'src>],
        bounds: &[GenericBound],
        fields: &mut Vec<Field<'src>>,
    ) -> Checked<Self> {
        if source.is_empty() {
            return Err(Diagnostic::new(
                "E0330",
                declaration.span,
                "an enum requires at least one variant",
            ));
        }
        // A CLR zero-initialized value must never expose an uninitialized payload.
        fields.push(Field {
            name: Cow::Borrowed("$tag"),
            ty: Type::I32,
            span: declaration.span,
        });
        let mut variants = Vec::with_capacity(source.len());
        let mut ids = HashMap::new();
        for variant in source {
            let id = variants.len();
            if variant.name == "_" || ids.insert(variant.name, id).is_some() {
                return Err(Diagnostic::new(
                    "E0330",
                    variant.span,
                    format!("invalid or duplicate variant '{}'", variant.name),
                ));
            }
            let start = fields.len();
            for (index, annotation) in variant.payload.iter().enumerate() {
                let ty = types.scoped_type(
                    annotation,
                    &declaration.type_parameters,
                    Some(bounds),
                    false,
                    true,
                )?;
                if !declaration.internal {
                    types.require_public_type(ty, annotation.span)?;
                }
                fields.push(Field {
                    name: Cow::Owned(format!("{}${index}", variant.name)),
                    ty,
                    span: annotation.span,
                });
            }
            variants.push(Variant {
                name: variant.name,
                span: variant.span,
                fields: start..fields.len(),
            });
        }
        Ok(Self {
            variants,
            ids,
            zero_variant: None,
        })
    }

    fn variant(&self, name: &str, span: Span) -> Checked<usize> {
        self.ids
            .get(name)
            .copied()
            .ok_or_else(|| Diagnostic::new("E0330", span, format!("unknown enum variant '{name}'")))
    }
}

pub(super) fn observe_declaration(
    types: &Structs<'_, '_>,
    definition: &Struct<'_>,
    source: &[syntax::Variant<'_>],
    type_parameters: &[(&str, Span)],
    occurrences: &mut Vec<Occurrence>,
) {
    let union = definition.union.as_ref().expect("enum definition");
    for (variant, source) in union.variants.iter().zip(source) {
        occurrences.push(Occurrence {
            span: variant.span,
            kind: OccurrenceKind::EnumMember,
            definition: None,
            definition_source: None,
            call_parameters: None,
            detail: format!("{}.{}", definition.name, variant.name),
            is_declaration: true,
            visibility: None,
        });
        for (field, annotation) in variant.fields.clone().zip(&source.payload) {
            observe_type(
                types,
                type_parameters,
                annotation,
                definition.fields[field].ty,
                &mut |span, definition, kind, detail| {
                    occurrences.push(Occurrence {
                        span,
                        kind,
                        definition,
                        definition_source: None,
                        call_parameters: None,
                        detail,
                        is_declaration: false,
                        visibility: None,
                    });
                },
            );
        }
    }
}

fn variant_tag(case: usize, span: Span) -> (usize, Expr) {
    (
        0,
        Expr {
            kind: ExprKind::Integer((case + 1) as i64),
            ty: Type::I32,
            span,
        },
    )
}

#[derive(Clone, Copy)]
pub(super) struct CarrierLayout {
    definition: usize,
    pub(super) success_case: usize,
    pub(super) success_field: usize,
    pub(super) failure_case: usize,
    pub(super) error_field: Option<usize>,
    pub(super) discriminant: Discriminant,
}

impl CarrierLayout {
    pub(super) fn for_type(types: &Structs<'_, '_>, ty: Type) -> Option<Self> {
        Self::for_definition(types, types.definition_of(ty)?)
    }

    fn for_definition(types: &Structs<'_, '_>, definition: usize) -> Option<Self> {
        let declaration = &types.definitions[definition];
        let union = declaration.union.as_ref()?;
        if union.variants.len() != 2 {
            return None;
        }
        let payload = |name, argument| {
            let case = *union.ids.get(name)?;
            let fields = &union.variants[case].fields;
            (fields.len() == 1
                && declaration.fields[fields.start].ty == Type::TypeParameter(argument))
            .then_some((case, fields.start))
        };
        let (success_case, success_field, failure_case, error_field) =
            match (declaration.name, declaration.type_parameters.len()) {
                ("Result", 2) => {
                    let (ok, value) = payload("Ok", 0)?;
                    let (err, error) = payload("Err", 1)?;
                    (ok, value, err, Some(error))
                }
                ("Option", 1) => {
                    let (some, value) = payload("Some", 0)?;
                    let none = *union.ids.get("None")?;
                    if !union.variants[none].fields.is_empty() {
                        return None;
                    }
                    (some, value, none, None)
                }
                _ => return None,
            };
        Some(Self {
            definition,
            success_case,
            success_field,
            failure_case,
            error_field,
            discriminant: union.discriminant(),
        })
    }

    fn name(self) -> &'static str {
        if self.error_field.is_some() {
            "Result"
        } else {
            "Option"
        }
    }
}

pub(super) struct TryContext {
    pub loop_depth: usize,
    layout: Option<CarrierLayout>,
    error: Option<Type>,
    used: bool,
}

impl<'src> Checker<'_, 'src> {
    pub(super) fn variant_constructor(
        &mut self,
        segments: &[(&'src str, Span)],
        type_arguments: &[Path<'src>],
        args: &[syntax::Expr<'src>],
        expected: Option<Type>,
    ) -> Checked<Option<(ExprKind, Type)>> {
        let [owner_segments @ .., (name, name_span)] = segments else {
            return Ok(None);
        };
        let Some(&(first, owner_span)) = owner_segments.first() else {
            return Ok(None);
        };
        let owner = super::path_name(owner_segments);
        self.track_source_type(first);
        let (name, name_span) = (*name, *name_span);
        let types = self.types;
        let Some(&id) = types.scopes.lookup(&types.names, &owner, owner_span) else {
            return Ok(None);
        };
        let definition = &types.definitions[id];
        let Some(union) = &definition.union else {
            return Ok(None);
        };
        let variant_id = union.variant(name, name_span)?;
        let variant = &union.variants[variant_id];
        if args.len() != variant.fields.len() {
            return Err(Diagnostic::new(
                "E0330",
                name_span,
                format!(
                    "'{owner}.{name}' expects {} payload values, found {}",
                    variant.fields.len(),
                    args.len()
                ),
            ));
        }
        let expected = if definition.builtin
            && type_arguments.is_empty()
            && let Some(payload) = expected.and_then(|ty| types.nullable_payload(ty))
        {
            Some(types.constructed(id, vec![payload], owner_span)?)
        } else {
            expected
        };
        let ty = if let Some(ty) = expected
            && type_arguments.is_empty()
            && types.definition_of(ty) == Some(id)
        {
            ty
        } else {
            let arity = definition.type_parameters.len();
            if type_arguments.len() != arity {
                return Err(Diagnostic::new(
                    "E0205",
                    owner_span,
                    format!("'{owner}' requires {arity} type arguments or an expected enum type"),
                ));
            }
            let mut arguments = Vec::with_capacity(arity);
            for path in type_arguments {
                let ty = self.scoped_value_type(path)?;
                self.record_annotation(path, ty);
                arguments.push(ty);
            }
            types.check_struct_bounds(
                id,
                |index| type_arguments.get(index).map_or(owner_span, |arg| arg.span),
                &arguments,
                self.bounds,
                self.type_parameters,
            )?;
            types.constructed(id, arguments, owner_span)?
        };
        self.record(
            owner_span,
            definition.source_span(definition.span),
            OccurrenceKind::Enum,
            || types.describe(ty),
        );
        self.record(
            name_span,
            definition.source_span(variant.span),
            OccurrenceKind::EnumMember,
            || format!("{owner}.{name}"),
        );
        let mut fields = Vec::with_capacity(args.len() + 1);
        fields.push(variant_tag(variant_id, name_span));
        let mut diverges = false;
        for (field, argument) in variant.fields.clone().zip(args) {
            let slot = &definition.fields[field];
            let expected = types.field_type(ty, slot.ty, argument.span)?;
            let value = self.expression(argument, Some(expected))?;
            diverges |= value.ty == Type::Never;
            fields.push((field, value));
        }
        let local = self.local(ty, owner_span)?;
        Ok(Some((
            ExprKind::Construct { ty, local, fields },
            if diverges { Type::Never } else { ty },
        )))
    }

    pub(super) fn try_block(
        &mut self,
        source: &syntax::Block<'src>,
        expected: Option<Type>,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        let types = self.types;
        let layout = expected.and_then(|ty| CarrierLayout::for_type(types, ty));
        let arguments = expected
            .and_then(|ty| ty.instance_id())
            .filter(|_| layout.is_some())
            .map(|id| types.instance(id).arguments);
        let expected_ok = arguments
            .as_ref()
            .and_then(|arguments| arguments.first())
            .copied();
        let outside = self.try_context.replace(TryContext {
            loop_depth: self.loop_depth,
            layout,
            error: arguments
                .as_ref()
                .and_then(|arguments| arguments.get(1))
                .copied(),
            used: false,
        });
        let body = self.block(source, expected_ok);
        let context = std::mem::replace(&mut self.try_context, outside).expect("active try block");
        let mut body = body?;
        if !context.used {
            return Err(Diagnostic::new(
                "E0334",
                span,
                "a try block requires a '?' expression",
            ));
        }
        let ok = expected_ok.unwrap_or(body.ty);
        if matches!(ok, Type::Void | Type::Never | Type::Null) {
            return Err(Diagnostic::new(
                "E0334",
                source.span,
                "a try block requires a success value or an explicit Result or Option type on a diverging path",
            ));
        }
        let layout = context.layout.ok_or_else(|| {
            Diagnostic::new(
                "E0334",
                span,
                "cannot infer the try block type; annotate its Result or Option type",
            )
        })?;
        let arguments = std::iter::once(ok).chain(context.error).collect::<Vec<_>>();
        types.check_struct_bounds(
            layout.definition,
            |_| span,
            &arguments,
            self.bounds,
            self.type_parameters,
        )?;
        let ty = types.constructed(layout.definition, arguments, span)?;
        if body.ty != Type::Never {
            let value = *body.tail.take().expect("try success has a value");
            body.tail = Some(Box::new(self.pack_carrier(
                ty,
                layout.success_case,
                Some((layout.success_field, value)),
                span,
            )?));
            body.ty = ty;
        }
        let error_local = context
            .error
            .map(|error| self.local(error, span))
            .transpose()?;
        let failure_payload =
            layout
                .error_field
                .zip(error_local.zip(context.error))
                .map(|(field, (local, ty))| {
                    (
                        field,
                        Expr {
                            kind: ExprKind::Load(Value::Local(local)),
                            ty,
                            span,
                        },
                    )
                });
        let failure = self.pack_carrier(ty, layout.failure_case, failure_payload, span)?;
        Ok((
            ExprKind::Try(Box::new(TryBlock {
                body,
                error_local,
                failure,
            })),
            ty,
        ))
    }

    fn pack_carrier(
        &mut self,
        ty: Type,
        case: usize,
        payload: Option<(usize, Expr)>,
        span: Span,
    ) -> Checked<Expr> {
        Ok(Expr {
            kind: ExprKind::Construct {
                ty,
                local: self.local(ty, span)?,
                fields: std::iter::once(variant_tag(case, span))
                    .chain(payload)
                    .collect(),
            },
            ty,
            span,
        })
    }

    pub(super) fn nullable_option(
        &mut self,
        ty: Type,
        expected: Option<Type>,
        span: Span,
    ) -> Checked<Option<(Type, CarrierLayout)>> {
        let types = self.types;
        let Some(underlying) = types.nullable_payload(ty) else {
            return Ok(None);
        };
        let definition = types.names["Option"];
        if expected.is_some_and(|expected| types.definition_of(expected) != Some(definition)) {
            return Ok(None);
        }
        self.track_source_type("Option");
        let Some(layout) = CarrierLayout::for_definition(types, definition)
            .filter(|layout| types.definitions[layout.definition].builtin)
        else {
            return Ok(None);
        };
        let carrier = if let Some(expected) = expected {
            expected
        } else {
            types.constructed(layout.definition, vec![underlying], span)?
        };
        let payload = types.field_type(carrier, Type::TypeParameter(0), span)?;
        self.require(underlying, payload, span)?;
        Ok(Some((carrier, layout)))
    }

    pub(super) fn pack_nullable_option(
        &mut self,
        value: Expr,
        carrier: Type,
        layout: CarrierLayout,
        span: Span,
    ) -> Checked<Expr> {
        let input = value.ty;
        let local = self.local(input, span)?;
        let load = |ty| Expr {
            kind: ExprKind::Load(Value::Local(local)),
            ty,
            span,
        };
        let (kind, ty) = self.null_test(Binary::Ne, load(input))?;
        let condition = Expr { kind, ty, span };
        let payload = if let Some(underlying) = self.types.nullable_underlying(input) {
            let member = self.require_nullable_member(input, "get_Value", span)?;
            let (kind, ty) = self.constrained_call(
                member,
                Box::new([]),
                load(input),
                Vec::new(),
                underlying,
                span,
            )?;
            Expr { kind, ty, span }
        } else {
            load(input.non_nullable())
        };
        let expected = self
            .types
            .field_type(carrier, Type::TypeParameter(0), span)?;
        let payload = self.coerce(payload, expected);
        self.pack_optional_value(value, local, (carrier, layout), condition, payload, span)
    }

    fn pack_optional_value(
        &mut self,
        value: Expr,
        local: u16,
        (carrier, layout): (Type, CarrierLayout),
        condition: Expr,
        payload: Expr,
        span: Span,
    ) -> Checked<Expr> {
        let some = self.pack_carrier(
            carrier,
            layout.success_case,
            Some((layout.success_field, payload)),
            span,
        )?;
        let none = self.pack_carrier(carrier, layout.failure_case, None, span)?;
        Ok(Expr {
            kind: ExprKind::Block(Block {
                statements: vec![Statement::DeclareLocal {
                    index: local,
                    value: Some(value),
                }],
                tail: Some(Box::new(Expr {
                    kind: ExprKind::If {
                        condition: Box::new(condition),
                        yes: Block {
                            statements: Vec::new(),
                            tail: Some(Box::new(some)),
                            ty: carrier,
                        },
                        no: Some(Box::new(none)),
                        result_local: None,
                    },
                    ty: carrier,
                    span,
                })),
                ty: carrier,
            }),
            ty: carrier,
            span,
        })
    }

    pub(super) fn optional_cast(
        &mut self,
        source: &syntax::Expr<'src>,
        target: &Path<'src>,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        if self.comptime {
            return Err(Diagnostic::new(
                "E0501",
                span,
                "'as?' requires runtime type information",
            ));
        }
        let value = self.expression(source, None)?;
        let ty = self.scoped_value_type(target)?;
        self.record_annotation(target, ty);
        if value.ty == Type::Never {
            return Ok((value.kind, Type::Never));
        }
        self.require_runtime_test_type(ty, target.span)?;
        self.require(value.ty, Type::NullableObject, source.span)?;

        self.track_source_type("Option");
        let definition = self.types.names["Option"];
        if !self.types.definitions[definition].builtin {
            return Err(Diagnostic::new(
                "E0330",
                span,
                "'as?' requires the standard Option type",
            ));
        }
        let layout =
            CarrierLayout::for_definition(self.types, definition).expect("standard Option");
        let carrier = self.types.constructed(definition, vec![ty], span)?;
        let local = self.local(Type::NullableObject, span)?;
        let load = || {
            Box::new(Expr {
                kind: ExprKind::Load(Value::Local(local)),
                ty: Type::NullableObject,
                span,
            })
        };
        let condition = Expr {
            kind: ExprKind::TypeTest(load(), ty),
            ty: Type::Bool,
            span,
        };
        let payload = Expr {
            kind: ExprKind::CheckedCast(load(), ty),
            ty,
            span,
        };
        let value = self.coerce(value, Type::NullableObject);
        let result =
            self.pack_optional_value(value, local, (carrier, layout), condition, payload, span)?;
        Ok((result.kind, result.ty))
    }

    pub(super) fn option_nullable(
        &self,
        actual: Type,
        expected: Type,
    ) -> Option<(CarrierLayout, Type)> {
        if !matches!(actual, Type::ConstructedStruct(_)) {
            return None;
        }
        let underlying = self.types.nullable_payload(expected)?;
        let (layout, payload) = self.standard_option_payload(actual)?;
        (underlying == payload && self.can_upcast(payload, expected)).then_some((layout, payload))
    }

    pub(super) fn standard_option_payload(&self, ty: Type) -> Option<(CarrierLayout, Type)> {
        let Type::ConstructedStruct(instance) = ty else {
            return None;
        };
        let types = self.types;
        let instance = types.instance(instance);
        let InstanceKind::Struct(definition) = instance.kind else {
            return None;
        };
        types.definitions[definition].builtin.then(|| {
            (
                CarrierLayout::for_definition(types, definition).expect("standard Option"),
                instance.arguments[0],
            )
        })
    }

    pub(super) fn unpack_nullable_option(
        &mut self,
        value: Expr,
        expected: Type,
        layout: CarrierLayout,
        payload: Type,
    ) -> Checked<Expr> {
        let span = value.span;
        let fallback = self.coerce(
            Expr {
                kind: ExprKind::Null,
                ty: Type::Null,
                span,
            },
            expected,
        );
        self.option_or(value, layout, payload, expected, fallback)
    }

    pub(super) fn option_or(
        &mut self,
        value: Expr,
        layout: CarrierLayout,
        payload: Type,
        expected: Type,
        fallback: Expr,
    ) -> Checked<Expr> {
        let span = value.span;
        let local = self.local(value.ty, span)?;
        let load = || Expr {
            kind: ExprKind::Load(Value::Local(local)),
            ty: value.ty,
            span,
        };
        let some = self.coerce(
            Expr {
                kind: ExprKind::Field(Box::new(load()), layout.success_field),
                ty: payload,
                span,
            },
            expected,
        );
        let mut targets = vec![Destination::Arm(1); 2];
        targets[layout.success_case] = Destination::Arm(0);
        Ok(Expr {
            kind: ExprKind::Match(Box::new(Match {
                decisions: vec![Decision {
                    value: load(),
                    discriminant: layout.discriminant,
                    literals: None,
                    targets,
                }],
                value,
                local,
                setups: Vec::new(),
                arms: [some, fallback]
                    .into_iter()
                    .map(|value| Block {
                        ty: value.ty,
                        statements: Vec::new(),
                        tail: Some(Box::new(value)),
                    })
                    .collect(),
                guards: Vec::new(),
                result_local: None,
            })),
            ty: expected,
            span,
        })
    }

    pub(super) fn propagate(
        &mut self,
        operand: &syntax::Expr<'src>,
        operator: Span,
    ) -> Checked<(ExprKind, Type)> {
        let types = self.types;
        let local_target = self.try_context.is_some();
        let function_layout = if local_target {
            None
        } else {
            Some(CarrierLayout::for_type(types, self.result).ok_or_else(|| {
                Diagnostic::new(
                    "E0333",
                    operator,
                    "'?' requires a function returning Result<T, E> or Option<T>",
                )
            })?)
        };
        let mut value = self.expression(operand, None)?;
        if let Some((carrier, layout)) = self.nullable_option(value.ty, None, operator)? {
            value = self.pack_nullable_option(value, carrier, layout, operator)?;
        }
        if let Some(context) = &mut self.try_context {
            context.used = true;
        }
        if value.ty == Type::Never {
            return Ok((value.kind, Type::Never));
        }
        let Type::ConstructedStruct(input) = value.ty else {
            return Err(propagation_operand_error(operator, function_layout));
        };
        let input = types.instance(input);
        let layout = CarrierLayout::for_type(types, value.ty)
            .ok_or_else(|| propagation_operand_error(operator, function_layout))?;
        let ok_type = input.arguments[0];
        let error_type = input.arguments.get(1).copied();
        let output_error = if let Some(context) = &mut self.try_context {
            let target = *context.layout.get_or_insert(layout);
            require_same_carrier(target, layout, operator)?;
            if context.error.is_none() {
                context.error = error_type;
            }
            context.error
        } else {
            require_same_carrier(function_layout.expect("function carrier"), layout, operator)?;
            let Type::ConstructedStruct(output) = self.result else {
                unreachable!("validated function carrier is constructed")
            };
            types.instance(output).arguments.get(1).copied()
        };
        if let Some((actual, expected)) = error_type.zip(output_error) {
            self.require(actual, expected, operator)?;
        }
        let type_parameters = self.type_parameters;
        self.record(operator, None, OccurrenceKind::Operator, || {
            let failure = if layout.error_field.is_some() {
                "Err"
            } else {
                "None"
            };
            format!(
                "Propagate {failure} or yield {}",
                describe_scoped(types, type_parameters, ok_type)
            )
        });

        let local = self.local(value.ty, operator)?;
        let load = || Expr {
            kind: ExprKind::Load(Value::Local(local)),
            ty: value.ty,
            span: operator,
        };
        let payload = |field, ty| Expr {
            kind: ExprKind::Field(Box::new(load()), field),
            ty,
            span: operator,
        };
        let error = layout.error_field.zip(error_type.zip(output_error)).map(
            |(field, (actual, expected))| (field, self.coerce(payload(field, actual), expected)),
        );
        let failure = if local_target {
            Statement::TryFailure(error.map(|(_, value)| value))
        } else {
            let value = if value.ty == self.result {
                load()
            } else {
                self.pack_carrier(self.result, layout.failure_case, error, operator)?
            };
            Statement::Return(Some(value))
        };
        let mut targets = vec![Destination::Arm(1); 2];
        targets[layout.success_case] = Destination::Arm(0);
        let decisions = vec![Decision {
            value: load(),
            discriminant: layout.discriminant,
            literals: None,
            targets,
        }];
        let arms = vec![
            Block {
                statements: Vec::new(),
                tail: Some(Box::new(payload(layout.success_field, ok_type))),
                ty: ok_type,
            },
            Block {
                statements: vec![failure],
                tail: None,
                ty: Type::Never,
            },
        ];
        Ok((
            ExprKind::Match(Box::new(Match {
                value,
                local,
                decisions,
                setups: Vec::new(),
                arms,
                guards: Vec::new(),
                result_local: None,
            })),
            ok_type,
        ))
    }
}

fn propagation_operand_error(span: Span, target: Option<CarrierLayout>) -> Diagnostic {
    let expected = match target.map(CarrierLayout::name) {
        Some("Result") => "a Result<T, E>",
        Some("Option") => "an Option<T>",
        _ => "a Result<T, E> or Option<T>",
    };
    Diagnostic::new("E0333", span, format!("'?' requires {expected} operand"))
}

fn require_same_carrier(target: CarrierLayout, operand: CarrierLayout, span: Span) -> Checked<()> {
    if target.definition == operand.definition {
        Ok(())
    } else {
        Err(Diagnostic::new(
            "E0333",
            span,
            format!(
                "'?' cannot propagate {} to {}",
                operand.name(),
                target.name()
            ),
        ))
    }
}

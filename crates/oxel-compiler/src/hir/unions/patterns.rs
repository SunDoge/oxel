use std::collections::VecDeque;

use super::*;
use crate::syntax::{Pattern, PatternKind};

const DECISION_BUDGET: usize = 1_000_000;

#[derive(Clone)]
struct Test {
    place: usize,
    case: Case,
}

#[derive(Clone)]
enum Case {
    Variant(usize),
    Literal(PatternLiteral),
}

#[derive(Clone)]
struct Row {
    arm: usize,
    priority: usize,
    setup: Option<usize>,
    tests: Vec<Test>,
    guarded: bool,
}

#[derive(Clone, Default)]
struct PatternPath<'src> {
    tests: Vec<Test>,
    names: Vec<(&'src str, Span, usize)>,
}

struct PatternPaths<'a, 'src> {
    alternatives: Vec<PatternPath<'src>>,
    budget: &'a mut usize,
}

impl PatternPaths<'_, '_> {
    fn charge(&mut self, cost: usize, span: Span) -> Checked<()> {
        *self.budget = self.budget.checked_sub(cost).ok_or_else(|| {
            Diagnostic::new("E0332", span, "match pattern analysis is too complex")
        })?;
        Ok(())
    }

    fn test(&mut self, place: usize, case: Case, span: Span) -> Checked<()> {
        self.charge(self.alternatives.len(), span)?;
        for path in &mut self.alternatives {
            path.tests.push(Test {
                place,
                case: case.clone(),
            });
        }
        Ok(())
    }
}

struct PatternMatrix {
    rows: Vec<Row>,
    setups: Vec<MatchSetup>,
    budget: usize,
}

impl PatternMatrix {
    fn new(arms: usize) -> Self {
        Self {
            rows: Vec::with_capacity(arms),
            setups: Vec::new(),
            budget: DECISION_BUDGET,
        }
    }
}

struct Place {
    ty: Type,
    projection: Option<(usize, usize)>,
}

struct Places {
    local: u16,
    values: Vec<Place>,
    fields: HashMap<(usize, usize), usize>,
}

impl Places {
    fn new(local: u16, ty: Type) -> Self {
        Self {
            local,
            values: vec![Place {
                ty,
                projection: None,
            }],
            fields: HashMap::new(),
        }
    }

    fn field(&mut self, parent: usize, field: usize, ty: Type) -> usize {
        *self.fields.entry((parent, field)).or_insert_with(|| {
            let id = self.values.len();
            self.values.push(Place {
                ty,
                projection: Some((parent, field)),
            });
            id
        })
    }

    fn expression(&self, place: usize, span: Span) -> Expr {
        let value = &self.values[place];
        let kind = if let Some((parent, field)) = value.projection {
            ExprKind::Field(Box::new(self.expression(parent, span)), field)
        } else {
            ExprKind::Load(Value::Local(self.local))
        };
        Expr {
            kind,
            ty: value.ty,
            span,
        }
    }
}

struct Bindings<'src> {
    names: Scope<'src>,
    stores: Vec<Statement>,
    visibility: Span,
}

impl<'src> Checker<'_, 'src> {
    fn pattern_value(
        &mut self,
        value: &syntax::Expr<'src>,
        expected: Option<Type>,
    ) -> Checked<Expr> {
        let value = self.expression(value, expected)?;
        let span = value.span;
        if let Some((carrier, layout)) = self.nullable_option(value.ty, None, span)? {
            self.pack_nullable_option(value, carrier, layout, span)
        } else {
            Ok(value)
        }
    }

    pub(in crate::hir) fn let_pattern(
        &mut self,
        binding: &syntax::LetPattern<'src>,
        scope_end: usize,
    ) -> Checked<Statement> {
        let expected = binding
            .annotation
            .as_ref()
            .map(|annotation| {
                let ty = self.scoped_value_type(annotation)?;
                self.record_annotation(annotation, ty);
                Ok(ty)
            })
            .transpose()?;
        let value = self.pattern_value(&binding.value, expected)?;
        let (mut selection, bindings) = self.pattern_selection(
            &binding.pattern,
            value,
            Span {
                start: binding.end,
                end: scope_end,
            },
            binding.fallback.as_ref().map(|_| "let-else"),
        )?;
        let fallback = binding
            .fallback
            .as_ref()
            .map(|source| {
                let fallback = self.block(source, None)?;
                if fallback.ty != Type::Never {
                    return Err(Diagnostic::new(
                        "E0335",
                        source.span,
                        "the else block must exit the current path",
                    ));
                }
                Ok(fallback)
            })
            .transpose()?;
        self.scopes
            .last_mut()
            .expect("block scope")
            .merge(bindings.names);
        selection.arms.push(Block {
            ty: Type::Void,
            statements: bindings.stores,
            tail: None,
        });
        selection.arms.extend(fallback);
        Ok(Statement::Discard(Expr {
            kind: ExprKind::Match(Box::new(selection)),
            ty: Type::Void,
            span: binding.pattern.span,
        }))
    }

    fn pattern_selection(
        &mut self,
        pattern: &Pattern<'src>,
        value: Expr,
        visibility: Span,
        failure: Option<&str>,
    ) -> Checked<(Match, Bindings<'src>)> {
        if !pattern_type(self.types, value.ty) {
            return Err(Diagnostic::new(
                "E0331",
                value.span,
                format!(
                    "{} requires an enum, struct, boolean, integer, character or string value",
                    failure.unwrap_or("let pattern"),
                ),
            ));
        }
        let local = self.local(value.ty, value.span)?;
        let mut places = Places::new(local, value.ty);
        let mut bindings = Bindings {
            names: Scope::Empty,
            stores: Vec::new(),
            visibility,
        };
        let mut matrix = PatternMatrix::new(2);
        self.pattern_rows(pattern, &mut places, &mut bindings, &mut matrix, 0, false)?;
        matrix.rows.push(Row {
            arm: 1,
            priority: matrix.rows.len(),
            setup: None,
            tests: Vec::new(),
            guarded: false,
        });
        let span = pattern.span;
        let (decisions, reachable) = decisions(self.types, &places, matrix.rows, span)?;
        if let Some(construct) = failure
            && !reachable[1]
        {
            return Err(Diagnostic::new(
                "E0335",
                span,
                format!("this pattern always matches; {construct} requires a possible failure"),
            ));
        }
        if failure.is_none() && reachable[1] {
            return Err(Diagnostic::new(
                "E0335",
                span,
                "this pattern can fail; add an else block",
            ));
        }
        Ok((
            Match {
                value,
                local,
                decisions,
                setups: matrix.setups,
                arms: Vec::new(),
                guards: Vec::new(),
                result_local: None,
            },
            bindings,
        ))
    }

    pub(in crate::hir) fn if_let(
        &mut self,
        binding: &syntax::IfLet<'src>,
        span: Span,
        expected: Option<Type>,
    ) -> Checked<(ExprKind, Type)> {
        let value = self.pattern_value(&binding.value, None)?;
        let (mut selection, bindings) = self.pattern_selection(
            &binding.pattern,
            value,
            binding.yes.interior(),
            Some("if let"),
        )?;
        let branch_expected = if binding.no.is_some() {
            expected
        } else {
            Some(Type::Void)
        };
        self.scopes.push(bindings.names);
        let yes = self.block_contents(&binding.yes, branch_expected);
        self.scopes.pop();
        let mut yes = yes?;
        let mut statements = bindings.stores;
        statements.append(&mut yes.statements);
        yes.statements = statements;

        let (ty, fallback) = if let Some(no) = &binding.no {
            let hint = self.else_hint(yes.ty, no, branch_expected);
            let no = self.expression(no, hint)?;
            let (ty, no) = self.join_branches(&mut yes, no)?;
            (
                ty,
                Block {
                    ty: no.ty,
                    statements: Vec::new(),
                    tail: Some(Box::new(no)),
                },
            )
        } else {
            (
                Type::Void,
                Block {
                    ty: Type::Void,
                    statements: Vec::new(),
                    tail: None,
                },
            )
        };
        if ty.external_reference() {
            selection.result_local = Some(self.local(ty, span)?);
        }
        selection.arms = vec![yes, fallback];
        Ok((ExprKind::Match(Box::new(selection)), ty))
    }

    pub(in crate::hir) fn while_let(
        &mut self,
        binding: &syntax::WhileLet<'src>,
    ) -> Checked<Statement> {
        let value = self.pattern_value(&binding.value, None)?;
        let (mut selection, bindings) = self.pattern_selection(
            &binding.pattern,
            value,
            binding.body.interior(),
            Some("while let"),
        )?;
        let span = binding.pattern.span;
        let arm = |matched, statements| Block {
            ty: Type::Bool,
            statements,
            tail: Some(Box::new(Expr {
                kind: ExprKind::Boolean(matched),
                ty: Type::Bool,
                span,
            })),
        };
        selection.arms = vec![arm(true, bindings.stores), arm(false, Vec::new())];

        self.loop_depth += 1;
        self.scopes.push(bindings.names);
        let body = self.block_contents(&binding.body, Some(Type::Void));
        self.scopes.pop();
        self.loop_depth -= 1;

        Ok(Statement::While {
            condition: Expr {
                kind: ExprKind::Match(Box::new(selection)),
                ty: Type::Bool,
                span,
            },
            body: body?,
        })
    }

    pub(in crate::hir) fn match_expression(
        &mut self,
        value: &syntax::Expr<'src>,
        source: &[syntax::MatchArm<'src>],
        span: Span,
        expected: Option<Type>,
    ) -> Checked<(ExprKind, Type)> {
        let value = self.pattern_value(value, None)?;
        let types = self.types;
        let root = union(types, value.ty);
        if !pattern_type(types, value.ty) {
            return Err(Diagnostic::new(
                "E0331",
                value.span,
                "match requires an enum, struct, boolean, integer, character or string value",
            ));
        }
        let local = self.local(value.ty, value.span)?;
        let mut places = Places::new(local, value.ty);
        let mut bindings = Vec::with_capacity(source.len());
        let mut matrix = PatternMatrix::new(source.len());
        for (arm, item) in source.iter().enumerate() {
            let mut bound = Bindings {
                names: Scope::Empty,
                stores: Vec::new(),
                visibility: Span {
                    start: item
                        .guard
                        .as_ref()
                        .map_or(item.arrow_end, |guard| guard.span.start),
                    end: item.value.span.end,
                },
            };
            let start = matrix.rows.len();
            self.pattern_rows(
                &item.pattern,
                &mut places,
                &mut bound,
                &mut matrix,
                arm,
                item.guard.is_some(),
            )?;
            if item.guard.is_none()
                && arm + 1 != source.len()
                && matrix.rows[start..].iter().any(|row| row.tests.is_empty())
            {
                return Err(wildcard_error(item.pattern.span));
            }
            bindings.push(bound);
        }

        let rows = matrix.rows;
        let runtime_rows = rows.iter().any(|row| row.guarded).then(|| rows.clone());
        let (decisions, reachable) = decisions(types, &places, rows, span)?;
        for (item, reachable) in source.iter().zip(reachable) {
            if !reachable {
                let wildcard = matches!(
                    &item.pattern.kind,
                    PatternKind::Name(path, None) if path.segments[0].0 == "_"
                );
                return Err(if wildcard {
                    wildcard_error(item.pattern.span)
                } else {
                    Diagnostic::new(
                        "E0331",
                        item.pattern.span,
                        "duplicate match arm or pattern already covered",
                    )
                });
            }
        }
        let missing = uncovered_cases(&decisions);
        if !missing.is_empty() {
            if types.definition_of(value.ty).is_some() && root.is_none() {
                return Err(Diagnostic::new(
                    "E0331",
                    span,
                    "non-exhaustive match, missing some struct field combinations",
                ));
            }
            let decision = &decisions[0];
            let names = missing
                .iter()
                .map(|&index| {
                    let name = if let Some((_, union)) = root {
                        union.variants[index].name.to_owned()
                    } else if let Some(literals) = &decision.literals {
                        literals.keys.get(index).map_or_else(
                            || "other values".into(),
                            |key| match key {
                                PatternLiteral::Integer(bits) => {
                                    decision.value.ty.integer_value(*bits).to_string()
                                }
                                PatternLiteral::Character(value) => format!("U+{value:04X}"),
                                PatternLiteral::String(value) => format!("{value:?}"),
                            },
                        )
                    } else {
                        ["false", "true"][index].to_owned()
                    };
                    if matches!(decision.targets.get(index), Some(Destination::Decision(_))) {
                        format!("{name} (some payload combinations)")
                    } else {
                        name
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Diagnostic::new(
                "E0331",
                span,
                format!("non-exhaustive match, missing {names}"),
            ));
        }

        let (decisions, fallbacks) = if let Some(rows) = runtime_rows {
            guarded_decisions(types, &places, &rows, span)?
        } else {
            (decisions, Vec::new())
        };
        let mut fallbacks = fallbacks.into_iter();
        let mut guards = Vec::new();
        let mut arms = Vec::with_capacity(source.len());
        let mut result = Type::Never;
        for (item, binding) in source.iter().zip(bindings) {
            let hint = expected
                .or_else(|| (!result.reference() && result != Type::Never).then_some(result));
            self.scopes.push(binding.names);
            let checked = (|| {
                let guard = item
                    .guard
                    .as_ref()
                    .map(|guard| self.expression(guard, Some(Type::Bool)))
                    .transpose()?;
                let refinements = guard
                    .as_ref()
                    .map_or_else(Vec::new, |guard| self.null_refinements(guard, true));
                let value = self
                    .with_nonnull(refinements, |checker| checker.expression(&item.value, hint))?;
                Ok((guard, value))
            })();
            self.scopes.pop();
            let (guard, value) = checked?;
            let mut stores = binding.stores;
            let ty = if let Some(guard) = guard {
                let ty = guard.ty;
                guards.push(MatchGuard {
                    arm: arms.len(),
                    condition: Block {
                        ty,
                        statements: std::mem::take(&mut stores),
                        tail: Some(Box::new(guard)),
                    },
                    fallback: fallbacks.next().expect("compiled guard fallback"),
                });
                if ty == Type::Never { ty } else { value.ty }
            } else {
                value.ty
            };
            result = self.join_types(result, ty, item.value.span)?;
            arms.push(Block {
                ty,
                statements: stores,
                tail: Some(Box::new(value)),
            });
        }
        for block in &mut arms {
            if block.ty != Type::Never && block.ty != result {
                let tail = block
                    .tail
                    .take()
                    .expect("a match arm has a result expression");
                block.tail = Some(Box::new(self.coerce(*tail, result)));
                block.ty = result;
            }
        }
        let result_local = if result.external_reference() {
            Some(self.local(result, span)?)
        } else {
            None
        };
        Ok((
            ExprKind::Match(Box::new(Match {
                value,
                local,
                decisions,
                setups: matrix.setups,
                arms,
                guards,
                result_local,
            })),
            result,
        ))
    }

    fn pattern_rows(
        &mut self,
        source: &Pattern<'src>,
        places: &mut Places,
        bindings: &mut Bindings<'src>,
        matrix: &mut PatternMatrix,
        arm: usize,
        guarded: bool,
    ) -> Checked<()> {
        let mut paths = PatternPaths {
            alternatives: vec![PatternPath::default()],
            budget: &mut matrix.budget,
        };
        self.pattern(source, 0, places, &mut paths)?;
        let record_alternatives = self.occurrences.is_some() && paths.alternatives.len() > 1;
        // Alternatives share local slots and an arm. Only different projections
        // need to initialize the slots before jumping to that arm.
        let first_names = &paths.alternatives[0].names;
        let binding_count = first_names.len();
        let per_path_stores = paths.alternatives[1..].iter().any(|path| {
            path.names.len() != binding_count
                || path.names.iter().zip(first_names).any(
                    |(&(name, _, place), &(first_name, _, first_place))| {
                        (name, place) != (first_name, first_place)
                    },
                )
        });
        let mut recorded = HashSet::new();
        for (index, path) in paths.alternatives.into_iter().enumerate() {
            let mut stores = Vec::new();
            if index == 0 {
                for (name, span, place) in &path.names {
                    self.bind_pattern(name, *span, places.expression(*place, *span), bindings)?;
                }
                if per_path_stores {
                    stores = std::mem::take(&mut bindings.stores);
                }
                if record_alternatives {
                    recorded.extend(path.names.iter().map(|(_, span, _)| span.start));
                }
            } else {
                let mismatch = |span| {
                    Diagnostic::new(
                        "E0331",
                        span,
                        "all alternatives must bind the same names with the same types",
                    )
                };
                if path.names.len() != binding_count {
                    return Err(mismatch(source.span));
                }
                let mut names = HashSet::new();
                for (name, span, place) in path.names {
                    if !names.insert(name) {
                        return Err(Diagnostic::new(
                            "E0331",
                            span,
                            format!("duplicate pattern binding '{name}'"),
                        ));
                    }
                    let binding = bindings.names.get(name).ok_or_else(|| mismatch(span))?;
                    if places.values[place].ty != binding.ty {
                        return Err(mismatch(span));
                    }
                    if record_alternatives && recorded.insert(span.start) {
                        let (types, parameters) = (self.types, self.type_parameters);
                        self.record(
                            span,
                            Some(binding.definition),
                            OccurrenceKind::Local { mutable: false },
                            || {
                                format!(
                                    "{name}: {}",
                                    describe_scoped(types, parameters, binding.ty)
                                )
                            },
                        );
                    }
                    if per_path_stores {
                        stores.push(Statement::Store {
                            target: binding.value,
                            value: places.expression(place, span),
                            span,
                        });
                    }
                }
            }
            let setup = (!stores.is_empty()).then(|| {
                let id = matrix.setups.len();
                matrix.setups.push(MatchSetup {
                    arm,
                    initialization: Block {
                        ty: Type::Void,
                        statements: stores,
                        tail: None,
                    },
                });
                id
            });
            matrix.rows.push(Row {
                arm,
                priority: matrix.rows.len(),
                setup,
                tests: path.tests,
                guarded,
            });
        }
        Ok(())
    }

    fn pattern(
        &mut self,
        pattern: &Pattern<'src>,
        place: usize,
        places: &mut Places,
        paths: &mut PatternPaths<'_, 'src>,
    ) -> Checked<()> {
        let types = self.types;
        let ty = places.values[place].ty;
        let (path, children) = match &pattern.kind {
            PatternKind::Or(alternatives) => {
                let input = std::mem::take(&mut paths.alternatives);
                let cost = input
                    .iter()
                    .map(|row| 1 + row.tests.len() + row.names.len())
                    .sum::<usize>();
                let (last, others) = alternatives.split_last().expect("pattern alternatives");
                let mut output = Vec::new();
                for alternative in others {
                    paths.charge(cost, alternative.span)?;
                    paths.alternatives = input.clone();
                    self.pattern(alternative, place, places, paths)?;
                    output.append(&mut paths.alternatives);
                }
                paths.alternatives = input;
                self.pattern(last, place, places, paths)?;
                output.append(&mut paths.alternatives);
                paths.alternatives = output;
                return Ok(());
            }
            PatternKind::Boolean(value) => {
                self.require(ty, Type::Bool, pattern.span)?;
                return paths.test(place, Case::Variant(usize::from(*value)), pattern.span);
            }
            PatternKind::Integer { digits, negative } => {
                if !ty.integer() {
                    return Err(Diagnostic::new(
                        "E0331",
                        pattern.span,
                        "integer patterns require an integer value",
                    ));
                }
                let bits = integer_literal(digits, *negative)
                    .filter(|&value| ty.contains_integer(value))
                    .ok_or_else(|| {
                        Diagnostic::new(
                            "E0303",
                            pattern.span,
                            format!("integer literal is out of range for {ty}"),
                        )
                    })?;
                return paths.test(
                    place,
                    Case::Literal(PatternLiteral::Integer(bits as i64)),
                    pattern.span,
                );
            }
            PatternKind::Character(value) => {
                self.require(ty, Type::Char, pattern.span)?;
                return paths.test(
                    place,
                    Case::Literal(PatternLiteral::Character(*value)),
                    pattern.span,
                );
            }
            PatternKind::String(value) => {
                self.require(ty, Type::NullableString, pattern.span)?;
                return paths.test(
                    place,
                    Case::Literal(PatternLiteral::String(Arc::from(value.as_ref()))),
                    pattern.span,
                );
            }
            PatternKind::Name(path, children) => (path, children),
            PatternKind::Struct { path, fields, rest } => {
                let Some(id) = types.definition_of(ty) else {
                    return Err(Diagnostic::new(
                        "E0331",
                        path.span,
                        "struct patterns require an Oxel struct value",
                    ));
                };
                let definition = &types.definitions[id];
                if definition.union.is_some()
                    || types.scopes.lookup(
                        &types.names,
                        &crate::hir::path_name(&path.segments),
                        path.span,
                    ) != Some(&id)
                {
                    return Err(Diagnostic::new(
                        "E0331",
                        path.span,
                        format!(
                            "struct pattern does not match value of type '{}'",
                            types.describe(ty)
                        ),
                    ));
                }
                self.track_source_type(definition.name);
                self.record_annotation(path, ty);
                let mut seen = HashSet::new();
                for &(name, span, ref child) in fields {
                    let (field, info) = definition.field(name, span)?;
                    if !seen.insert(field) {
                        return Err(Diagnostic::new(
                            "E0331",
                            span,
                            format!("duplicate pattern field '{name}'"),
                        ));
                    }
                    let field_type = types.field_type(ty, info.ty, span)?;
                    let parameters = self.type_parameters;
                    self.record(
                        span,
                        definition.source_span(info.span),
                        if span == child.span {
                            OccurrenceKind::ShorthandField
                        } else {
                            OccurrenceKind::Field { readonly: true }
                        },
                        || format!("{name}: {}", describe_scoped(types, parameters, field_type)),
                    );
                    let projection = places.field(place, field, field_type);
                    self.pattern(child, projection, places, paths)?;
                }
                if !rest && seen.len() != definition.fields.len() {
                    return Err(Diagnostic::new(
                        "E0331",
                        pattern.span,
                        "struct pattern omits fields; add '..' to ignore them",
                    ));
                }
                return Ok(());
            }
        };
        let (owner, name, name_span) = match path.segments.as_slice() {
            [("_", _)] if children.is_none() => return Ok(()),
            [(name, span)] => (None, *name, *span),
            [owner @ .., (name, span)] => (Some(owner), *name, *span),
            [] => unreachable!("nonempty pattern path"),
        };
        if place != 0 && children.is_none() && owner.is_none() {
            paths.charge(paths.alternatives.len(), pattern.span)?;
            for path in &mut paths.alternatives {
                path.names.push((name, name_span, place));
            }
            return Ok(());
        }
        let (definition, union) = union(types, ty).ok_or_else(|| {
            Diagnostic::new("E0331", path.span, "variant patterns require an enum value")
        })?;
        if let Some(owner) = owner {
            let span = owner.last().expect("qualified pattern owner").1;
            let resolved = types
                .scopes
                .lookup(&types.names, &crate::hir::path_name(owner), span)
                .copied();
            if resolved != types.definition_of(ty) {
                return Err(Diagnostic::new(
                    "E0331",
                    span,
                    format!("expected variant of '{}'", definition.name),
                ));
            }
            self.record(
                span,
                definition.source_span(definition.span),
                OccurrenceKind::Enum,
                || types.describe(ty),
            );
        }
        let case = union.variant(name, name_span)?;
        let variant = &union.variants[case];
        self.record(
            name_span,
            definition.source_span(variant.span),
            OccurrenceKind::EnumMember,
            || format!("{}.{name}", definition.name),
        );
        let children = children.as_deref().unwrap_or_default();
        if children.len() != variant.fields.len() {
            return Err(Diagnostic::new(
                "E0331",
                pattern.span,
                format!(
                    "pattern for '{}' requires {} payload patterns",
                    variant.name,
                    variant.fields.len()
                ),
            ));
        }
        paths.test(place, Case::Variant(case), pattern.span)?;
        for (field, child) in variant.fields.clone().zip(children) {
            let field_ty = types.field_type(ty, definition.fields[field].ty, child.span)?;
            let projection = places.field(place, field, field_ty);
            self.pattern(child, projection, places, paths)?;
        }
        Ok(())
    }

    fn bind_pattern(
        &mut self,
        name: &'src str,
        span: Span,
        value: Expr,
        bindings: &mut Bindings<'src>,
    ) -> Checked<()> {
        if bindings.names.get(name).is_some() {
            return Err(Diagnostic::new(
                "E0331",
                span,
                format!("duplicate pattern binding '{name}'"),
            ));
        }
        let ty = value.ty;
        let index = self.local(ty, span)?;
        bindings.names.insert(
            name,
            Binding {
                value: Value::Local(index),
                ty,
                mutable: false,
                uninitialized: false,
                can_make_mutable: false,
                definition: span,
            },
        );
        self.record_scoped_binding(
            name,
            span,
            OccurrenceKind::Local { mutable: false },
            ty,
            bindings.visibility,
        );
        bindings.stores.push(Statement::Store {
            target: Value::Local(index),
            value,
            span,
        });
        Ok(())
    }
}

fn union<'a, 'src>(
    types: &'a Structs<'src, '_>,
    ty: Type,
) -> Option<(&'a Struct<'src>, &'a Union<'src>)> {
    let definition = &types.definitions[types.definition_of(ty)?];
    Some((definition, definition.union.as_ref()?))
}

fn pattern_type(types: &Structs<'_, '_>, ty: Type) -> bool {
    ty.integer()
        || matches!(ty, Type::Bool | Type::Char | Type::String)
        || types.definition_of(ty).is_some()
}

fn wildcard_error(span: Span) -> Diagnostic {
    Diagnostic::new(
        "E0331",
        span,
        "wildcard must cover remaining variants in the last arm",
    )
}

fn decisions(
    types: &Structs<'_, '_>,
    places: &Places,
    rows: Vec<Row>,
    span: Span,
) -> Checked<(Vec<Decision>, Vec<bool>)> {
    let mut reachable = vec![false; rows.last().map_or(0, |row| row.arm + 1)];
    let mut budget = DECISION_BUDGET;
    let decisions = decision_tree(types, places, rows, span, &mut reachable, &mut budget)?;
    Ok((decisions, reachable))
}

fn guarded_decisions(
    types: &Structs<'_, '_>,
    places: &Places,
    rows: &[Row],
    span: Span,
) -> Checked<(Vec<Decision>, Vec<usize>)> {
    let mut decisions = Vec::new();
    let mut fallbacks = Vec::new();
    let mut reachable = vec![false; rows.last().map_or(0, |row| row.arm + 1)];
    let mut budget = DECISION_BUDGET;
    // Keep every alternative of an arm in the same group. A failed pattern or
    // a false guard resumes at the next group without rerunning that guard.
    for group in rows.chunk_by(|left, right| !left.guarded || left.arm == right.arm) {
        let mut group = group.to_vec();
        let last = group.last().expect("nonempty decision group");
        let guarded = last.guarded;
        if guarded {
            let arm = last.arm;
            for row in group.iter_mut().rev().take_while(|row| row.arm == arm) {
                row.guarded = false;
            }
        }
        let mut tree = decision_tree(types, places, group, span, &mut reachable, &mut budget)?;
        let offset = decisions.len();
        let fallback = offset + tree.len();
        for target in tree.iter_mut().flat_map(Decision::destinations_mut) {
            match target {
                Destination::Decision(index) => *index += offset,
                Destination::Invalid if guarded => *target = Destination::Decision(fallback),
                _ => {}
            }
        }
        if guarded {
            fallbacks.push(fallback);
        }
        decisions.append(&mut tree);
    }
    Ok((decisions, fallbacks))
}

fn decision_tree(
    types: &Structs<'_, '_>,
    places: &Places,
    rows: Vec<Row>,
    span: Span,
    reachable: &mut [bool],
    budget: &mut usize,
) -> Checked<Vec<Decision>> {
    let mut queue = VecDeque::from([(0, rows)]);
    let mut next = 1;
    let mut decisions = Vec::new();
    let complex = || Diagnostic::new("E0332", span, "match pattern analysis is too complex");

    // Rows stay in source order. A test's parent projections appear before it
    // in its row, so each decision reads only fields of already matched variants.
    // The queue assigns child decisions higher indexes than their parents.
    while let Some((place, rows)) = queue.pop_front() {
        let value = places.expression(place, span);
        let enum_type = union(types, value.ty);
        let count = enum_type.map_or(usize::from(value.ty == Type::Bool) * 2, |(_, union)| {
            union.variants.len()
        });
        *budget = budget.checked_sub(count).ok_or_else(complex)?;
        let mut cases = vec![Vec::new(); count];
        let mut default = Vec::new();
        let mut keys = Vec::new();
        let mut literal_indices = HashMap::new();
        for mut row in rows {
            *budget = budget
                .checked_sub(row.tests.len() + 1)
                .ok_or_else(complex)?;
            if let Some(index) = row.tests.iter().position(|test| test.place == place) {
                let test = row.tests.remove(index);
                let index = match test.case {
                    Case::Variant(index) => index,
                    Case::Literal(literal) => {
                        *literal_indices.entry(literal.clone()).or_insert_with(|| {
                            keys.push(literal);
                            cases.push(Vec::new());
                            cases.len() - 1
                        })
                    }
                };
                cases[index].push(row);
            } else {
                default.push(row);
            }
        }

        // Rows that do not test this place apply to every case. Cases without
        // their own tests share a destination for those rows.
        let sparse = enum_type.is_none() && value.ty != Type::Bool;
        let complete = value
            .ty
            .shift_mask()
            .map(|mask| 1u128 << (mask + 1))
            .or_else(|| (value.ty == Type::Char).then_some(65_536));
        let other = sparse && complete.is_none_or(|count| (keys.len() as u128) < count);
        let default_target = if other || cases.iter().any(Vec::is_empty) {
            destination(default.clone(), &mut queue, &mut next, reachable)
        } else {
            Destination::Invalid
        };
        let mut targets = Vec::with_capacity(count);
        let default_cost = default.iter().map(|row| row.tests.len() + 1).sum::<usize>();
        for mut rows in cases {
            if rows.is_empty() {
                targets.push(default_target);
            } else {
                if !default.is_empty() {
                    *budget = budget.checked_sub(default_cost).ok_or_else(complex)?;
                    rows.extend(default.iter().cloned());
                    rows.sort_unstable_by_key(|row| row.priority);
                }
                targets.push(destination(rows, &mut queue, &mut next, reachable));
            }
        }
        decisions.push(Decision {
            value,
            discriminant: enum_type.map_or(Discriminant::Value, |(_, union)| union.discriminant()),
            targets,
            literals: sparse.then_some(LiteralCases {
                keys,
                other: other.then_some(default_target),
            }),
        });
    }
    Ok(decisions)
}

fn destination(
    mut rows: Vec<Row>,
    queue: &mut VecDeque<(usize, Vec<Row>)>,
    next: &mut usize,
    reachable: &mut [bool],
) -> Destination {
    let remaining = rows
        .iter()
        .take_while(|row| row.guarded && row.tests.is_empty())
        .count();
    for row in rows.drain(..remaining) {
        reachable[row.arm] = true;
    }
    let Some(first) = rows.first() else {
        return Destination::Invalid;
    };
    let Some(test) = first.tests.first() else {
        reachable[first.arm] = true;
        return first
            .setup
            .map_or(Destination::Arm(first.arm), Destination::Setup);
    };
    let node = *next;
    *next += 1;
    queue.push_back((test.place, rows));
    Destination::Decision(node)
}

fn uncovered_cases(decisions: &[Decision]) -> Vec<usize> {
    let mut uncovered = vec![false; decisions.len()];
    let missing = |target, uncovered: &[bool]| match target {
        Destination::Invalid => true,
        Destination::Arm(_) | Destination::Setup(_) => false,
        Destination::Decision(id) => uncovered[id],
    };
    for id in (1..decisions.len()).rev() {
        uncovered[id] = decisions[id]
            .destinations()
            .any(|&target| missing(target, &uncovered));
    }
    decisions[0]
        .destinations()
        .enumerate()
        .filter_map(|(index, &target)| missing(target, &uncovered).then_some(index))
        .collect()
}

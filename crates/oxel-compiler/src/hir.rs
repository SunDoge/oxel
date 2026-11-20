mod arrays;
mod assignment;
mod bounds;
mod builtin_types;
mod constants;
mod hierarchy;
mod indexers;
mod inference;
mod iteration;
mod lambdas;
mod nullability;
mod test_harness;
mod unions;

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) use arrays::ArrayAccess;
pub use builtin_types::Type;
pub(crate) use constants::EvaluationCache;
use hierarchy::DataMember;
pub(crate) use hierarchy::Member;
pub(crate) use indexers::IndexUpdate;
use iteration::local_interface_call;

use crate::{
    ArgumentMode, AssemblyKind, CallableSignature, Diagnostic, DiagnosticFix, Occurrence,
    OccurrenceKind, References, Span,
    analysis::{self, Lookup, schedule},
    clr::{
        EventAccess, MetadataConstant, NullabilityOverride, PropertyAccess, TypeBound,
        TypeBoundKind,
    },
    syntax::{self, Binary, Expression, LoopControl, Path, TypeSuffix, Unary},
};

#[derive(Clone, Copy)]
enum ParameterKind {
    Struct,
    Method,
    ImportedMethod,
}

impl ParameterKind {
    fn index(self, ty: Type) -> Option<u16> {
        match (self, ty.non_nullable()) {
            (Self::Struct, Type::TypeParameter(index))
            | (Self::Method, Type::MethodParameter(index))
            | (Self::ImportedMethod, Type::ImportedMethodParameter(index)) => Some(index),
            _ => None,
        }
    }
}

impl Type {
    pub(crate) fn scalar(self) -> bool {
        self.numeric() || matches!(self, Self::Bool | Self::Char)
    }

    pub(crate) fn imported_value(self) -> bool {
        matches!(self, Self::ClrEnum(_)) || self.clr_struct()
    }

    pub(crate) fn clr_struct(self) -> bool {
        matches!(self, Self::ClrStruct(_) | Self::ConstructedClrStruct(_))
    }

    fn integer(self) -> bool {
        self.shift_mask().is_some()
    }

    pub(crate) fn shift_mask(self) -> Option<u32> {
        match self {
            Self::I8 | Self::U8 => Some(7),
            Self::I16 | Self::U16 => Some(15),
            Self::I32 | Self::U32 => Some(31),
            Self::I64 | Self::U64 => Some(63),
            _ => None,
        }
    }

    fn bitwise_integer(self) -> bool {
        self.integer() || matches!(self, Self::ClrEnum(_))
    }
    fn narrow_integer(self) -> bool {
        matches!(self, Self::I8 | Self::U8 | Self::I16 | Self::U16)
    }

    pub(crate) fn wide_integer(self) -> bool {
        matches!(self, Self::I64 | Self::U64)
    }

    pub(crate) fn unsigned(self) -> bool {
        matches!(self, Self::U8 | Self::U16 | Self::U32 | Self::U64)
    }

    fn integer_value(self, bits: i64) -> i128 {
        if self == Self::U64 {
            i128::from(bits as u64)
        } else {
            i128::from(bits)
        }
    }

    fn contains_integer(self, value: i128) -> bool {
        match self {
            Self::I8 => i8::try_from(value).is_ok(),
            Self::U8 => u8::try_from(value).is_ok(),
            Self::I16 => i16::try_from(value).is_ok(),
            Self::U16 => u16::try_from(value).is_ok(),
            Self::I32 => i32::try_from(value).is_ok(),
            Self::U32 => u32::try_from(value).is_ok(),
            Self::I64 => i64::try_from(value).is_ok(),
            Self::U64 => u64::try_from(value).is_ok(),
            _ => false,
        }
    }

    pub(crate) fn floating(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }

    pub(crate) fn reference(self) -> bool {
        matches!(
            self,
            Self::Object
                | Self::String
                | Self::External(_)
                | Self::ConstructedExternal(_)
                | Self::Array(_)
                | Self::Null
        ) || self.is_nullable()
    }

    pub(crate) fn instance_id(self) -> Option<usize> {
        match self {
            Self::ConstructedStruct(id)
            | Self::ConstructedClrStruct(id)
            | Self::ConstructedExternal(id)
            | Self::NullableConstructedExternal(id)
            | Self::Array(id)
            | Self::NullableArray(id) => Some(id),
            _ => None,
        }
    }

    fn external_reference(self) -> bool {
        matches!(
            self.non_nullable(),
            Self::External(_) | Self::ConstructedExternal(_)
        )
    }

    fn numeric(self) -> bool {
        self.integer() || self.floating()
    }
}

impl fmt::Display for Type {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::NullableString => "System.String?",
            Self::NullableObject => "System.Object?",
            Self::NullableExternal(id) => return write!(out, "CLR reference type #{id}?"),
            Self::Null => "null",
            Self::Struct(id) => return write!(out, "struct #{id}"),
            Self::ClrEnum(id) => return write!(out, "CLR enum #{id}"),
            Self::ClrStruct(id) => return write!(out, "CLR struct #{id}"),
            Self::ConstructedClrStruct(id) => return write!(out, "constructed CLR struct #{id}"),
            Self::ConstructedStruct(id) => return write!(out, "constructed struct #{id}"),
            Self::ConstructedExternal(id) => return write!(out, "constructed CLR reference #{id}"),
            Self::NullableConstructedExternal(id) => {
                return write!(out, "constructed CLR reference #{id}?");
            }
            Self::Array(id) => return write!(out, "CLR array #{id}"),
            Self::NullableArray(id) => return write!(out, "CLR array #{id}?"),
            Self::ImportedArray(id) => return write!(out, "CLR array signature #{id}"),
            Self::NullableImportedArray(id) => return write!(out, "CLR array signature #{id}?"),
            Self::ImportedInstance(id) => return write!(out, "CLR signature template #{id}"),
            Self::NullableImportedInstance(id) => {
                return write!(out, "CLR signature template #{id}?");
            }
            Self::TypeParameter(index) => return write!(out, "struct type parameter #{index}"),
            Self::NullableTypeParameter(index) => {
                return write!(out, "struct type parameter #{index}?");
            }
            Self::External(id) => return write!(out, "CLR reference type #{id}"),
            Self::ImportedMethodParameter(index) => {
                return write!(out, "CLR method parameter #{index}");
            }
            Self::NullableImportedMethodParameter(index) => {
                return write!(out, "CLR method parameter #{index}?");
            }
            Self::MethodParameter(index) => return write!(out, "method type parameter #{index}"),
            Self::Never => "diverging expression",
            ty => ty.builtin_clr_name().expect("CLR built-in type name"),
        })
    }
}

pub(crate) struct Program<'src> {
    pub functions: Vec<Function<'src>>,
    pub structs: Vec<Struct<'src>>,
    pub instances: Vec<TypeInstance>,
    pub entry: Option<usize>,
    pub kind: AssemblyKind,
    pub test_filter: Option<Box<str>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum InstanceKind {
    Struct(usize),
    External(usize),
    Array,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct TypeInstance {
    pub kind: InstanceKind,
    pub arguments: Arc<[Type]>,
    depth: usize,
    size: usize,
}

#[derive(Default)]
struct TypeInstances {
    values: Vec<TypeInstance>,
    ids: HashMap<TypeInstance, usize>,
}

pub(crate) struct Struct<'src> {
    pub internal: bool,
    pub name: &'src str,
    pub builtin: bool,
    pub fields: Vec<Field<'src>>,
    pub type_parameters: Vec<&'src str>,
    pub bounds: Vec<GenericBound>,
    pub union: Option<unions::Union<'src>>,
    span: Span,
    field_ids: HashMap<&'src str, usize>,
}

pub(crate) struct Field<'src> {
    pub name: Cow<'src, str>,
    pub ty: Type,
    span: Span,
}

impl Struct<'_> {
    fn source_span(&self, span: Span) -> Option<Span> {
        (!self.builtin).then_some(span)
    }

    fn field(&self, name: &str, span: Span) -> Checked<(usize, &Field<'_>)> {
        let id = *self.field_ids.get(name).ok_or_else(|| {
            Diagnostic::new(
                "E0305",
                span,
                format!("'{}' has no field '{name}'", self.name),
            )
        })?;
        Ok((id, &self.fields[id]))
    }
}

struct Structs<'src, 'refs> {
    references: &'refs References,
    names: HashMap<&'src str, usize>,
    definitions: Vec<Struct<'src>>,
    arities: Vec<usize>,
    instances: RwLock<TypeInstances>,
    layouts_ready: bool,
    deferred_constructor_checks: AtomicBool,
    field_info: usize,
    field_info_used: AtomicBool,
}

impl<'src, 'refs> Structs<'src, 'refs> {
    fn collect(
        declarations: &[&syntax::TypeDefinition<'src>],
        references: &'refs References,
    ) -> Checked<Self> {
        let mut names = HashMap::new();
        for (id, declaration) in declarations.iter().enumerate() {
            if declaration.name == "Program"
                || builtin_type(&[(declaration.name, declaration.span)]).is_some()
            {
                return Err(Diagnostic::new(
                    "E0201",
                    declaration.span,
                    format!("reserved type name '{}'", declaration.name),
                ));
            }
            if names.insert(declaration.name, id).is_some() {
                return Err(Diagnostic::new(
                    "E0201",
                    declaration.span,
                    format!("duplicate type '{}'", declaration.name),
                ));
            }
        }
        let builtin_option = !names.contains_key("Option");
        if builtin_option {
            names.insert("Option", declarations.len());
        }
        let mut types = Self {
            references,
            names,
            definitions: Vec::new(),
            arities: declarations
                .iter()
                .map(|item| item.type_parameters.len())
                .chain(builtin_option.then_some(1))
                .chain(std::iter::once(0))
                .collect(),
            instances: RwLock::default(),
            layouts_ready: false,
            deferred_constructor_checks: AtomicBool::new(false),
            field_info: declarations.len() + usize::from(builtin_option),
            field_info_used: AtomicBool::new(false),
        };
        for declaration in declarations {
            validate_type_parameters(&declaration.type_parameters, declaration.span)?;
            let bounds = bounds::resolve(
                &declaration.type_parameters,
                &declaration.type_bounds,
                &types,
                true,
            )?;
            types.definitions.push(Struct {
                internal: declaration.internal,
                name: declaration.name,
                builtin: false,
                span: declaration.span,
                fields: Vec::new(),
                union: None,
                type_parameters: declaration
                    .type_parameters
                    .iter()
                    .map(|&(name, _)| name)
                    .collect(),
                bounds,
                field_ids: HashMap::new(),
            });
        }
        if builtin_option {
            types.definitions.push(unions::builtin_option());
        }
        types.definitions.push(constants::builtin_field_info());
        for (id, declaration) in declarations.iter().enumerate() {
            let bounds = &types.definitions[id].bounds;
            bounds::validate_arguments(
                &declaration.type_parameters,
                &declaration.type_bounds,
                bounds,
                &types,
                true,
            )?;
            if !declaration.internal {
                types.require_public_bounds(bounds, &declaration.type_bounds)?;
            }
            let mut field_ids = HashMap::new();
            let mut fields = Vec::new();
            let union = match &declaration.shape {
                syntax::TypeShape::Struct(source_fields) => {
                    for (name, span, annotation) in source_fields {
                        if field_ids.insert(*name, fields.len()).is_some() {
                            return Err(Diagnostic::new(
                                "E0201",
                                *span,
                                format!("duplicate field '{name}' in '{}'", declaration.name),
                            ));
                        }
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
                            name: Cow::Borrowed(name),
                            ty,
                            span: *span,
                        });
                    }
                    None
                }
                syntax::TypeShape::Union(variants) => Some(unions::Union::collect(
                    &types,
                    declaration,
                    variants,
                    bounds,
                    &mut fields,
                )?),
            };
            let definition = &mut types.definitions[id];
            definition.fields = fields;
            definition.union = union;
            definition.field_ids = field_ids;
        }
        types.validate_layouts()?;
        types.layouts_ready = true;
        if *types.deferred_constructor_checks.get_mut() {
            // A type used as a constructor argument can be declared later.
            // Recheck annotations once all field types and enum tags are known.
            for (id, declaration) in declarations.iter().enumerate() {
                let bounds = &types.definitions[id].bounds;
                let parameters = &declaration.type_parameters;
                bounds::validate_arguments(
                    parameters,
                    &declaration.type_bounds,
                    bounds,
                    &types,
                    true,
                )?;
                let check = |annotation| {
                    types.scoped_type(annotation, parameters, Some(bounds), false, true)
                };
                match &declaration.shape {
                    syntax::TypeShape::Struct(fields) => {
                        for (_, _, annotation) in fields {
                            check(annotation)?;
                        }
                    }
                    syntax::TypeShape::Union(variants) => {
                        for annotation in variants.iter().flat_map(|variant| &variant.payload) {
                            check(annotation)?;
                        }
                    }
                }
            }
        }
        Ok(types)
    }

    fn require_public_type(&self, ty: Type, span: Span) -> Checked<()> {
        if let Some(id) = self.definition_of(ty)
            && let definition = &self.definitions[id]
            && definition.internal
        {
            return Err(Diagnostic::new(
                "E0342",
                span,
                format!(
                    "public CLR declaration exposes internal type '{}'",
                    definition.name
                ),
            )
            .with_related(definition.span, "internal type declared here"));
        }
        if let Some(id) = ty.instance_id() {
            for &argument in self.instance(id).arguments.iter() {
                self.require_public_type(argument, span)?;
            }
        }
        Ok(())
    }

    fn require_native_type(&self, ty: Type, span: Span, context: &str) -> Checked<()> {
        if ty.numeric() {
            return Ok(());
        }

        // Source value layouts have already been checked for cycles. Visit each
        // reachable definition once, including when several fields share it.
        let mut pending = vec![(ty, None)];
        let mut seen = HashSet::new();
        while let Some((ty, field)) = pending.pop() {
            if ty.numeric() || !seen.insert(ty) {
                continue;
            }
            if let Type::Struct(id) = ty {
                let definition = &self.definitions[id];
                if !definition.builtin
                    && definition.union.is_none()
                    && !definition.fields.is_empty()
                {
                    pending.extend(
                        definition
                            .fields
                            .iter()
                            .rev()
                            .map(|field| (field.ty, Some((definition.name, field)))),
                    );
                    continue;
                }
            }

            let type_name = self.describe(ty);
            let error = Diagnostic::new(
                "E0343",
                span,
                format!(
                    "{context} require fixed-width numeric types or nonempty nongeneric Oxel structs of such types, found '{type_name}'"
                ),
            );
            return Err(if let Some((owner, field)) = field {
                error.with_related(
                    field.span,
                    format!(
                        "field '{owner}.{}' has unsupported native type '{type_name}'",
                        field.name
                    ),
                )
            } else {
                error
            });
        }
        Ok(())
    }

    fn require_public_bounds(
        &self,
        bounds: &[GenericBound],
        source: &[syntax::TypeBound<'_>],
    ) -> Checked<()> {
        for (bound, source) in bounds.iter().zip(source) {
            if let TypeBoundKind::Target(target) = bound.kind {
                self.require_public_type(target, source.span)?;
            }
        }
        Ok(())
    }

    fn validate_layouts(&self) -> Checked<()> {
        struct Layout {
            definition: usize,
            pending: Vec<Type>,
            used_parameters: Vec<bool>,
        }
        let start = |id: usize| Layout {
            definition: id,
            pending: self.definitions[id]
                .fields
                .iter()
                .rev()
                .map(|field| field.ty)
                .collect(),
            used_parameters: vec![false; self.arities[id]],
        };

        // Summarize which arguments contribute to a struct's size. An unused
        // parameter can refer back to the enclosing type without embedding it.
        let mut completed: Vec<Option<Vec<bool>>> = vec![None; self.definitions.len()];
        let mut active = vec![false; self.definitions.len()];
        for root in 0..self.definitions.len() {
            if completed[root].is_some() {
                continue;
            }
            let mut stack = vec![start(root)];
            active[root] = true;
            while let Some(frame) = stack.last_mut() {
                let Some(ty) = frame.pending.pop() else {
                    let finished = stack.pop().expect("active layout");
                    active[finished.definition] = false;
                    completed[finished.definition] = Some(finished.used_parameters);
                    continue;
                };
                if let Type::TypeParameter(index) = ty {
                    frame.used_parameters[index as usize] = true;
                    continue;
                }
                if let Type::ConstructedClrStruct(id) = ty {
                    // Reference assemblies can omit private fields. Treat each
                    // value argument as a potential part of the foreign layout.
                    frame
                        .pending
                        .extend(self.instance(id).arguments.iter().rev().copied());
                    continue;
                }
                let Some(inner) = self.definition_of(ty) else {
                    continue;
                };
                if let Some(parameters) = &completed[inner] {
                    if let Some(id) = ty.instance_id() {
                        let instance = self.instance(id);
                        frame.pending.extend(
                            instance
                                .arguments
                                .iter()
                                .zip(parameters)
                                .rev()
                                .filter_map(|(&ty, &used)| used.then_some(ty)),
                        );
                    }
                } else if active[inner] {
                    let definition = &self.definitions[inner];
                    return Err(Diagnostic::new(
                        "E0304",
                        definition.span,
                        format!(
                            "{} '{}' has a recursive value-type layout",
                            if definition.union.is_some() {
                                "enum"
                            } else {
                                "struct"
                            },
                            definition.name
                        ),
                    ));
                } else {
                    frame.pending.push(ty);
                    active[inner] = true;
                    stack.push(start(inner));
                }
            }
        }
        Ok(())
    }

    fn instance(&self, id: usize) -> TypeInstance {
        self.instances.read().expect("type interner").values[id].clone()
    }

    fn definition_of(&self, ty: Type) -> Option<usize> {
        match ty {
            Type::Struct(id) => Some(id),
            Type::ConstructedStruct(id) => match self.instance(id).kind {
                InstanceKind::Struct(definition) => Some(definition),
                _ => unreachable!("source struct instance"),
            },
            _ => None,
        }
    }

    fn constructed(&self, definition: usize, arguments: Vec<Type>, span: Span) -> Checked<Type> {
        self.constructed_type(InstanceKind::Struct(definition), arguments, span)
    }

    fn constructed_type(
        &self,
        kind: InstanceKind,
        arguments: Vec<Type>,
        span: Span,
    ) -> Checked<Type> {
        let external_value = matches!(kind, InstanceKind::External(definition) if self.references.imported_type(definition).value_struct);
        if arguments.is_empty() {
            return Ok(match kind {
                InstanceKind::External(definition) if external_value => Type::ClrStruct(definition),
                InstanceKind::External(definition) => Type::External(definition),
                InstanceKind::Struct(definition) => Type::Struct(definition),
                InstanceKind::Array => unreachable!("array element type"),
            });
        }
        let pool = self.instances.read().expect("type interner");
        let depth = 1 + arguments
            .iter()
            .filter_map(|ty| ty.instance_id().map(|id| pool.values[id].depth))
            .max()
            .unwrap_or(0);
        if depth > syntax::MAX_DEPTH {
            return Err(Diagnostic::new(
                "E0309",
                span,
                "constructed type nesting limit exceeded",
            ));
        }
        let size = 1 + arguments
            .iter()
            .map(|ty| ty.instance_id().map_or(1, |id| pool.values[id].size))
            .sum::<usize>();
        // Bound expanded signatures even when many nodes share one interned argument.
        if size > 16_384 {
            return Err(Diagnostic::new(
                "E0309",
                span,
                "constructed type exceeds the signature complexity limit",
            ));
        }
        let instance = TypeInstance {
            kind,
            arguments: arguments.into(),
            depth,
            size,
        };
        let id = if let Some(&id) = pool.ids.get(&instance) {
            id
        } else {
            drop(pool);
            let mut pool = self.instances.write().expect("type interner");
            let next = pool.values.len();
            let id = *pool.ids.entry(instance.clone()).or_insert(next);
            if id == next {
                pool.values.push(instance);
            }
            id
        };
        Ok(match kind {
            InstanceKind::External(_) if external_value => Type::ConstructedClrStruct(id),
            InstanceKind::External(_) => Type::ConstructedExternal(id),
            InstanceKind::Struct(_) => Type::ConstructedStruct(id),
            InstanceKind::Array => Type::Array(id),
        })
    }

    fn specialize(
        &self,
        ty: Type,
        arguments: &[Type],
        kind: ParameterKind,
        span: Span,
    ) -> Checked<Type> {
        if arguments.is_empty() {
            return Ok(ty);
        }
        Ok(self
            .substitute_parameters(ty, kind, span, |index| Some(arguments[index as usize]))?
            .expect("complete type arguments"))
    }

    fn substitute_parameters(
        &self,
        ty: Type,
        kind: ParameterKind,
        span: Span,
        argument: impl Fn(u16) -> Option<Type> + Copy,
    ) -> Checked<Option<Type>> {
        if let Some(index) = kind.index(ty) {
            return argument(index)
                .map(|argument| nullability::specialize_annotation(ty, argument, span))
                .transpose();
        }
        match ty {
            Type::ConstructedStruct(id)
            | Type::ConstructedClrStruct(id)
            | Type::ConstructedExternal(id)
            | Type::NullableConstructedExternal(id)
            | Type::Array(id)
            | Type::NullableArray(id) => {
                let instance = self.instance(id);
                let mut resolved_arguments = Vec::with_capacity(instance.arguments.len());
                for &arg in instance.arguments.iter() {
                    let Some(arg) = self.substitute_parameters(arg, kind, span, argument)? else {
                        return Ok(None);
                    };
                    resolved_arguments.push(arg);
                }
                let resolved = self.constructed_type(instance.kind, resolved_arguments, span)?;
                Ok(Some(if ty.is_nullable() {
                    resolved.nullable().expect("nullable instance")
                } else {
                    resolved
                }))
            }
            _ => Ok(Some(ty)),
        }
    }

    fn method_signature(&self, member: Member, span: Span) -> Checked<Signature> {
        let method = self.references.method(member.id);
        let raw = &method.signature;
        let owner_arguments = self.owner_arguments(member.owner);
        let owner = owner_arguments.as_deref().unwrap_or_default();
        Ok(Signature {
            type_arity: raw.type_arity,
            bounds: Vec::new(),
            parameters: raw
                .parameters
                .iter()
                .enumerate()
                .map(|(index, &ty)| {
                    self.import_input_type(ty, method.parameter_nullability(index), owner, span)
                })
                .collect::<Checked<_>>()?,
            result: self.import_annotated_type(raw.result, method.read_nullability, owner, span)?,
        })
    }

    fn field_type(&self, receiver: Type, field: Type, span: Span) -> Checked<Type> {
        if let Type::ConstructedStruct(id) = receiver {
            self.specialize(
                field,
                &self.instance(id).arguments,
                ParameterKind::Struct,
                span,
            )
        } else {
            Ok(field)
        }
    }

    fn import_signature_type(&self, ty: Type, owner: &[Type], span: Span) -> Checked<Type> {
        match ty {
            Type::TypeParameter(index) | Type::NullableTypeParameter(index) => {
                let argument = owner.get(usize::from(index)).copied().ok_or_else(|| {
                    Diagnostic::new(
                        "E0205",
                        span,
                        "CLR signature needs a constructed declaring type",
                    )
                })?;
                nullability::specialize_annotation(ty, argument, span)
            }
            Type::ImportedArray(id) | Type::NullableImportedArray(id) => {
                let element =
                    self.import_signature_type(self.references.signature_element(id), owner, span)?;
                let array = self.array(element, span)?;
                Ok(if ty.is_nullable() {
                    array.nullable().expect("CLR array")
                } else {
                    array
                })
            }
            Type::ImportedInstance(id) | Type::NullableImportedInstance(id) => {
                let instance = self.references.signature_instance(id);
                let arguments = instance
                    .arguments
                    .iter()
                    .map(|&arg| self.import_signature_type(arg, owner, span))
                    .collect::<Checked<Vec<_>>>()?;
                let imported = self.constructed_type(
                    InstanceKind::External(instance.definition),
                    arguments,
                    span,
                )?;
                Ok(if ty.is_nullable() {
                    imported.nullable().expect("CLR reference instance")
                } else {
                    imported
                })
            }
            _ => Ok(ty),
        }
    }

    fn import_annotated_type(
        &self,
        ty: Type,
        contract: NullabilityOverride,
        owner: &[Type],
        span: Span,
    ) -> Checked<Type> {
        // Apply the contract to a type parameter before substitution as well,
        // so its existing annotation cannot override the contract.
        let ty = if contract == NullabilityOverride::NonNull
            || matches!(ty, Type::TypeParameter(_) | Type::NullableTypeParameter(_))
        {
            contract.apply(ty)
        } else {
            ty
        };
        let ty = self.import_signature_type(ty, owner, span)?;
        Ok(contract.apply(ty))
    }

    fn import_input_type(
        &self,
        ty: Type,
        contract: NullabilityOverride,
        owner: &[Type],
        span: Span,
    ) -> Checked<Type> {
        if contract != NullabilityOverride::Nullable {
            return self.import_annotated_type(ty, contract, owner, span);
        }
        let ty = self.import_signature_type(ty, owner, span)?;
        // The precondition widens the input after generic argument inference.
        Ok(if ParameterKind::ImportedMethod.index(ty).is_some() {
            ty
        } else {
            contract.apply(ty)
        })
    }

    fn imported_id(&self, ty: Type) -> Option<usize> {
        match ty.non_nullable() {
            Type::External(id) => Some(id),
            Type::ConstructedExternal(id) => match self.instance(id).kind {
                InstanceKind::External(definition) => Some(definition),
                _ => unreachable!("imported type instance"),
            },
            _ => None,
        }
    }

    fn source_type_name(&self, id: usize) -> &'src str {
        // Struct fields are checked while their definitions are still being collected.
        self.definitions.get(id).map_or_else(
            || {
                self.names
                    .iter()
                    .find_map(|(&name, &candidate)| (id == candidate).then_some(name))
                    .expect("declared source type")
            },
            |definition| definition.name,
        )
    }

    fn describe(&self, ty: Type) -> String {
        describe_scoped(self, &[], ty)
    }

    fn field_info_type(&self) -> Type {
        self.field_info_used.store(true, Ordering::Relaxed);
        Type::Struct(self.field_info)
    }

    fn scoped_type(
        &self,
        path: &Path<'_>,
        type_parameters: &[(&str, Span)],
        bounds: Option<&[GenericBound]>,
        allow_void: bool,
        from_struct: bool,
    ) -> Checked<Type> {
        let mut ty = self.base_type(
            path,
            type_parameters,
            bounds,
            allow_void && path.suffixes.is_empty(),
            from_struct,
        )?;
        for &suffix in &path.suffixes {
            match suffix {
                TypeSuffix::Array(span) => ty = self.array(ty, span)?,
                TypeSuffix::Nullable(span) => {
                    ty = self.nullable_type(ty, bounds.unwrap_or_default(), span)?.ok_or_else(|| {
                        let error = Diagnostic::new(
                            "E0311", span, "value-type '?' requires CLR framework references and a non-nullable value type",
                        );
                        if ty.numeric() || ty == Type::Bool {
                            error.with_fix(DiagnosticFix {
                                title: "Remove unsupported nullable suffix",
                                span,
                                replacement: "",
                            })
                        } else {
                            error
                        }
                    })?;
                }
            }
        }
        Ok(ty)
    }

    fn base_type(
        &self,
        path: &Path<'_>,
        type_parameters: &[(&str, Span)],
        bounds: Option<&[GenericBound]>,
        allow_void: bool,
        from_struct: bool,
    ) -> Checked<Type> {
        let segments = path.segments.as_slice();
        if let [(name, _)] = segments
            && let Some(index) = type_parameters
                .iter()
                .position(|&(parameter, _)| *name == parameter)
        {
            self.check_type_arity(name, 0, path)?;
            return Ok(if from_struct {
                Type::TypeParameter(index as u16)
            } else {
                Type::MethodParameter(index as u16)
            });
        }
        if let Some(ty) = builtin_type(segments) {
            if !path.type_arguments.is_empty() {
                self.check_type_arity(&path.qualified_name(), 0, path)?;
            }
            if ty == Type::Void && !allow_void {
                return Err(Diagnostic::new(
                    "E0301",
                    path.span,
                    "System.Void cannot be used as a value type",
                ));
            }
            return Ok(ty);
        }
        if matches!(segments, [("compiler", _), ("FieldInfo", _)]) {
            self.check_type_arity("compiler.FieldInfo", 0, path)?;
            return Ok(self.field_info_type());
        }
        if let [(name, _)] = segments
            && let Some(&id) = self.names.get(name)
        {
            return self.struct_type(id, path, type_parameters, bounds, from_struct);
        }
        self.imported_type(path, type_parameters, bounds, from_struct)
    }

    fn imported_type(
        &self,
        path: &Path<'_>,
        type_parameters: &[(&str, Span)],
        bounds: Option<&[GenericBound]>,
        from_struct: bool,
    ) -> Checked<Type> {
        let imported = self.resolve_imported_type(path, false)?;
        self.instantiate_imported_type(imported, path, type_parameters, bounds, from_struct)
    }

    fn instantiate_imported_type(
        &self,
        imported: usize,
        path: &Path<'_>,
        type_parameters: &[(&str, Span)],
        bounds: Option<&[GenericBound]>,
        from_struct: bool,
    ) -> Checked<Type> {
        let definition = self.references.imported_type(imported);
        self.check_type_arity(&path.qualified_name(), usize::from(definition.arity), path)?;
        if definition.enum_storage.is_some() {
            return Ok(Type::ClrEnum(imported));
        }
        let required = definition.bounds.as_deref().ok_or_else(|| {
            Diagnostic::new(
                "E0211",
                path.span,
                "imported generic type has unsupported or unresolved constraints",
            )
        })?;
        let arguments = self.type_arguments(path, type_parameters, bounds, from_struct)?;
        if let Some(bounds) = bounds {
            self.check_imported_type_bounds(path, &arguments, bounds, required, type_parameters)?;
        }
        self.constructed_type(InstanceKind::External(imported), arguments, path.span)
    }

    fn struct_type(
        &self,
        id: usize,
        path: &Path<'_>,
        type_parameters: &[(&str, Span)],
        bounds: Option<&[GenericBound]>,
        from_struct: bool,
    ) -> Checked<Type> {
        self.check_type_arity(path.segments[0].0, self.arities[id], path)?;
        let arguments = self.type_arguments(path, type_parameters, bounds, from_struct)?;
        if let Some(bounds) = bounds {
            self.check_struct_bounds(
                id,
                |index| {
                    path.type_arguments
                        .get(index)
                        .map_or(path.span, |arg| arg.span)
                },
                &arguments,
                bounds,
                type_parameters,
            )?;
        }
        self.constructed(id, arguments, path.span)
    }

    fn type_arguments(
        &self,
        path: &Path<'_>,
        type_parameters: &[(&str, Span)],
        bounds: Option<&[GenericBound]>,
        from_struct: bool,
    ) -> Checked<Vec<Type>> {
        path.type_arguments
            .iter()
            .map(|argument| self.scoped_type(argument, type_parameters, bounds, false, from_struct))
            .collect()
    }

    fn check_type_arity(&self, name: &str, arity: usize, path: &Path<'_>) -> Checked<()> {
        let actual = path.type_arguments.len();
        if arity != actual {
            return Err(Diagnostic::new(
                "E0205",
                path.span,
                format!("'{name}' expects {arity} type arguments, found {actual}"),
            ));
        }
        Ok(())
    }

    fn resolve_imported_type(&self, path: &Path<'_>, allow_static_class: bool) -> Checked<usize> {
        let name = path.qualified_name();
        let key = if path.type_arguments.is_empty() {
            name.clone()
        } else {
            format!("{name}`{}", path.type_arguments.len())
        };
        let mut candidates = self.references.type_candidates(&key, allow_static_class);
        match (candidates.next(), candidates.next()) {
            (Some(id), None) => Ok(id),
            (None, _) => Err(Diagnostic::new(
                "E0202",
                path.span,
                format!("unknown type '{name}'"),
            )),
            _ => Err(Diagnostic::new(
                "E0208",
                path.span,
                format!("CLR type '{name}' occurs in multiple references"),
            )),
        }
    }

    fn require(
        &self,
        actual: Type,
        expected: Type,
        span: Span,
        type_parameters: &[(&str, Span)],
    ) -> Checked<()> {
        if actual == expected || actual == Type::Never || self.upcasts(actual, expected) {
            return Ok(());
        }
        let expected_name = describe_scoped(self, type_parameters, expected);
        let actual_name = describe_scoped(self, type_parameters, actual);
        let mut error = Diagnostic::new(
            "E0301",
            span,
            format!("expected {expected_name}, found {actual_name}"),
        );
        // Constructed type IDs belong to this module's interner. Expose their
        // resolved names to diagnostics consumers, including cached editor snapshots.
        error.expected = expected.instance_id().is_none().then_some(expected);
        error.actual = actual.instance_id().is_none().then_some(actual);
        error.type_names = Some(Arc::new((expected_name, actual_name)));
        Err(error)
    }
}

pub(crate) struct Function<'src> {
    pub has_exception_regions: bool,
    pub internal: bool,
    pub name: &'src str,
    pub span: Span,
    pub signature: Signature,
    pub type_parameters: Vec<&'src str>,
    pub locals: Vec<Type>,
    pub captured_cells: HashSet<u16>,
    pub body: MethodBody,
    pub lambdas: Vec<Function<'src>>,
}

pub(crate) enum MethodBody {
    Managed(Block),
    Native {
        import: syntax::NativeImport,
        parameters: Vec<syntax::ParameterPassing>,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Signature {
    pub type_arity: u16,
    pub bounds: Vec<GenericBound>,
    pub parameters: Vec<Type>,
    pub result: Type,
}

pub(crate) type GenericBound = TypeBound;

pub(crate) struct Block {
    pub statements: Vec<Statement>,
    pub tail: Option<Box<Expr>>,
    pub ty: Type,
}

pub(crate) enum Statement {
    DeclareLocal {
        index: u16,
        value: Option<Expr>,
    },
    Store {
        target: Value,
        value: Expr,
        span: Span,
    },
    Return(Option<Expr>),
    TryFailure(Option<Expr>),
    Discard(Expr),
    While {
        condition: Expr,
        body: Block,
    },
    For(Box<RangeLoop>),
    LoopControl(LoopControl),
}

pub(crate) struct RangeLoop {
    pub index: u16,
    pub limit: u16,
    pub start: Expr,
    pub end: Expr,
    pub inclusive: bool,
    pub body: Block,
}

impl RangeLoop {
    pub fn diverges(&self) -> bool {
        self.start.ty == Type::Never || self.end.ty == Type::Never
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Value {
    Parameter(u16),
    Local(u16),
    Captured(usize, bool),
    CapturedCell(usize, Type),
}

pub(crate) enum Target {
    Function(usize, Box<[Type]>),
    WriteLine(Option<Type>),
    AssertionException,
    DefaultConstructor(Type),
    External {
        method: usize,
        arguments: Box<[Type]>,
        owner: Option<Type>,
    },
    ExternalField {
        member: Member,
        store: bool,
        address: bool,
    },
}

pub(crate) struct Expr {
    pub kind: ExprKind,
    pub ty: Type,
    pub span: Span,
}

pub(crate) struct TryBlock {
    pub body: Block,
    pub error_local: Option<u16>,
    pub failure: Expr,
}

pub(crate) struct ExceptionRegion {
    pub body: Block,
    pub catches: Vec<Catch>,
    pub result: Option<u16>,
}

pub(crate) struct UsingRegion {
    pub resource: Expr,
    pub local: u16,
    pub body: Block,
    pub dispose: Expr,
    pub result: Option<u16>,
}

pub(crate) struct Catch {
    /// A synthesized handler without a source type catches System.Exception.
    pub ty: Option<Type>,
    pub local: Option<u16>,
    pub body: Block,
}

pub(crate) enum ExprKind {
    Integer(i64),
    Float32(f32),
    Float64(f64),
    Boolean(bool),
    Null,
    DefaultValue(u16),
    Coalesce(Box<Expr>, Box<Expr>, u16),
    String(Arc<str>),
    TypeQuery {
        query: constants::TypeQuery,
        ty: Type,
        argument: Span,
    },
    Interpolation(Vec<Expr>),
    Load(Value),
    Lambda {
        method: usize,
        definition: usize,
        environment: Box<Expr>,
    },
    CaptureCell(Value),
    LocalAddress(Value, ArgumentMode),
    Construct {
        ty: Type,
        local: u16,
        fields: Vec<(usize, Expr)>,
    },
    Field(Box<Expr>, usize),
    BoxValue(Box<Expr>),
    CheckedCast(Box<Expr>, Type),
    NumericCast(Box<Expr>, Type),
    TypeTest(Box<Expr>, Type),
    Throw(Box<Expr>),
    Try(Box<TryBlock>),
    TryCatch(Box<ExceptionRegion>),
    Using(Box<UsingRegion>),
    ConstrainedCall {
        method: Member,
        type_arguments: Box<[Type]>,
        receiver: Box<Expr>,
        temporary: Option<u16>,
        args: Vec<Expr>,
    },
    Call(Target, Vec<Expr>),
    IndexUpdate(Box<IndexUpdate>),
    ArrayAccess(Box<ArrayAccess>),
    NewArray {
        element: Type,
        length: Box<Expr>,
    },
    ArrayLength(Box<Expr>),
    StringLength(Box<Expr>),
    StringIndex(Box<Expr>, Box<Expr>),
    ArrayLiteral {
        element: Type,
        items: Vec<Expr>,
    },
    Unary(Unary, Box<Expr>),
    Binary(Binary, Box<Expr>, Box<Expr>),
    Block(Block),
    Match(Box<Match>),
    If {
        condition: Box<Expr>,
        yes: Block,
        no: Option<Box<Expr>>,
        result_local: Option<u16>,
    },
}

pub(crate) struct Match {
    pub value: Expr,
    pub local: u16,
    pub decisions: Vec<Decision>,
    pub setups: Vec<MatchSetup>,
    pub arms: Vec<Block>,
    pub guards: Vec<MatchGuard>,
    pub result_local: Option<u16>,
}

pub(crate) struct MatchSetup {
    pub arm: usize,
    pub initialization: Block,
}

pub(crate) struct MatchGuard {
    pub arm: usize,
    pub condition: Block,
    pub fallback: usize,
}

pub(crate) struct Decision {
    pub value: Expr,
    pub discriminant: Discriminant,
    pub targets: Vec<Destination>,
    pub literals: Option<LiteralCases>,
}

#[derive(Clone, Copy)]
pub(crate) enum Discriminant {
    Value,
    Tag { zero_variant: Option<usize> },
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum PatternLiteral {
    Integer(i64),
    Character(u16),
    String(Arc<str>),
}

pub(crate) struct LiteralCases {
    pub keys: Vec<PatternLiteral>,
    pub other: Option<Destination>,
}

impl Decision {
    pub(crate) fn zero_tag_target(&self) -> Option<Destination> {
        let Discriminant::Tag { zero_variant } = self.discriminant else {
            return None;
        };
        Some(zero_variant.map_or(Destination::Invalid, |variant| self.targets[variant]))
    }

    fn destinations(&self) -> impl Iterator<Item = &Destination> {
        self.targets.iter().chain(
            self.literals
                .iter()
                .filter_map(|cases| cases.other.as_ref()),
        )
    }

    fn destinations_mut(&mut self) -> impl Iterator<Item = &mut Destination> {
        self.targets.iter_mut().chain(
            self.literals
                .iter_mut()
                .filter_map(|cases| cases.other.as_mut()),
        )
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Destination {
    Invalid,
    Arm(usize),
    Setup(usize),
    Decision(usize),
}

type Checked<T> = Result<T, Diagnostic>;

pub(crate) fn check(
    file: syntax::Module<'_, '_>,
    references: &References,
    kind: Option<AssemblyKind>,
) -> Result<(), Vec<Diagnostic>> {
    let entry = kind.map_or(EntryPoint::None, EntryPoint::for_build);
    prepare(file, references, entry, None, None)?.check_runtime(drop, |()| {})
}

pub(crate) fn lower<'src>(
    mut file: syntax::Module<'_, 'src>,
    references: &References,
    kind: AssemblyKind,
    test_filter: Option<&str>,
) -> Result<Program<'src>, Vec<Diagnostic>> {
    if test_filter.is_none() {
        file.functions.retain(|function| !function.test);
    }
    let mode = if test_filter.is_some() {
        EntryPoint::None
    } else {
        EntryPoint::for_build(kind)
    };
    let scope = prepare(file, references, mode, None, None)?;
    let mut functions = Vec::with_capacity(scope.runtime_functions().len());
    scope.check_runtime(std::convert::identity, |function| functions.push(function))?;
    let entry = if let Some(filter) = test_filter {
        let runner = test_harness::runner(scope.runtime_functions(), filter);
        let entry = functions.len();
        functions.push(runner);
        Some(entry)
    } else {
        scope.entry
    };
    let mut structs = scope.types.definitions;
    if !scope.types.field_info_used.load(Ordering::Relaxed) {
        structs.pop();
    }
    let instances = scope
        .types
        .instances
        .into_inner()
        .expect("type interner")
        .values;
    if let Some(last) = structs.last()
        && last.builtin
        && last.name == "Option"
        && !instances
            .iter()
            .any(|instance| instance.kind == InstanceKind::Struct(structs.len() - 1))
    {
        structs.pop();
    }
    Ok(Program {
        functions,
        structs,
        instances,
        entry,
        kind,
        test_filter: test_filter.map(Into::into),
    })
}

pub(crate) fn inspect(
    file: syntax::Module<'_, '_>,
    references: &References,
    kind: AssemblyKind,
    occurrences: &mut Vec<Occurrence>,
    mut cache: Option<&mut analysis::BodyCache>,
) -> Vec<Diagnostic> {
    let scope = match prepare(
        file,
        references,
        match kind {
            AssemblyKind::Executable => EntryPoint::Optional,
            AssemblyKind::Library => EntryPoint::None,
        },
        Some(occurrences),
        cache.as_deref_mut(),
    ) {
        Ok(scope) => scope,
        Err(errors) => return errors,
    };
    if let Some(cache) = cache.as_deref_mut() {
        cache.prepare_constants(
            scope
                .constants
                .iter()
                .map(|(&name, constant)| (name, constant.fingerprint(), constant.definition)),
        );
    }
    let mut errors = Vec::new();
    let mut missing = Vec::new();
    for (index, &function) in scope.runtime_functions().iter().enumerate() {
        let base = function.declaration_span.start;
        match cache
            .as_deref_mut()
            .and_then(|cache| cache.reuse(function.name, base, occurrences))
        {
            Some(Ok(())) => {}
            Some(Err(error)) => errors.push(error),
            None => missing.push((index, function)),
        }
    }
    let track_lookups = cache.is_some();
    schedule::ordered(
        &missing,
        |&(_, function)| body_bytes(function),
        |_, &(index, function)| {
            let mut facts = Vec::new();
            let mut lookups = Vec::new();
            let result = scope
                .check(
                    index,
                    Some(&mut facts),
                    track_lookups.then_some(&mut lookups),
                )
                .map(|_| ());
            (function, facts, lookups, result)
        },
        |(function, facts, lookups, result)| {
            if let Some(cache) = cache.as_deref_mut() {
                cache.store(
                    function.name,
                    function.declaration_span.start,
                    &facts,
                    lookups,
                    &result,
                );
            }
            occurrences.extend(facts);
            if let Err(error) = result {
                errors.push(error);
            }
        },
    );
    errors
}

fn body_bytes(function: &syntax::Function<'_>) -> usize {
    function
        .body
        .managed()
        .map_or(0, |body| body.span.end - body.span.start)
}

struct ModuleScope<'ast, 'src, 'refs> {
    types: Structs<'src, 'refs>,
    functions: Vec<&'ast syntax::Function<'src>>,
    names: HashMap<&'src str, usize>,
    signatures: Vec<Signature>,
    constants: constants::Values<'src>,
    references: &'refs References,
    entry: Option<usize>,
}

#[derive(Clone, Copy)]
enum EntryPoint {
    Required,
    Optional,
    None,
}

impl EntryPoint {
    fn for_build(kind: AssemblyKind) -> Self {
        match kind {
            AssemblyKind::Executable => Self::Required,
            AssemblyKind::Library => Self::None,
        }
    }
}

fn prepare<'ast, 'src, 'refs>(
    file: syntax::Module<'ast, 'src>,
    references: &'refs References,
    entry_point: EntryPoint,
    mut occurrences: Option<&mut Vec<Occurrence>>,
    cache: Option<&mut analysis::BodyCache>,
) -> Result<ModuleScope<'ast, 'src, 'refs>, Vec<Diagnostic>> {
    let types = Structs::collect(&file.types, references).map_err(|error| vec![error])?;
    let mut functions = file.functions;
    // Runtime method IDs stay dense even though comptime functions share the
    // module namespace and type checker.
    functions.sort_by_key(|function| function.comptime);
    let mut names = HashMap::new();
    let mut signatures = Vec::new();
    let mut errors = Vec::new();
    for (index, function) in functions.iter().enumerate() {
        if names.insert(function.name, index).is_some() {
            errors.push(Diagnostic::new(
                "E0201",
                function.name_span,
                format!("duplicate function '{}'", function.name),
            ));
        }
        match signature(function, &types) {
            Ok(signature) => {
                if function.test
                    && (function.name == "main"
                        || signature.type_arity != 0
                        || !signature.parameters.is_empty()
                        || signature.result != Type::Void)
                {
                    errors.push(Diagnostic::new(
                        "E0346",
                        function.name_span,
                        "a test must be a non-generic, parameterless void function named other than 'main'",
                    ));
                }
                signatures.push(signature);
            }
            Err(error) => errors.push(error),
        }
    }
    if !errors.is_empty() {
        errors.sort_by_key(|error| error.span.start);
        return Err(errors);
    }
    let entry = match (entry_point, names.get("main").copied()) {
        (EntryPoint::None, _) => None,
        (_, Some(index))
            if !functions[index].comptime
                && functions[index].body.managed().is_some()
                && signatures[index].type_arity == 0
                && match signatures[index].parameters.as_slice() {
                    [] => true,
                    [ty @ Type::Array(_)] => types.array_element(*ty) == Some(Type::String),
                    _ => false,
                }
                && matches!(signatures[index].result, Type::Void | Type::I32) =>
        {
            Some(index)
        }
        (_, Some(index)) => {
            return Err(vec![Diagnostic::new(
                "E0206",
                functions[index].name_span,
                "main must have an Oxel body, be non-generic, take no parameters or one string[] parameter, and return void or i32",
            )]);
        }
        (EntryPoint::Optional, None) => None,
        (EntryPoint::Required, None) => {
            return Err(vec![Diagnostic::new(
                "E0206",
                Span::default(),
                "missing entry point function main",
            )]);
        }
    };
    if let Some(occurrences) = occurrences.as_deref_mut() {
        for (definition, declaration) in types.definitions.iter().zip(&file.types) {
            occurrences.push(Occurrence {
                span: definition.span,
                kind: if definition.union.is_some() {
                    OccurrenceKind::Enum
                } else {
                    OccurrenceKind::Struct
                },
                definition: None,
                definition_source: None,
                call_parameters: None,
                detail: format!(
                    "{}{} {}{}",
                    if definition.internal { "internal " } else { "" },
                    if definition.union.is_some() {
                        "enum"
                    } else {
                        "struct"
                    },
                    definition.name,
                    bounds::describe(&declaration.type_parameters, &definition.bounds, &types)
                ),
                is_declaration: true,
                visibility: None,
            });
            for &(name, span) in &declaration.type_parameters {
                occurrences.push(Occurrence {
                    span,
                    kind: OccurrenceKind::TypeParameter,
                    definition: None,
                    definition_source: None,
                    call_parameters: None,
                    detail: format!("type parameter {name}"),
                    is_declaration: true,
                    visibility: None,
                });
            }
            for (source, bound) in declaration.type_bounds.iter().zip(&definition.bounds) {
                let (syntax::Bound::Type(path), TypeBoundKind::Target(target)) =
                    (&source.constraint, bound.kind)
                else {
                    continue;
                };
                observe_type(
                    &types,
                    &declaration.type_parameters,
                    path,
                    target,
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
            let fields = match &declaration.shape {
                syntax::TypeShape::Struct(fields) => fields,
                syntax::TypeShape::Union(variants) => {
                    unions::observe_declaration(
                        &types,
                        definition,
                        variants,
                        &declaration.type_parameters,
                        occurrences,
                    );
                    continue;
                }
            };
            for (field, (_, _, annotation)) in definition.fields.iter().zip(fields) {
                occurrences.push(Occurrence {
                    span: field.span,
                    kind: OccurrenceKind::Field { readonly: true },
                    definition: None,
                    definition_source: None,
                    call_parameters: None,
                    detail: format!(
                        "{}: {}",
                        field.name,
                        describe_scoped(&types, &declaration.type_parameters, field.ty)
                    ),
                    is_declaration: true,
                    visibility: None,
                });
                observe_type(
                    &types,
                    &declaration.type_parameters,
                    annotation,
                    field.ty,
                    &mut |span, definition, kind, detail| {
                        occurrences.push(Occurrence {
                            span,
                            kind,
                            definition,
                            detail,
                            definition_source: None,
                            call_parameters: None,
                            is_declaration: false,
                            visibility: None,
                        });
                    },
                );
            }
        }
    }
    let mut scope = ModuleScope {
        types,
        functions,
        names,
        signatures,
        constants: constants::Values::new(),
        references,
        entry,
    };
    constants::evaluate(&file.constants, &mut scope, occurrences, cache)
        .map_err(|error| vec![error])?;
    Ok(scope)
}

impl<'ast, 'src> ModuleScope<'ast, 'src, '_> {
    fn runtime_functions(&self) -> &[&'ast syntax::Function<'src>] {
        &self.functions[..self
            .functions
            .partition_point(|function| !function.comptime)]
    }

    fn check_runtime<R: Send>(
        &self,
        finish: impl Fn(Function<'src>) -> R + Sync + Send,
        mut accept: impl FnMut(R),
    ) -> Result<(), Vec<Diagnostic>> {
        let mut errors = Vec::new();
        schedule::ordered(
            self.runtime_functions(),
            |function| body_bytes(function),
            |index, _| self.check(index, None, None).map(&finish),
            |result| match result {
                Ok(value) => accept(value),
                Err(error) => errors.push(error),
            },
        );
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn check(
        &self,
        index: usize,
        occurrences: Option<&mut Vec<Occurrence>>,
        lookups: Option<&mut Vec<Lookup<'src>>>,
    ) -> Checked<Function<'src>> {
        let function = self.functions[index];
        let signature = &self.signatures[index];
        let types = &self.types;
        let mut checker = Checker {
            names: &self.names,
            comptime: function.comptime,
            in_test: function.test,
            signatures: &self.signatures,
            constants: &self.constants,
            functions: &self.functions,
            occurrences,
            lookups,
            references: self.references,
            types,
            result: signature.result,
            type_parameters: &function.type_parameters,
            bounds: &signature.bounds,
            scopes: vec![Scope::Empty],
            locals: Vec::new(),
            uninitialized: Vec::new(),
            loop_depth: 0,
            try_context: None,
            has_exception_regions: false,
            nonnull: Vec::new(),
            capture: None,
            captured_cells: HashSet::new(),
            lambdas: Vec::new(),
        };
        checker.track_signature(function);
        checker.record_declaration(function.name_span, OccurrenceKind::Function, || {
            signature_detail(function, signature, types)
        });
        for &(name, span) in &function.type_parameters {
            checker.record_declaration(span, OccurrenceKind::TypeParameter, || {
                format!("type parameter {name}")
            });
        }
        for (source, bound) in function.type_bounds.iter().zip(&signature.bounds) {
            if let (syntax::Bound::Type(path), TypeBoundKind::Target(target)) =
                (&source.constraint, bound.kind)
            {
                checker.record_annotation(path, target);
            }
        }
        for (index, parameter) in function.parameters.iter().enumerate() {
            let name = parameter.name;
            let span = parameter.span;
            let ty = signature.parameters[index];
            if let Some(body) = function.body.managed() {
                checker.record_scoped_binding(
                    name,
                    span,
                    OccurrenceKind::Parameter,
                    ty,
                    body.interior(),
                );
                checker.scopes[0].insert(
                    name,
                    Binding {
                        value: Value::Parameter(index as u16),
                        ty,
                        mutable: false,
                        uninitialized: false,
                        can_make_mutable: false,
                        definition: span,
                    },
                );
            } else {
                checker.record_declaration(span, OccurrenceKind::Parameter, || {
                    format!("{name}: {}", types.describe(ty))
                });
            }
            checker.record_annotation(&parameter.annotation, ty);
        }
        if let Some(result) = function.result.as_ref() {
            checker.record_annotation(result, signature.result);
        }
        let body = match &function.body {
            syntax::FunctionBody::Managed(source) => {
                let body = checker.block(source, Some(signature.result))?;
                if !checker.uninitialized.is_empty() {
                    assignment::validate(&body, &checker.uninitialized)?;
                }
                MethodBody::Managed(body)
            }
            syntax::FunctionBody::Native(import) => MethodBody::Native {
                import: import.clone(),
                parameters: function
                    .parameters
                    .iter()
                    .map(|param| param.passing)
                    .collect(),
            },
        };
        Ok(Function {
            has_exception_regions: checker.has_exception_regions,
            internal: function.internal,
            name: function.name,
            span: function.name_span,
            signature: signature.clone(),
            type_parameters: function
                .type_parameters
                .iter()
                .map(|&(name, _)| name)
                .collect(),
            locals: checker.locals,
            captured_cells: checker.captured_cells,
            body,
            lambdas: checker.lambdas,
        })
    }
}

fn signature(function: &syntax::Function<'_>, types: &Structs<'_, '_>) -> Checked<Signature> {
    if function.parameters.len() > u16::MAX as usize {
        return Err(Diagnostic::new(
            "E0207",
            function.name_span,
            "too many parameters for a CLR method",
        ));
    }
    let type_parameters = &function.type_parameters;
    let type_arity = validate_type_parameters(type_parameters, function.name_span)?;
    let native = function.body.managed().is_none();
    if native && type_arity != 0 {
        return Err(Diagnostic::new(
            "E0343",
            function.name_span,
            "native imports cannot be generic",
        ));
    }
    let bounds = bounds::resolve(type_parameters, &function.type_bounds, types, false)?;
    bounds::validate_arguments(
        type_parameters,
        &function.type_bounds,
        &bounds,
        types,
        false,
    )?;
    let exported = !function.internal && !function.comptime;
    if exported {
        types.require_public_bounds(&bounds, &function.type_bounds)?;
    }
    let mut names = HashSet::new();
    let mut parameters = Vec::new();
    for parameter in &function.parameters {
        let name = parameter.name;
        let annotation = &parameter.annotation;
        if !names.insert(name) {
            return Err(Diagnostic::new(
                "E0201",
                parameter.span,
                format!("duplicate parameter '{name}'"),
            ));
        }
        let ty = types.scoped_type(annotation, type_parameters, Some(&bounds), false, false)?;
        if native {
            match parameter.passing {
                syntax::ParameterPassing::Utf8 => {
                    if !matches!(ty, Type::String | Type::NullableString) {
                        return Err(Diagnostic::new(
                            "E0343",
                            annotation.span,
                            "'utf8' requires a string or string? parameter",
                        ));
                    }
                }
                _ if matches!(ty, Type::String | Type::NullableString) => {
                    return Err(Diagnostic::new(
                        "E0343",
                        annotation.span,
                        "native parameters require an explicit 'utf8' prefix for by-value strings",
                    ));
                }
                _ => types.require_native_type(ty, annotation.span, "native parameters")?,
            }
        }
        if exported {
            types.require_public_type(ty, annotation.span)?;
        }
        parameters.push(ty);
    }
    let result = function
        .result
        .as_ref()
        .map(|path| types.scoped_type(path, type_parameters, Some(&bounds), true, false))
        .transpose()?
        .unwrap_or(Type::Void);
    if let Some(path) = &function.result {
        if native && result != Type::Void {
            types.require_native_type(result, path.span, "native return types")?;
        }
        if exported {
            types.require_public_type(result, path.span)?;
        }
    }
    if function.comptime {
        let span = function.name_span;
        let supported = constants::supported_value;
        let valid = (result == Type::Void
            || supported(types, result, constants::ValueContext::Function, span)?)
            && parameters.iter().try_fold(true, |valid, &ty| {
                Ok(valid && supported(types, ty, constants::ValueContext::Function, span)?)
            })?;
        if !valid {
            return Err(Diagnostic::new(
                "E0501",
                span,
                "comptime functions require signatures with supported value types",
            ));
        }
    }
    Ok(Signature {
        type_arity,
        bounds,
        parameters,
        result,
    })
}

fn validate_type_parameters(parameters: &[(&str, Span)], span: Span) -> Checked<u16> {
    let arity = u16::try_from(parameters.len())
        .map_err(|_| Diagnostic::new("E0207", span, "too many CLR generic parameters"))?;
    let mut names = HashSet::new();
    for &(name, span) in parameters {
        if builtin_type(&[(name, span)]).is_some() || !names.insert(name) {
            return Err(Diagnostic::new(
                "E0201",
                span,
                format!("duplicate or reserved type parameter '{name}'"),
            ));
        }
    }
    Ok(arity)
}

fn describe_scoped(types: &Structs<'_, '_>, type_parameters: &[(&str, Span)], ty: Type) -> String {
    match ty {
        Type::MethodParameter(index) | Type::TypeParameter(index) => type_parameters
            .get(index as usize)
            .map_or_else(|| ty.to_string(), |&(name, _)| name.into()),
        Type::Array(id) | Type::NullableArray(id) => {
            let element = describe_scoped(types, type_parameters, types.instance(id).arguments[0]);
            let nullable = if ty.is_nullable() { "?" } else { "" };
            format!("{element}[]{nullable}")
        }
        Type::Struct(id) => types.source_type_name(id).into(),
        Type::ConstructedStruct(id)
        | Type::ConstructedClrStruct(id)
        | Type::ConstructedExternal(id)
        | Type::NullableConstructedExternal(id) => {
            let instance = types.instance(id);
            let arguments = instance
                .arguments
                .iter()
                .map(|&arg| describe_scoped(types, type_parameters, arg))
                .collect::<Vec<_>>()
                .join(", ");
            let name = match instance.kind {
                InstanceKind::External(id) => {
                    let name = &types.references.imported_type(id).qualified_name;
                    name.rsplit_once('`')
                        .map_or(name.as_str(), |(name, _)| name)
                }
                InstanceKind::Struct(id) => types.source_type_name(id),
                InstanceKind::Array => unreachable!("constructed non-array type"),
            };
            let nullable = if ty.is_nullable() { "?" } else { "" };
            format!("{name}<{arguments}>{nullable}")
        }
        Type::External(id) | Type::ClrEnum(id) | Type::ClrStruct(id) => {
            types.references.imported_type(id).qualified_name.clone()
        }
        Type::NullableExternal(id) => {
            format!("{}?", types.references.imported_type(id).qualified_name)
        }
        ty => ty.to_string(),
    }
}

fn observe_type(
    types: &Structs<'_, '_>,
    type_parameters: &[(&str, Span)],
    path: &Path<'_>,
    ty: Type,
    observe: &mut impl FnMut(Span, Option<Span>, OccurrenceKind, String),
) {
    // The source identifier names the innermost element, so it keeps its own
    // definition link and can be renamed even inside jagged array signatures.
    let mut element_type = ty;
    while let Some(element) = types.array_element(element_type) {
        element_type = element;
    }
    let (definition, kind) = match element_type {
        Type::ClrEnum(_) => (None, OccurrenceKind::Enum),
        ty if ty.clr_struct() => (None, OccurrenceKind::Struct),
        Type::MethodParameter(index) | Type::TypeParameter(index) => (
            Some(type_parameters[index as usize].1),
            OccurrenceKind::TypeParameter,
        ),
        ty => {
            if let Some(id) = types.definition_of(ty) {
                let definition = &types.definitions[id];
                let kind = if definition.union.is_some() {
                    OccurrenceKind::Enum
                } else {
                    OccurrenceKind::Struct
                };
                (definition.source_span(definition.span), kind)
            } else if let Some(id) = types.imported_id(ty) {
                let kind = if types.references.imported_type(id).is_interface {
                    OccurrenceKind::Interface
                } else {
                    OccurrenceKind::Class
                };
                (None, kind)
            } else {
                (None, OccurrenceKind::BuiltinType)
            }
        }
    };
    observe(
        path.head_span(),
        definition,
        kind,
        describe_scoped(types, type_parameters, ty),
    );
    if let Type::ConstructedStruct(id)
    | Type::ConstructedExternal(id)
    | Type::ConstructedClrStruct(id) = element_type.non_nullable()
    {
        let instance = types.instance(id);
        for (path, &arg) in path.type_arguments.iter().zip(instance.arguments.iter()) {
            observe_type(types, type_parameters, path, arg, observe);
        }
    }
}

fn generic_parameters(parameters: &[(&str, Span)]) -> String {
    if parameters.is_empty() {
        String::new()
    } else {
        format!(
            "<{}>",
            parameters
                .iter()
                .map(|&(name, _)| name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn signature_detail(
    function: &syntax::Function<'_>,
    signature: &Signature,
    types: &Structs<'_, '_>,
) -> String {
    let params = function
        .parameters
        .iter()
        .zip(&signature.parameters)
        .map(|(parameter, &ty)| {
            format!(
                "{}{}: {}",
                parameter.passing.prefix(),
                parameter.name,
                describe_scoped(types, &function.type_parameters, ty)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let generics = bounds::describe(&function.type_parameters, &signature.bounds, types);
    format!(
        "{} {}{generics}({params}) -> {}",
        syntax::function_prefix(
            function.internal,
            function.comptime,
            function.body.managed().is_none()
        ),
        function.name,
        describe_scoped(types, &function.type_parameters, signature.result)
    )
}

fn builtin_type(name: &[(&str, Span)]) -> Option<Type> {
    match name {
        [("System", _), (name, _)] => Type::clr_builtin(name),
        [(name, _)] => Type::builtin_alias(name),
        _ => None,
    }
}

/// Displays available signatures for a CLR static call before its arguments are resolved.
pub fn clr_function_signatures(name: &str, references: &References) -> Vec<CallableSignature> {
    if name == "System.Console.WriteLine" {
        return vec![
            CallableSignature::new(name.into(), ["System.Object?".into()], "System.Void", false),
            CallableSignature::new(name.into(), [], "System.Void", false),
        ];
    }
    let candidates = references.candidates(name);
    if candidates.is_empty() {
        return Vec::new();
    }
    let types = Structs::collect(&[], references).expect("no source types");
    candidates
        .iter()
        .filter_map(|&id| {
            let method = references.method(id);
            let signature = types
                .method_signature(Member::declared(id, None), Span::default())
                .ok()?;
            Some(CallableSignature::new(
                name.into(),
                signature
                    .parameters
                    .into_iter()
                    .enumerate()
                    .map(|(index, ty)| {
                        format!(
                            "{}{}",
                            method.parameter_mode(index).prefix(),
                            types.describe(ty)
                        )
                    }),
                &types.describe(signature.result),
                method.parameter_array,
            ))
        })
        .collect()
}

fn record_type_lookup<'src>(lookups: &mut Vec<Lookup<'src>>, name: &'src str) {
    if Type::builtin_alias(name).is_none() {
        lookups.push(Lookup::Type(name));
    }
}

fn explain_argument(
    error: Diagnostic,
    declaration: &syntax::Function<'_>,
    args: &[syntax::Expr<'_>],
) -> Diagnostic {
    if matches!(error.code, "E0301" | "E0210")
        && error.related().is_none()
        && let Some(argument) = args.iter().position(|expr| expr.span == error.span)
    {
        let parameter = declaration.parameters[argument].name;
        return error.with_related(
            declaration.name_span,
            format!(
                "parameter '{parameter}' is declared by '{}'",
                declaration.name
            ),
        );
    }
    error
}

#[derive(Clone, Copy)]
struct Binding {
    value: Value,
    ty: Type,
    mutable: bool,
    uninitialized: bool,
    can_make_mutable: bool,
    definition: Span,
}

// Avoid allocating and hashing for empty blocks and single-binding scopes.
enum Scope<'src> {
    Empty,
    Single(&'src str, Binding),
    Multiple(HashMap<&'src str, Binding>),
}

impl<'src> Scope<'src> {
    fn get(&self, name: &str) -> Option<Binding> {
        match self {
            Self::Single(key, binding) if *key == name => Some(*binding),
            Self::Multiple(bindings) => bindings.get(name).copied(),
            _ => None,
        }
    }

    fn insert(&mut self, name: &'src str, binding: Binding) -> Option<Binding> {
        match self {
            Self::Empty => {
                *self = Self::Single(name, binding);
                None
            }
            Self::Single(key, old) if *key == name => Some(std::mem::replace(old, binding)),
            Self::Single(key, old) => {
                *self = Self::Multiple(HashMap::from([(*key, *old), (name, binding)]));
                None
            }
            Self::Multiple(bindings) => bindings.insert(name, binding),
        }
    }

    fn any_binding(&self, mut matches: impl FnMut(Binding) -> bool) -> bool {
        match self {
            Self::Empty => false,
            Self::Single(_, binding) => matches(*binding),
            Self::Multiple(bindings) => bindings.values().copied().any(matches),
        }
    }

    fn merge(&mut self, other: Self) {
        match other {
            Self::Empty => {}
            Self::Single(name, binding) => {
                self.insert(name, binding);
            }
            Self::Multiple(bindings) => {
                for (name, binding) in bindings {
                    self.insert(name, binding);
                }
            }
        }
    }
}

fn suggest_mutable(error: Diagnostic, offset: usize) -> Diagnostic {
    error.with_fix(DiagnosticFix::insert(
        "Make the binding mutable",
        offset,
        "mut ",
    ))
}

#[derive(Clone, Copy)]
enum OverloadUse {
    Method(Option<Type>),
    Indexer(PropertyAccess),
}

struct ResolvedOverload {
    member: Member,
    type_arguments: Box<[Type]>,
    arguments: Vec<Expr>,
    result: Type,
}

struct ExternalOverload<'a> {
    member: Member,
    signature: Signature,
    hints: Vec<Option<Type>>,
    defaults: &'a [MetadataConstant],
    parameter_array: Option<(usize, Type)>,
}

struct ExternalCandidate<'a> {
    member: Member,
    type_arguments: std::borrow::Cow<'a, [Type]>,
    parameters: Vec<Type>,
    result: Type,
    defaults: &'a [MetadataConstant],
    parameter_array: Option<(usize, Type)>,
}

#[derive(Clone, Copy)]
struct Assignment<'a, 'src> {
    operator: Option<Binary>,
    value: &'a syntax::Expr<'src>,
}

enum MemberAccess<'a, 'src> {
    Read,
    ReadForAssignment,
    Write {
        value: &'a syntax::Expr<'src>,
        previous: Option<(Binary, Expr)>,
    },
}

struct Checker<'a, 'src> {
    in_test: bool,
    comptime: bool,
    names: &'a HashMap<&'src str, usize>,
    signatures: &'a [Signature],
    constants: &'a constants::Values<'src>,
    functions: &'a [&'a syntax::Function<'src>],
    occurrences: Option<&'a mut Vec<Occurrence>>,
    lookups: Option<&'a mut Vec<Lookup<'src>>>,
    references: &'a References,
    types: &'a Structs<'src, 'a>,
    result: Type,
    type_parameters: &'a [(&'src str, Span)],
    bounds: &'a [GenericBound],
    scopes: Vec<Scope<'src>>,
    locals: Vec<Type>,
    uninitialized: Vec<assignment::Local<'src>>,
    loop_depth: usize,
    try_context: Option<unions::TryContext>,
    has_exception_regions: bool,
    nonnull: Vec<Value>,
    capture: Option<lambdas::CaptureScope<'a, 'src>>,
    captured_cells: HashSet<u16>,
    lambdas: Vec<Function<'src>>,
}

impl<'a, 'src> Checker<'a, 'src> {
    fn record(
        &mut self,
        span: Span,
        definition: Option<Span>,
        kind: OccurrenceKind,
        detail: impl FnOnce() -> String,
    ) {
        self.record_occurrence(span, definition, kind, false, None, detail);
    }

    fn record_declaration(
        &mut self,
        span: Span,
        kind: OccurrenceKind,
        detail: impl FnOnce() -> String,
    ) {
        self.record_occurrence(span, None, kind, true, None, detail);
    }

    fn record_scoped_binding(
        &mut self,
        name: &str,
        span: Span,
        kind: OccurrenceKind,
        ty: Type,
        visibility: Span,
    ) {
        let types = self.types;
        let type_parameters = self.type_parameters;
        self.record_occurrence(span, None, kind, true, Some(visibility), || {
            format!("{name}: {}", describe_scoped(types, type_parameters, ty))
        });
    }

    fn record_occurrence(
        &mut self,
        span: Span,
        definition: Option<Span>,
        kind: OccurrenceKind,
        is_declaration: bool,
        visibility: Option<Span>,
        detail: impl FnOnce() -> String,
    ) {
        if let Some(occurrences) = &mut self.occurrences {
            occurrences.push(Occurrence {
                span,
                kind,
                definition,
                definition_source: None,
                call_parameters: None,
                detail: detail(),
                is_declaration,
                visibility,
            });
        }
    }

    fn require(&self, actual: Type, expected: Type, span: Span) -> Checked<()> {
        if self.can_upcast(actual, expected) {
            return Ok(());
        }
        self.types
            .require(actual, expected, span, self.type_parameters)
    }

    fn record_annotation(&mut self, path: &Path<'_>, ty: Type) {
        if self.occurrences.is_some() {
            observe_type(
                self.types,
                self.type_parameters,
                path,
                ty,
                &mut |span, definition, kind, detail| {
                    self.record(span, definition, kind, || detail);
                },
            );
        }
    }

    fn record_clr_call(
        &mut self,
        name: &str,
        span: Span,
        parameters: impl IntoIterator<Item = (ArgumentMode, Type)>,
        result: Type,
        kind: OccurrenceKind,
        variadic: bool,
    ) {
        let Some(occurrences) = &mut self.occurrences else {
            return;
        };
        let types = self.types;
        let type_parameters = self.type_parameters;
        let signature = CallableSignature::new(
            name.into(),
            parameters.into_iter().map(|(mode, ty)| {
                format!(
                    "{}{}",
                    mode.prefix(),
                    describe_scoped(types, type_parameters, ty)
                )
            }),
            &describe_scoped(types, type_parameters, result),
            variadic,
        );
        occurrences.push(Occurrence {
            span,
            kind,
            definition: None,
            definition_source: None,
            call_parameters: Some(signature.parameters),
            detail: signature.label,
            is_declaration: false,
            visibility: None,
        });
    }

    fn block(&mut self, block: &syntax::Block<'src>, expected: Option<Type>) -> Checked<Block> {
        self.scopes.push(Scope::Empty);
        let result = self.block_contents(block, expected);
        self.scopes.pop();
        result
    }

    fn block_contents(
        &mut self,
        block: &syntax::Block<'src>,
        expected: Option<Type>,
    ) -> Checked<Block> {
        self.with_nonnull([], |checker| checker.check_block_contents(block, expected))
    }

    fn check_block_contents(
        &mut self,
        block: &syntax::Block<'src>,
        expected: Option<Type>,
    ) -> Checked<Block> {
        let mut statements = Vec::with_capacity(block.statements.len());
        let mut diverges = false;
        for statement in &block.statements {
            let statement = self.statement(statement, block.interior().end)?;
            diverges |= match &statement {
                Statement::Return(_) | Statement::TryFailure(_) | Statement::LoopControl(_) => true,
                Statement::While { condition, .. } => condition.ty == Type::Never,
                Statement::For(range) => range.diverges(),
                Statement::Store { value, .. } | Statement::Discard(value) => {
                    value.ty == Type::Never
                }
                Statement::DeclareLocal { value, .. } => {
                    value.as_ref().is_some_and(|value| value.ty == Type::Never)
                }
            };
            if !diverges {
                self.refine_after_statement(&statement);
            }
            statements.push(statement);
        }
        let tail = block
            .tail
            .as_ref()
            .map(|expr| {
                self.expression(expr, if diverges { None } else { expected })
                    .map(Box::new)
            })
            .transpose()?;
        let ty = if diverges {
            Type::Never
        } else {
            tail.as_ref().map_or(Type::Void, |expr| expr.ty)
        };
        if let Some(expected) = expected {
            self.require(ty, expected, block.span)?;
        }
        Ok(Block {
            statements,
            tail,
            ty,
        })
    }

    fn binding(&mut self, name: &str, span: Span) -> Option<Binding> {
        let mut binding = self
            .scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .or_else(|| self.capture.as_mut()?.binding(name, span))?;
        if self.nonnull.contains(&binding.value) {
            binding.ty = binding.ty.non_nullable();
        }
        let types = self.types;
        let type_parameters = self.type_parameters;
        let kind = match binding.value {
            Value::Parameter(_) | Value::Captured(_, true) => OccurrenceKind::Parameter,
            Value::Local(_) | Value::Captured(_, false) | Value::CapturedCell(..) => {
                OccurrenceKind::Local {
                    mutable: binding.mutable,
                }
            }
        };
        self.record(span, Some(binding.definition), kind, || {
            format!(
                "{name}: {}",
                describe_scoped(types, type_parameters, binding.ty)
            )
        });
        Some(binding)
    }

    fn statement(
        &mut self,
        statement: &syntax::Statement<'src>,
        scope_end: usize,
    ) -> Checked<Statement> {
        Ok(match statement {
            syntax::Statement::LetPattern(binding) => self.let_pattern(binding, scope_end)?,
            syntax::Statement::WhileLet(binding) => self.while_let(binding)?,
            syntax::Statement::Let {
                name,
                name_span,
                mutable,
                annotation,
                value,
                end,
            } => {
                let expected = annotation
                    .as_ref()
                    .map(|path| self.scoped_value_type(path))
                    .transpose()?;
                if let Some((path, ty)) = annotation.as_ref().zip(expected) {
                    self.record_annotation(path, ty);
                }
                let ty;
                let value = if let Some(value) = value {
                    let initializer = self.expression(value, expected)?;
                    let value = if expected.is_none()
                        && !matches!(value.kind, Expression::New(..))
                        && matches!(
                            initializer.kind,
                            ExprKind::Call(
                                Target::External { .. }
                                    | Target::ExternalField { store: false, .. },
                                _
                            ) | ExprKind::ConstrainedCall { .. }
                        )
                        && let Some((carrier, layout)) =
                            self.nullable_option(initializer.ty, None, initializer.span)?
                    {
                        let span = initializer.span;
                        self.pack_nullable_option(initializer, carrier, layout, span)?
                    } else {
                        initializer
                    };
                    if value.ty == Type::Null {
                        return Err(Diagnostic::new(
                            "E0311",
                            value.span,
                            "a null binding requires a nullable reference annotation",
                        ));
                    }
                    if value.ty == Type::Void {
                        return Err(Diagnostic::new(
                            "E0301",
                            value.span,
                            "cannot bind a System.Void expression",
                        ));
                    }
                    ty = if value.ty == Type::Never {
                        expected.unwrap_or(Type::I32)
                    } else {
                        value.ty
                    };
                    Some(value)
                } else {
                    ty = expected.ok_or_else(|| {
                        Diagnostic::new(
                            "E0320",
                            *name_span,
                            "a declaration without an initializer requires a type annotation",
                        )
                    })?;
                    None
                };
                self.record_scoped_binding(
                    name,
                    *name_span,
                    OccurrenceKind::Local { mutable: *mutable },
                    ty,
                    Span {
                        start: *end,
                        end: scope_end,
                    },
                );
                let index = self.local(ty, *name_span)?;
                // Mutable references cannot use the binding's non-null refinement.
                let can_make_mutable = !ty.is_nullable();
                self.scopes.last_mut().expect("block scope").insert(
                    name,
                    Binding {
                        value: Value::Local(index),
                        ty,
                        mutable: *mutable,
                        uninitialized: value.is_none(),
                        can_make_mutable,
                        definition: *name_span,
                    },
                );
                if value.is_none() {
                    self.uninitialized.push(assignment::Local {
                        index,
                        name,
                        mutable: *mutable,
                        mutability_edit: can_make_mutable.then_some(name_span.start),
                    });
                }
                Statement::DeclareLocal { index, value }
            }
            syntax::Statement::Assign {
                operator,
                target,
                value,
            } => self.assignment(
                target,
                Assignment {
                    operator: *operator,
                    value,
                },
            )?,
            syntax::Statement::Return { span, value } => {
                if self.try_context.is_some() {
                    return Err(Diagnostic::new(
                        "E0334",
                        *span,
                        "'return' is not allowed inside a try block",
                    ));
                }
                let value = value
                    .as_ref()
                    .map(|expr| self.expression(expr, Some(self.result)))
                    .transpose()?;
                if value.is_none() {
                    self.require(Type::Void, self.result, *span)?;
                }
                Statement::Return(value)
            }
            syntax::Statement::While { condition, body } => {
                let condition = self.expression(condition, Some(Type::Bool))?;
                let refinements = self.null_refinements(&condition, true);
                self.loop_depth += 1;
                let body =
                    self.with_nonnull(refinements, |checker| checker.block(body, Some(Type::Void)));
                self.loop_depth -= 1;
                Statement::While {
                    condition,
                    body: body?,
                }
            }
            syntax::Statement::For(loop_) => match &loop_.range {
                Some(range) => self.range_loop(loop_, range)?,
                None => self.sequence_loop(loop_)?,
            },
            syntax::Statement::LoopControl { kind, span } => {
                if self
                    .try_context
                    .as_ref()
                    .is_some_and(|context| context.loop_depth == self.loop_depth)
                {
                    return Err(Diagnostic::new(
                        "E0334",
                        *span,
                        "loop control cannot exit a try block",
                    ));
                }
                if self.loop_depth == 0 {
                    let keyword = match kind {
                        LoopControl::Break => "break",
                        LoopControl::Continue => "continue",
                    };
                    return Err(Diagnostic::new(
                        "E0307",
                        *span,
                        format!("'{keyword}' requires an enclosing loop body"),
                    ));
                }
                Statement::LoopControl(*kind)
            }
            syntax::Statement::Expression(expr) => Statement::Discard(self.expression(expr, None)?),
        })
    }

    fn range_loop(
        &mut self,
        loop_: &syntax::ForLoop<'src>,
        range: &syntax::RangeEnd<'src>,
    ) -> Checked<Statement> {
        let start_literal = matches!(loop_.source.kind, Expression::Int { .. });
        let end_literal = matches!(range.end.kind, Expression::Int { .. });
        let (start, end) = if start_literal && !end_literal {
            let end = self.expression(&range.end, None)?;
            let start = self.expression(&loop_.source, end.ty.integer().then_some(end.ty))?;
            (start, end)
        } else {
            let start = self.expression(&loop_.source, None)?;
            let hint = (end_literal && start.ty.integer()).then_some(start.ty);
            let end = self.expression(&range.end, hint)?;
            (start, end)
        };
        for value in [&start, &end] {
            if value.ty != Type::Never && !value.ty.integer() {
                return Err(Diagnostic::new(
                    "E0313",
                    value.span,
                    "range bounds must be integers",
                ));
            }
        }
        let ty = [start.ty, end.ty]
            .into_iter()
            .find(|ty| ty.integer())
            .unwrap_or(Type::I32);
        self.require(end.ty, ty, range.end.span)?;
        let index = self.local(ty, loop_.binding.1)?;
        let limit = self.local(ty, range.end.span)?;
        Ok(Statement::For(Box::new(RangeLoop {
            index,
            limit,
            start,
            end,
            inclusive: range.inclusive,
            body: self.for_body(loop_, index, ty)?,
        })))
    }

    fn sequence_loop(&mut self, loop_: &syntax::ForLoop<'src>) -> Checked<Statement> {
        let source = self.expression(&loop_.source, None)?;
        let span = source.span;
        let ty = source.ty;
        let binding_span = loop_.binding.1;
        if ty == Type::Never {
            // Check the unreachable body with a placeholder element type.
            let binding = self.local(Type::I32, binding_span)?;
            self.for_body(loop_, binding, Type::I32)?;
            return Ok(Statement::Discard(source));
        }
        let element = match ty {
            Type::String => Type::Char,
            Type::Array(_) => self.types.array_element(ty).expect("CLR vector"),
            _ => return self.enumeration_loop(loop_, source),
        };
        // Capture the sequence before the loop, then use the range loop's index and control flow.
        let sequence = self.local(ty, span)?;
        let index = self.local(Type::I32, span)?;
        let limit = self.local(Type::I32, span)?;
        let binding = self.local(element, binding_span)?;
        let load = |index, ty| Expr {
            kind: ExprKind::Load(Value::Local(index)),
            ty,
            span,
        };
        let receiver = || Box::new(load(sequence, ty));
        let (length, value) = if ty == Type::String {
            (
                ExprKind::StringLength(receiver()),
                Expr {
                    kind: ExprKind::StringIndex(receiver(), Box::new(load(index, Type::I32))),
                    ty: element,
                    span,
                },
            )
        } else {
            (
                ExprKind::ArrayLength(receiver()),
                arrays::array_access(
                    load(sequence, ty),
                    load(index, Type::I32),
                    element,
                    None,
                    span,
                ),
            )
        };
        let mut body = self.for_body(loop_, binding, element)?;
        body.statements.insert(
            0,
            Statement::Store {
                target: Value::Local(binding),
                value,
                span: binding_span,
            },
        );
        let range = Statement::For(Box::new(RangeLoop {
            index,
            limit,
            start: Expr {
                kind: ExprKind::Integer(0),
                ty: Type::I32,
                span,
            },
            end: Expr {
                kind: length,
                ty: Type::I32,
                span,
            },
            inclusive: false,
            body,
        }));
        Ok(Statement::Discard(Expr {
            kind: ExprKind::Block(Block {
                statements: vec![
                    Statement::Store {
                        target: Value::Local(sequence),
                        value: source,
                        span,
                    },
                    range,
                ],
                tail: None,
                ty: Type::Void,
            }),
            ty: Type::Void,
            span,
        }))
    }

    fn for_body(&mut self, loop_: &syntax::ForLoop<'src>, index: u16, ty: Type) -> Checked<Block> {
        let (name, span) = loop_.binding;
        self.record_scoped_binding(
            name,
            span,
            OccurrenceKind::Local { mutable: false },
            ty,
            loop_.body.interior(),
        );
        self.scopes.push(Scope::Single(
            name,
            Binding {
                value: Value::Local(index),
                ty,
                mutable: false,
                uninitialized: false,
                can_make_mutable: false,
                definition: span,
            },
        ));
        self.loop_depth += 1;
        let body = self.block_contents(&loop_.body, Some(Type::Void));
        self.loop_depth -= 1;
        self.scopes.pop();
        body
    }

    fn local(&mut self, ty: Type, span: Span) -> Checked<u16> {
        if self.comptime
            && !constants::supported_value(self.types, ty, constants::ValueContext::Function, span)?
        {
            return Err(Diagnostic::new(
                "E0501",
                span,
                "comptime functions require supported local value types",
            ));
        }
        let index = u16::try_from(self.locals.len())
            .ok()
            .filter(|&index| index < u16::MAX)
            .ok_or_else(|| {
                Diagnostic::new("E0207", span, "too many local variables for a CLR method")
            })?;
        self.locals.push(ty);
        Ok(index)
    }

    fn assignment(
        &mut self,
        target: &syntax::Expr<'src>,
        assignment: Assignment<'_, 'src>,
    ) -> Checked<Statement> {
        let setter = match &target.kind {
            Expression::Name(path) => {
                let (&(name, span), receiver) = path.segments.split_last().expect("nonempty path");
                if receiver.is_empty() {
                    let binding = self.binding(name, span).ok_or_else(|| {
                        if self.constant(name).is_some() {
                            Diagnostic::new(
                                "E0204",
                                span,
                                format!("constant '{name}' is immutable"),
                            )
                        } else {
                            Diagnostic::new("E0203", span, format!("unknown variable '{name}'"))
                        }
                    })?;
                    if !matches!(binding.value, Value::Local(_) | Value::CapturedCell(..)) {
                        let label = if matches!(binding.value, Value::Parameter(_)) {
                            "parameter"
                        } else {
                            "captured binding"
                        };
                        return Err(Diagnostic::new(
                            "E0204",
                            span,
                            format!("{label} '{name}' is immutable"),
                        ));
                    }
                    if !binding.mutable && (!binding.uninitialized || assignment.operator.is_some())
                    {
                        let error = Diagnostic::new(
                            "E0204",
                            span,
                            format!("binding '{name}' is immutable"),
                        );
                        return Err(if binding.can_make_mutable && !binding.uninitialized {
                            suggest_mutable(error, binding.definition.start)
                        } else {
                            error
                        });
                    }
                    let previous = assignment.operator.map(|operator| {
                        (
                            operator,
                            Expr {
                                kind: ExprKind::Load(binding.value),
                                ty: binding.ty,
                                span,
                            },
                        )
                    });
                    return Ok(Statement::Store {
                        target: binding.value,
                        value: self.assigned_value(assignment.value, previous, binding.ty)?,
                        span,
                    });
                }
                if let Some(receiver) = self.path_value(receiver)? {
                    self.instance_data_member(receiver, name, span, Some(assignment))?
                } else {
                    let name = path.qualified_name();
                    let members = self.static_data_members(&name, None);
                    self.data_member(&name, span, &members, None, Some(assignment))?
                }
            }
            Expression::Field(receiver, name, span) => {
                let receiver = self.expression(receiver, None)?;
                self.instance_data_member(receiver, name, *span, Some(assignment))?
            }
            Expression::Index {
                receiver,
                arguments,
                span,
            } => {
                let receiver = self.expression(receiver, None)?;
                self.indexer(receiver, arguments, *span, Some(assignment))?
            }
            Expression::StaticMember {
                owner,
                member: (name, span),
                call: None,
            } => self.generic_static_data_member(owner, name, *span, Some(assignment))?,
            _ => {
                return Err(Diagnostic::new(
                    "E0204",
                    target.span,
                    "expected an assignable binding or CLR member",
                ));
            }
        };
        Ok(Statement::Discard(setter))
    }

    fn path_value(&mut self, segments: &[(&'src str, Span)]) -> Checked<Option<Expr>> {
        self.path_value_expected(segments, None)
    }

    fn path_value_expected(
        &mut self,
        segments: &[(&'src str, Span)],
        expected: Option<Type>,
    ) -> Checked<Option<Expr>> {
        let (first, span) = segments[0];
        let (mut value, fields) = if let Some(value) = self.named_value(first, span)? {
            (value, &segments[1..])
        } else if let [_, (_, variant_span), fields @ ..] = segments
            && let Some((kind, ty)) =
                self.variant_constructor(&segments[..2], &[], &[], expected)?
        {
            (
                Expr {
                    kind,
                    ty,
                    span: Span {
                        start: span.start,
                        end: variant_span.end,
                    },
                },
                fields,
            )
        } else {
            if segments.len() < 2 {
                return Ok(None);
            }
            let mut qualified = first.to_string();
            let mut root = None;
            for (index, &(name, span)) in segments.iter().enumerate().skip(1) {
                qualified.push('.');
                qualified.push_str(name);
                let members = self.static_data_members(&qualified, None);
                if !members.is_empty() {
                    root = Some((
                        self.data_member(&qualified, span, &members, None, None)?,
                        &segments[index + 1..],
                    ));
                    break;
                }
            }
            let Some(root) = root else { return Ok(None) };
            root
        };
        for &(name, span) in fields {
            value = self.field(value, name, span)?;
        }
        Ok(Some(value))
    }

    fn constant(&mut self, name: &'src str) -> Option<&'a constants::Constant> {
        // Record misses as well: a new constant can resolve an unknown name or
        // take precedence over a qualified CLR or enum path.
        if let Some(lookups) = &mut self.lookups {
            lookups.push(Lookup::Constant(name));
        }
        self.constants.get(name)
    }

    fn named_value(&mut self, name: &'src str, span: Span) -> Checked<Option<Expr>> {
        if let Some(binding) = self.binding(name, span) {
            Ok(Some(Expr {
                kind: ExprKind::Load(binding.value),
                ty: binding.ty,
                span,
            }))
        } else {
            let Some(constant) = self.constant(name) else {
                return Ok(None);
            };
            self.record(
                span,
                Some(constant.definition),
                OccurrenceKind::Constant,
                || constant.detail(name),
            );
            constant.expression(self, span).map(Some)
        }
    }

    fn track_constant_type(&mut self, ty: Type) {
        let Some(lookups) = &mut self.lookups else {
            return;
        };
        let mut seen = HashSet::new();
        let mut pending = vec![ty];
        while let Some(ty) = pending.pop() {
            if !seen.insert(ty) {
                continue;
            }
            if let Some(id) = self.types.definition_of(ty) {
                lookups.push(Lookup::Type(self.types.definitions[id].name));
            }
            if let Some(id) = ty.instance_id() {
                pending.extend(self.types.instance(id).arguments.iter().copied());
            }
        }
    }

    fn instance_data_member(
        &mut self,
        receiver: Expr,
        name: &str,
        span: Span,
        value: Option<Assignment<'_, 'src>>,
    ) -> Checked<Expr> {
        if receiver.ty == Type::Never {
            return Ok(receiver);
        }
        self.require_nonnull_receiver(receiver.ty, span)?;
        let owner_name = describe_scoped(self.types, self.type_parameters, receiver.ty);
        if !receiver.ty.external_reference()
            && !receiver.ty.clr_struct()
            && !matches!(receiver.ty, Type::MethodParameter(_))
        {
            return Err(Diagnostic::new(
                "E0204",
                span,
                format!("'{owner_name}.{name}' is not an assignable CLR member"),
            ));
        }
        self.require_mutable_struct_receiver(&receiver, value.is_some(), span)?;
        let qualified = format!("{owner_name}.{name}");
        let members = self.bound_members(receiver.ty, |owner| {
            self.types.instance_data_members(owner, name)
        });
        self.data_member(&qualified, span, &members, Some(receiver), value)
    }

    fn static_data_members(&self, key: &str, owner: Option<(usize, Type)>) -> Vec<DataMember> {
        let matches_owner = |declaration| match owner {
            Some((id, _)) => id == declaration,
            None => self.references.imported_type(declaration).arity == 0,
        };
        let owner = owner.map(|(_, ty)| ty);
        self.references
            .static_fields(key)
            .iter()
            .copied()
            .filter(|&id| matches_owner(self.references.field(id).owner))
            .map(|id| DataMember::Field(Member::declared(id, owner)))
            .chain(
                self.references
                    .static_properties(key)
                    .iter()
                    .copied()
                    .filter(|&id| matches_owner(self.references.property_owner(id)))
                    .map(|id| DataMember::Property(Member::declared(id, owner))),
            )
            .chain(
                self.references
                    .static_events(key)
                    .iter()
                    .copied()
                    .filter(|&id| matches_owner(self.references.event_owner(id)))
                    .map(|id| DataMember::Event(Member::declared(id, owner))),
            )
            .collect()
    }

    fn generic_static_member(
        &mut self,
        path: &Path<'src>,
        member: &str,
    ) -> Checked<(usize, Type, String, String)> {
        self.track_type_path(path);
        let id = self.types.resolve_imported_type(path, true)?;
        let owner = self.types.instantiate_imported_type(
            id,
            path,
            self.type_parameters,
            Some(self.bounds),
            false,
        )?;
        self.record_annotation(path, owner);
        let name = describe_scoped(self.types, self.type_parameters, owner);
        let metadata_name = &self.references.imported_type(id).qualified_name;
        Ok((
            id,
            owner,
            format!("{name}.{member}"),
            format!("{metadata_name}.{member}"),
        ))
    }

    fn generic_static_data_member(
        &mut self,
        path: &Path<'src>,
        member: &str,
        span: Span,
        value: Option<Assignment<'_, 'src>>,
    ) -> Checked<Expr> {
        let (id, owner, name, key) = self.generic_static_member(path, member)?;
        let members = self.static_data_members(&key, Some((id, owner)));
        self.data_member(&name, span, &members, None, value)
    }

    fn generic_static_call(
        &mut self,
        path: &Path<'src>,
        member: &str,
        span: Span,
        call: &syntax::CallArguments<'src>,
    ) -> Checked<(ExprKind, Type)> {
        let (id, owner, name, key) = self.generic_static_member(path, member)?;
        let methods = self
            .references
            .candidates(&key)
            .iter()
            .copied()
            .filter(|&method| self.references.method(method).owner == id)
            .map(|method| Member::declared(method, Some(owner)))
            .collect::<Vec<_>>();
        let (target, args, result) = self.external_call(
            &name,
            span,
            &methods,
            &call.type_arguments,
            &call.arguments,
            None,
        )?;
        Ok(self.call_result(target, args, result))
    }

    fn data_member(
        &mut self,
        name: &str,
        span: Span,
        members: &[DataMember],
        receiver: Option<Expr>,
        value: Option<Assignment<'_, 'src>>,
    ) -> Checked<Expr> {
        if let [DataMember::Event(event)] = members {
            return self.event_handler(name, span, *event, receiver, value);
        }
        let Some(assignment) = value else {
            return self.access_data_member(name, span, members, receiver, MemberAccess::Read);
        };
        let Some(operator) = assignment.operator else {
            return self.access_data_member(
                name,
                span,
                members,
                receiver,
                MemberAccess::Write {
                    value: assignment.value,
                    previous: None,
                },
            );
        };
        let (receiver, saved) = match receiver {
            Some(receiver) if !matches!(receiver.kind, ExprKind::Load(_)) => {
                let index = self.local(receiver.ty, receiver.span)?;
                let load = Expr {
                    kind: ExprKind::Load(Value::Local(index)),
                    ty: receiver.ty,
                    span: receiver.span,
                };
                let store = Statement::Store {
                    target: Value::Local(index),
                    span: receiver.span,
                    value: receiver,
                };
                (Some(load), Some(store))
            }
            receiver => (receiver, None),
        };
        // A load is evaluated as the setter receiver before the getter and RHS.
        // Retain its original address for constrained calls on value types.
        let read_receiver = receiver.as_ref().map(|receiver| {
            let ExprKind::Load(binding) = receiver.kind else {
                unreachable!("saved compound assignment receiver")
            };
            Expr {
                kind: ExprKind::Load(binding),
                ty: receiver.ty,
                span: receiver.span,
            }
        });
        let previous = self.access_data_member(
            name,
            span,
            members,
            read_receiver,
            MemberAccess::ReadForAssignment,
        )?;
        let write = self.access_data_member(
            name,
            span,
            members,
            receiver,
            MemberAccess::Write {
                value: assignment.value,
                previous: Some((operator, previous)),
            },
        )?;
        if let Some(saved) = saved {
            let ty = write.ty;
            Ok(Expr {
                kind: ExprKind::Block(Block {
                    statements: vec![saved],
                    tail: Some(Box::new(write)),
                    ty,
                }),
                ty,
                span,
            })
        } else {
            Ok(write)
        }
    }

    fn event_handler(
        &mut self,
        name: &str,
        span: Span,
        event: Member,
        receiver: Option<Expr>,
        assignment: Option<Assignment<'_, 'src>>,
    ) -> Checked<Expr> {
        let (access, handler) = match assignment {
            Some(Assignment {
                operator: Some(Binary::Add),
                value,
            }) => (EventAccess::Add, value),
            Some(Assignment {
                operator: Some(Binary::Sub),
                value,
            }) => (EventAccess::Remove, value),
            _ => {
                return Err(Diagnostic::new(
                    "E0341",
                    span,
                    format!("CLR event '{name}' requires '+=' or '-='"),
                ));
            }
        };
        if self.comptime {
            return Err(Diagnostic::new(
                "E0501",
                span,
                "CLR events cannot be used during constant evaluation",
            ));
        }
        let method = self
            .references
            .event_accessor(event.id, access)
            .ok_or_else(|| {
                Diagnostic::new(
                    "E0341",
                    span,
                    format!(
                        "CLR event '{name}' has no supported public {} accessor",
                        access.name()
                    ),
                )
            })?;
        let member = Member {
            id: method,
            owner: event.owner,
        };
        let signature = self.types.method_signature(member, span)?;
        let handler_type = signature.parameters[0];
        if !self
            .types
            .imported_id(handler_type)
            .is_some_and(|id| self.references.imported_type(id).is_delegate)
        {
            return Err(Diagnostic::new(
                "E0341",
                span,
                format!("CLR event '{name}' requires a delegate type"),
            ));
        }
        let types = self.types;
        let type_parameters = self.type_parameters;
        self.record(span, None, OccurrenceKind::Event, || {
            format!(
                "event {name}: {}",
                describe_scoped(types, type_parameters, handler_type)
            )
        });
        let handler = self.expression(handler, Some(handler_type))?;
        let target = Target::External {
            method,
            owner: event.owner,
            arguments: Box::new([]),
        };
        let (kind, ty) = if let Some(receiver) = receiver {
            self.call_clr_instance(receiver, target, vec![handler], Type::Void, span)?
        } else {
            self.call_result(target, vec![handler], Type::Void)
        };
        Ok(Expr { kind, ty, span })
    }

    fn access_data_member(
        &mut self,
        name: &str,
        span: Span,
        members: &[DataMember],
        receiver: Option<Expr>,
        access: MemberAccess<'_, 'src>,
    ) -> Checked<Expr> {
        match members {
            [DataMember::Field(field)] => self.external_field(name, span, *field, receiver, access),
            [DataMember::Property(property)] if !self.comptime => {
                self.property(name, span, *property, receiver, access)
            }
            [DataMember::Property(_)] => Err(Diagnostic::new(
                "E0501",
                span,
                "CLR properties cannot be accessed during constant evaluation",
            )),
            [] => Err(Diagnostic::new(
                "E0305",
                span,
                format!("unknown CLR field or property '{name}'"),
            )),
            _ => Err(Diagnostic::new(
                "E0208",
                span,
                format!("ambiguous CLR field or property '{name}'"),
            )),
        }
    }

    fn external_field(
        &mut self,
        name: &str,
        span: Span,
        member: Member,
        receiver: Option<Expr>,
        access: MemberAccess<'_, 'src>,
    ) -> Checked<Expr> {
        let field = self.references.field(member.id);
        let signature_type = field.ty.ok_or_else(|| {
            Diagnostic::new("E0305", span, format!("unsupported CLR field '{name}'"))
        })?;
        let store = matches!(access, MemberAccess::Write { .. });
        if field.readonly && store {
            return Err(Diagnostic::new(
                "E0204",
                span,
                format!("CLR field '{name}' is readonly"),
            ));
        }
        let owner_arguments = self.types.owner_arguments(member.owner);
        let owner = owner_arguments.as_deref().unwrap_or_default();
        let ty = if store {
            self.types
                .import_input_type(signature_type, field.write_nullability, owner, span)?
        } else {
            self.types
                .import_annotated_type(signature_type, field.read_nullability, owner, span)?
        };
        let types = self.types;
        let parameters = self.type_parameters;
        if !matches!(access, MemberAccess::ReadForAssignment) {
            self.record(
                span,
                None,
                OccurrenceKind::Field {
                    readonly: field.readonly,
                },
                || format!("{name}: {}", describe_scoped(types, parameters, ty)),
            );
        }
        if let Some(constant) = &field.constant {
            let kind = constant_expression(constant);
            return Ok(Expr { kind, ty, span });
        }
        if self.comptime {
            return Err(Diagnostic::new(
                "E0501",
                span,
                "CLR storage fields cannot be accessed during constant evaluation",
            ));
        }
        let address = store && receiver.as_ref().is_some_and(|value| value.ty.clr_struct());
        let mut args = Vec::new();
        if let Some(receiver) = receiver {
            args.push(if matches!(receiver.ty, Type::MethodParameter(_)) {
                self.coerce(
                    receiver,
                    member.owner.unwrap_or(Type::External(field.owner)),
                )
            } else {
                receiver
            });
        }
        if let MemberAccess::Write { value, previous } = access {
            args.push(self.assigned_value(value, previous, ty)?);
        }
        let (kind, ty) = self.call_result(
            Target::ExternalField {
                member,
                store,
                address,
            },
            args,
            if store { Type::Void } else { ty },
        );
        Ok(Expr { kind, ty, span })
    }

    fn property(
        &mut self,
        name: &str,
        span: Span,
        property: Member,
        receiver: Option<Expr>,
        access: MemberAccess<'_, 'src>,
    ) -> Checked<Expr> {
        let store = matches!(access, MemberAccess::Write { .. });
        let accessor_kind = if store {
            PropertyAccess::Set
        } else {
            PropertyAccess::Get
        };
        let method = self
            .references
            .accessor(property.id, accessor_kind)
            .ok_or_else(|| {
                let accessor = if store { "setter" } else { "getter" };
                Diagnostic::new(
                    "E0308",
                    span,
                    format!("'{name}' has no supported public {accessor}"),
                )
            })?;
        let owner = property.owner;
        let member = Member { id: method, owner };
        let signature = self.types.method_signature(member, span)?;
        let property_type = signature
            .parameters
            .first()
            .copied()
            .unwrap_or(signature.result);
        let result_type = if store { Type::Void } else { property_type };
        let types = self.types;
        if !matches!(access, MemberAccess::ReadForAssignment) {
            self.record(span, None, OccurrenceKind::Property, || {
                format!("{name}: {}", types.describe(property_type))
            });
        }
        let args = if let MemberAccess::Write { value, previous } = access {
            vec![self.assigned_value(value, previous, property_type)?]
        } else {
            Vec::new()
        };
        if let Some(receiver) = receiver {
            self.require_nonnull_receiver(receiver.ty, span)?;
            return self.call_property_accessor(member, receiver, args, result_type, span);
        }
        let (kind, ty) = self.call_result(
            Target::External {
                method,
                arguments: Box::new([]),
                owner,
            },
            args,
            result_type,
        );
        Ok(Expr { kind, ty, span })
    }

    fn field(&mut self, receiver: Expr, name: &str, span: Span) -> Checked<Expr> {
        if receiver.ty == Type::Never {
            return Ok(receiver);
        }
        if name == "Length" && receiver.ty.non_nullable() == Type::String {
            self.require_nonnull_receiver(receiver.ty, span)?;
            self.record(span, None, OccurrenceKind::Property, || {
                "Length: System.Int32".into()
            });
            return Ok(Expr {
                kind: ExprKind::StringLength(Box::new(receiver)),
                ty: Type::I32,
                span,
            });
        }
        if name == "Length" && self.types.array_element(receiver.ty).is_some() {
            self.require_nonnull_receiver(receiver.ty, span)?;
            self.record(span, None, OccurrenceKind::Property, || {
                "Length: System.Int32".into()
            });
            return Ok(Expr {
                kind: ExprKind::ArrayLength(Box::new(receiver)),
                ty: Type::I32,
                span,
            });
        }
        if receiver.ty.external_reference()
            || receiver.ty.clr_struct()
            || matches!(receiver.ty, Type::MethodParameter(_))
        {
            return self.instance_data_member(receiver, name, span, None);
        }
        let Some(id) = self.types.definition_of(receiver.ty) else {
            return Err(Diagnostic::new(
                "E0305",
                span,
                format!("{} has no field '{name}'", self.types.describe(receiver.ty)),
            ));
        };
        let types = self.types;
        let definition = &types.definitions[id];
        let (field, info) = definition.field(name, span)?;
        let ty = types.field_type(receiver.ty, info.ty, span)?;
        let type_parameters = self.type_parameters;
        self.record(
            span,
            definition.source_span(info.span),
            OccurrenceKind::Field { readonly: true },
            || format!("{name}: {}", describe_scoped(types, type_parameters, ty)),
        );
        Ok(Expr {
            kind: ExprKind::Field(Box::new(receiver), field),
            ty,
            span,
        })
    }

    fn construct_clr(
        &mut self,
        path: &Path<'src>,
        args: &[syntax::Expr<'src>],
    ) -> Checked<(ExprKind, Type)> {
        if let Some(&TypeSuffix::Nullable(span)) = path.suffixes.last() {
            return Err(Diagnostic::new(
                "E0311",
                span,
                "constructor type cannot be nullable",
            ));
        }
        if !path.suffixes.is_empty() {
            return Err(Diagnostic::new(
                "E0314",
                path.span,
                "allocate a CLR array with 'new Type[length]'",
            ));
        }
        if let [(name, _)] = path.segments.as_slice()
            && self
                .type_parameters
                .iter()
                .any(|&(parameter, _)| parameter == *name)
        {
            let ty = self.scoped_value_type(path)?;
            if !args.is_empty() {
                return Err(Diagnostic::new(
                    "E0205",
                    path.span,
                    "a type parameter constructor takes no arguments",
                ));
            }
            if !self
                .types
                .satisfies_type_bound(ty, self.bounds, TypeBoundKind::Constructor)
            {
                return Err(Diagnostic::new(
                    "E0212",
                    path.span,
                    "construction requires a 'new()' constraint",
                ));
            }
            self.record_annotation(path, ty);
            return Ok(self.call_result(Target::DefaultConstructor(ty), Vec::new(), ty));
        }
        self.track_type_path(path);
        let ty = self
            .types
            .imported_type(path, self.type_parameters, Some(self.bounds), false)?;
        self.record_annotation(path, ty);
        if args.is_empty() && self.types.nullable_underlying(ty).is_some() {
            let value = self.coerce(
                Expr {
                    kind: ExprKind::Null,
                    ty: Type::Null,
                    span: path.span,
                },
                ty,
            );
            return Ok((value.kind, value.ty));
        }
        let owner = self
            .types
            .ancestor_id(ty)
            .filter(|_| !matches!(ty, Type::ClrEnum(_)))
            .ok_or_else(|| {
                Diagnostic::new(
                    "E0205",
                    path.span,
                    "CLR enums use named values and have no constructor",
                )
            })?;
        let label = format!(
            "new {}",
            describe_scoped(self.types, self.type_parameters, ty)
        );
        let candidates: Vec<_> = self
            .references
            .members(owner, ".ctor")
            .iter()
            .map(|&id| Member::declared(id, Some(ty)))
            .collect();
        let (target, args, result) =
            self.external_call(&label, path.span, &candidates, &[], args, Some(ty))?;
        Ok(self.call_result(target, args, result))
    }

    fn construct(
        &mut self,
        path: &Path<'src>,
        fields: &[(&'src str, Span, syntax::Expr<'src>)],
        base: Option<&syntax::Expr<'src>>,
        span: Span,
        expected: Option<Type>,
    ) -> Checked<(ExprKind, Type)> {
        let name = path.segments[0].0;
        self.track_source_type(name);
        let id =
            *self.types.names.get(name).ok_or_else(|| {
                Diagnostic::new("E0202", span, format!("unknown struct '{name}'"))
            })?;
        let types = self.types;
        let definition = &types.definitions[id];
        if definition.union.is_some() {
            return Err(Diagnostic::new(
                "E0330",
                span,
                format!("construct '{name}' using one of its variants"),
            ));
        }
        let ty = match expected {
            Some(ty) if path.type_arguments.is_empty() && types.definition_of(ty) == Some(id) => ty,
            _ => types.struct_type(id, path, self.type_parameters, Some(self.bounds), false)?,
        };
        self.record_annotation(path, ty);
        let type_parameters = self.type_parameters;
        let mut initialized = vec![false; definition.fields.len()];
        let mut values = Vec::with_capacity(fields.len());
        let mut diverges = false;
        for &(name, field_span, ref value) in fields {
            let (field, info) = definition.field(name, field_span)?;
            let field_type = types.field_type(ty, info.ty, field_span)?;
            self.record(
                field_span,
                Some(info.span),
                if field_span == value.span {
                    OccurrenceKind::ShorthandField
                } else {
                    OccurrenceKind::Field { readonly: true }
                },
                || {
                    format!(
                        "{name}: {}",
                        describe_scoped(types, type_parameters, field_type)
                    )
                },
            );
            if std::mem::replace(&mut initialized[field], true) {
                return Err(Diagnostic::new(
                    "E0306",
                    field_span,
                    format!("field '{name}' is initialized twice"),
                ));
            }
            let value = self.expression(value, Some(field_type))?;
            diverges |= value.ty == Type::Never;
            values.push((field, value));
        }
        if let Some(base) = base {
            let base = self.expression(base, Some(ty))?;
            if values.is_empty() {
                return Ok((base.kind, base.ty));
            }
            return self.construct_update(ty, definition, values, initialized, base, span);
        }
        if let Some(missing) = initialized.iter().position(|initialized| !initialized) {
            return Err(Diagnostic::new(
                "E0306",
                span,
                format!(
                    "missing field '{}' in '{name}'",
                    definition.fields[missing].name
                ),
            ));
        }
        let local = self.local(ty, span)?;
        Ok((
            ExprKind::Construct {
                ty,
                local,
                fields: values,
            },
            if diverges { Type::Never } else { ty },
        ))
    }

    fn construct_update(
        &mut self,
        ty: Type,
        definition: &Struct<'src>,
        mut values: Vec<(usize, Expr)>,
        initialized: Vec<bool>,
        base: Expr,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        let mut statements = Vec::with_capacity(values.len() + 1);
        let mut result = if base.ty == Type::Never {
            Type::Never
        } else {
            ty
        };
        // Keep source evaluation order. Synthetic loads below make constructing
        // the result independent of side effects in either part of the literal.
        for (field, value) in &mut values {
            let field_type = if value.ty == Type::Never {
                result = Type::Never;
                self.types
                    .field_type(ty, definition.fields[*field].ty, value.span)?
            } else {
                value.ty
            };
            let location = value.span;
            let local = self.local(field_type, location)?;
            let load = Expr {
                kind: ExprKind::Load(Value::Local(local)),
                ty: field_type,
                span: location,
            };
            statements.push(Statement::Store {
                target: Value::Local(local),
                value: std::mem::replace(value, load),
                span: location,
            });
        }
        let source = self.local(ty, base.span)?;
        let source_span = base.span;
        statements.push(Statement::Store {
            target: Value::Local(source),
            value: base,
            span: source_span,
        });
        for (field, initialized) in initialized.into_iter().enumerate() {
            if !initialized {
                let field_type = self
                    .types
                    .field_type(ty, definition.fields[field].ty, span)?;
                let receiver = Box::new(Expr {
                    kind: ExprKind::Load(Value::Local(source)),
                    ty,
                    span: source_span,
                });
                values.push((
                    field,
                    Expr {
                        kind: ExprKind::Field(receiver, field),
                        ty: field_type,
                        span,
                    },
                ));
            }
        }
        let local = self.local(ty, span)?;
        Ok((
            ExprKind::Block(Block {
                statements,
                tail: Some(Box::new(Expr {
                    kind: ExprKind::Construct {
                        ty,
                        local,
                        fields: values,
                    },
                    ty,
                    span,
                })),
                ty: result,
            }),
            result,
        ))
    }

    fn expression(&mut self, expr: &syntax::Expr<'src>, expected: Option<Type>) -> Checked<Expr> {
        let value = self.expression_value(expr, expected)?;
        self.contextual_conversion(value, expected)
    }

    fn expression_value(
        &mut self,
        expr: &syntax::Expr<'src>,
        expected: Option<Type>,
    ) -> Checked<Expr> {
        let (kind, ty) = match &expr.kind {
            Expression::Int { digits, negative } => {
                let ty = expected
                    .map(|ty| self.types.nullable_underlying(ty).unwrap_or(ty))
                    .filter(|ty| ty.integer())
                    .unwrap_or(Type::I32);
                let number = integer_literal(digits, *negative)
                    .filter(|&n| ty.contains_integer(n))
                    .ok_or_else(|| {
                        Diagnostic::new(
                            "E0303",
                            expr.span,
                            format!("integer literal is out of range for {ty}"),
                        )
                    })?;
                (ExprKind::Integer(number as i64), ty)
            }
            Expression::Float { digits, negative } => {
                let ty = expected
                    .map(|ty| self.types.nullable_underlying(ty).unwrap_or(ty))
                    .filter(|ty| ty.floating())
                    .unwrap_or(Type::F64);
                let kind = float_literal(digits, *negative, ty).ok_or_else(|| {
                    Diagnostic::new(
                        "E0303",
                        expr.span,
                        format!("floating-point literal is out of range for {ty}"),
                    )
                })?;
                (kind, ty)
            }
            Expression::Match(value, arms) => {
                self.match_expression(value, arms, expr.span, expected)?
            }
            Expression::IfLet(binding) => self.if_let(binding, expr.span, expected)?,
            Expression::Propagate(operand, operator) => self.propagate(operand, *operator)?,
            Expression::Try(body) => self.try_block(body, expected, expr.span)?,
            Expression::TryCatch(body, catches) => {
                self.try_catch(body, catches, expected, expr.span)?
            }
            Expression::Using(resource) => self.using_region(resource, expected, expr.span)?,
            Expression::Lambda(lambda) => self.lambda(lambda, expected, expr.span)?,
            Expression::Throw(operand) => {
                let exception = self.references.exception.ok_or_else(|| {
                    Diagnostic::new("E0316", expr.span, "'throw' requires --framework")
                })?;
                let value = self.expression(operand, Some(Type::External(exception)))?;
                (ExprKind::Throw(Box::new(value)), Type::Never)
            }
            Expression::ByRef(..) => {
                return Err(Diagnostic::new(
                    "E0213",
                    expr.span,
                    "'ref' and 'out' arguments require a CLR method or constructor",
                ));
            }
            Expression::Char(unit) => (ExprKind::Integer(i64::from(*unit)), Type::Char),
            Expression::Bool(value) => (ExprKind::Boolean(*value), Type::Bool),
            Expression::Null => (ExprKind::Null, Type::Null),
            Expression::String(value) => {
                (ExprKind::String(Arc::from(value.as_ref())), Type::String)
            }
            Expression::Interpolated(parts) => self.interpolation(parts, expr.span)?,
            Expression::Name(path) => {
                let (name, span) = path.segments[0];
                let value = self
                    .path_value_expected(&path.segments, expected)?
                    .ok_or_else(|| {
                        Diagnostic::new("E0203", span, format!("unknown variable '{name}'"))
                    })?;
                (value.kind, value.ty)
            }
            Expression::Index {
                receiver,
                arguments,
                span,
            } => {
                let receiver = self.expression(receiver, None)?;
                let value = self.indexer(receiver, arguments, *span, None)?;
                (value.kind, value.ty)
            }
            Expression::Field(receiver, field, span) => {
                let receiver = self.expression(receiver, None)?;
                let value = self.field(receiver, field, *span)?;
                (value.kind, value.ty)
            }
            Expression::Construct { path, fields, base } => {
                self.construct(path, fields, base.as_deref(), expr.span, expected)?
            }
            Expression::Array(items) => self.array_literal(items, expected, expr.span)?,
            Expression::NewArray(element, length) => self.new_array(element, length, expr.span)?,
            Expression::New(path, args) => self.construct_clr(path, args)?,
            Expression::MemberCall {
                receiver,
                method,
                span,
                type_arguments,
                arguments,
            } => {
                let receiver = self.expression(receiver, None)?;
                self.instance_call(receiver, method, *span, type_arguments, arguments)?
            }
            Expression::StaticMember {
                owner,
                member: (name, span),
                call,
            } => {
                if let Some(call) = call {
                    self.generic_static_call(owner, name, *span, call)?
                } else {
                    let value = self.generic_static_data_member(owner, name, *span, None)?;
                    (value.kind, value.ty)
                }
            }
            Expression::Call {
                path,
                type_arguments,
                arguments,
            } => self.call(path, type_arguments, arguments, expected)?,
            Expression::Unary(op, operand) => {
                let hint = match op {
                    Unary::Not => Some(Type::Bool),
                    Unary::Negate => expected.filter(|ty| ty.numeric()),
                    Unary::Complement => expected
                        .map(|ty| self.types.nullable_underlying(ty).unwrap_or(ty))
                        .filter(|ty| ty.bitwise_integer()),
                };
                let span = operand.span;
                let operand = self.expression(operand, hint)?;
                if matches!(op, Unary::Negate) {
                    self.require_numeric(operand.ty, span)?;
                    if operand.ty.unsigned() {
                        return Err(Diagnostic::new(
                            "E0302",
                            span,
                            format!("cannot negate {}", operand.ty),
                        ));
                    }
                }
                if matches!(op, Unary::Complement)
                    && operand.ty != Type::Never
                    && !operand.ty.bitwise_integer()
                {
                    return Err(Diagnostic::new(
                        "E0302",
                        span,
                        "bitwise complement requires an integer or CLR enum",
                    ));
                }
                let ty = operand.ty;
                (ExprKind::Unary(*op, Box::new(operand)), ty)
            }
            Expression::Cast(source, target) => {
                self.conversion(source, target, false, expr.span)?
            }
            Expression::TypeTest(source, target) => {
                self.conversion(source, target, true, expr.span)?
            }
            Expression::Binary(op, left, right) => self.binary(*op, left, right, expected)?,
            Expression::Block(block) => {
                let block = self.block(block, expected)?;
                let ty = block.ty;
                (ExprKind::Block(block), ty)
            }
            Expression::If { condition, yes, no } => {
                let condition = self.expression(condition, Some(Type::Bool))?;
                let yes_refinement = self.null_refinements(&condition, true);
                let no_refinement = self.null_refinements(&condition, false);
                let branch_expected = if no.is_some() {
                    expected
                } else {
                    Some(Type::Void)
                };
                let mut yes = self.with_nonnull(yes_refinement, |checker| {
                    checker.block(yes, branch_expected)
                })?;
                let mut no = no
                    .as_ref()
                    .map(|expr| {
                        let hint = self.else_hint(yes.ty, expr, branch_expected);
                        self.with_nonnull(no_refinement, |checker| checker.expression(expr, hint))
                            .map(Box::new)
                    })
                    .transpose()?;
                let mut ty = Type::Void;
                if let Some(other) = no.take() {
                    let (joined, other) = self.join_branches(&mut yes, *other)?;
                    ty = joined;
                    no = Some(Box::new(other));
                }
                if condition.ty == Type::Never {
                    ty = Type::Never;
                }
                // CLR verification can merge different concrete references to
                // System.Object even when both are assignable to the result interface.
                let result_local = if ty.external_reference()
                    && yes.ty != Type::Never
                    && no.as_ref().is_some_and(|no| no.ty != Type::Never)
                {
                    Some(self.local(ty, expr.span)?)
                } else {
                    None
                };
                (
                    ExprKind::If {
                        condition: Box::new(condition),
                        yes,
                        no,
                        result_local,
                    },
                    ty,
                )
            }
        };
        Ok(Expr {
            kind,
            ty,
            span: expr.span,
        })
    }

    fn using_region(
        &mut self,
        source: &syntax::Using<'src>,
        expected: Option<Type>,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        let disposable = self.references.disposable.ok_or_else(|| {
            Diagnostic::new(
                "E0345",
                span,
                "'using' requires --framework and System.IDisposable",
            )
        })?;
        let [method] = self.references.members(disposable, "Dispose") else {
            return Err(Diagnostic::new(
                "E0345",
                span,
                "System.IDisposable.Dispose is unavailable",
            ));
        };
        let resource = self.expression(&source.value, None)?;
        let resource_type = resource.ty;
        let interface = Type::External(disposable);
        if resource_type != interface && !self.can_upcast(resource_type, interface) {
            return Err(Diagnostic::new(
                "E0345",
                resource.span,
                format!(
                    "'using' requires a non-null System.IDisposable resource, found '{}'",
                    self.types.describe(resource_type)
                ),
            ));
        }
        let local = self.local(resource_type, source.name_span)?;
        self.record_scoped_binding(
            source.name,
            source.name_span,
            OccurrenceKind::Local { mutable: false },
            resource_type,
            source.body.interior(),
        );
        self.scopes.push(Scope::Single(
            source.name,
            Binding {
                value: Value::Local(local),
                ty: resource_type,
                mutable: false,
                uninitialized: false,
                can_make_mutable: false,
                definition: source.name_span,
            },
        ));
        let body = self.block(&source.body, expected);
        self.scopes.pop();
        let body = body?;
        self.disposal_region(resource, local, body, Member::declared(*method, None), span)
    }

    fn disposal_region(
        &mut self,
        resource: Expr,
        local: u16,
        body: Block,
        dispose: Member,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        let dispose = local_interface_call(local, resource.ty, dispose, Type::Void, span);
        let ty = body.ty;
        let result = self.exception_result(ty, span)?;
        self.has_exception_regions = true;
        Ok((
            ExprKind::Using(Box::new(UsingRegion {
                resource,
                local,
                body,
                dispose,
                result,
            })),
            ty,
        ))
    }

    fn exception_result(&mut self, ty: Type, span: Span) -> Checked<Option<u16>> {
        if matches!(ty, Type::Never | Type::Void | Type::Null) {
            Ok(None)
        } else {
            self.local(ty, span).map(Some)
        }
    }

    fn try_catch(
        &mut self,
        source: &syntax::Block<'src>,
        catches: &[syntax::CatchArm<'src>],
        expected: Option<Type>,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        let exception = self.references.exception.ok_or_else(|| {
            Diagnostic::new("E0344", span, "CLR exception handling requires --framework")
        })?;
        let (body, catches, ty) =
            self.exception_branches(source, catches, expected, Type::External(exception))?;
        self.has_exception_regions = true;
        let result = self.exception_result(ty, span)?;
        Ok((
            ExprKind::TryCatch(Box::new(ExceptionRegion {
                body,
                catches,
                result,
            })),
            ty,
        ))
    }

    fn exception_branches(
        &mut self,
        source: &syntax::Block<'src>,
        catches: &[syntax::CatchArm<'src>],
        expected: Option<Type>,
        exception: Type,
    ) -> Checked<(Block, Vec<Catch>, Type)> {
        let mut body = self.block(source, expected)?;
        let mut result = body.ty;
        let mut handlers: Vec<Catch> = Vec::with_capacity(catches.len());
        for arm in catches {
            let (name, span, annotation) = &arm.binding;
            let ty = self.scoped_value_type(annotation)?;
            if self.types.imported_id(ty).is_none()
                || !(ty == exception || self.can_upcast(ty, exception))
            {
                return Err(Diagnostic::new(
                    "E0344",
                    annotation.span,
                    "catch requires a non-null CLR exception class",
                ));
            }
            if handlers
                .iter()
                .filter_map(|handler| handler.ty)
                .any(|previous| ty == previous || self.can_upcast(ty, previous))
            {
                return Err(Diagnostic::new(
                    "E0344",
                    annotation.span,
                    "an earlier catch already handles this exception type",
                ));
            }
            self.record_annotation(annotation, ty);
            self.scopes.push(Scope::Empty);
            let checked = (|| {
                let local = if *name == "_" {
                    None
                } else {
                    let local = self.local(ty, *span)?;
                    self.record_scoped_binding(
                        name,
                        *span,
                        OccurrenceKind::Local { mutable: false },
                        ty,
                        arm.body.interior(),
                    );
                    self.scopes.last_mut().expect("handler scope").insert(
                        name,
                        Binding {
                            value: Value::Local(local),
                            ty,
                            mutable: false,
                            uninitialized: false,
                            can_make_mutable: false,
                            definition: *span,
                        },
                    );
                    Some(local)
                };
                let hint = arm
                    .body
                    .tail
                    .as_ref()
                    .and_then(|tail| self.else_hint(result, tail, expected))
                    .or(expected);
                let body = self.block(&arm.body, hint)?;
                Ok(Catch {
                    ty: Some(ty),
                    local,
                    body,
                })
            })();
            self.scopes.pop();
            let handler = checked?;
            result = self.join_types(result, handler.body.ty, arm.body.span)?;
            handlers.push(handler);
        }
        for branch in std::iter::once(&mut body).chain(handlers.iter_mut().map(|arm| &mut arm.body))
        {
            if branch.ty != Type::Never && branch.ty != result {
                let tail = branch.tail.take().expect("converted exception result");
                branch.tail = Some(Box::new(self.coerce(*tail, result)));
                branch.ty = result;
            }
        }
        Ok((body, handlers, result))
    }

    fn contextual_conversion(&mut self, value: Expr, expected: Option<Type>) -> Checked<Expr> {
        if let Some(expected) = expected {
            let (ty, span) = (value.ty, value.span);
            if let Some((carrier, layout)) = self.nullable_option(ty, Some(expected), span)? {
                return self.pack_nullable_option(value, carrier, layout, span);
            }
            if let Some((layout, payload)) = self.option_nullable(ty, expected) {
                return self.unpack_nullable_option(value, expected, layout, payload);
            }
            self.require(ty, expected, span)?;
            Ok(self.coerce(value, expected))
        } else {
            Ok(value)
        }
    }

    fn assigned_value(
        &mut self,
        value: &syntax::Expr<'src>,
        previous: Option<(Binary, Expr)>,
        expected: Type,
    ) -> Checked<Expr> {
        let Some((operator, lhs)) = previous else {
            return self.expression(value, Some(expected));
        };
        let rhs = self.expression(
            value,
            Some(if operator.shift() { Type::I32 } else { lhs.ty }),
        )?;
        let span = Span {
            start: lhs.span.start,
            end: rhs.span.end,
        };
        let (kind, ty) = self.binary_operands(operator, lhs, rhs)?;
        self.require(ty, expected, span)?;
        Ok(self.coerce(Expr { kind, ty, span }, expected))
    }

    fn binary(
        &mut self,
        op: Binary,
        left: &syntax::Expr<'src>,
        right: &syntax::Expr<'src>,
        expected: Option<Type>,
    ) -> Checked<(ExprKind, Type)> {
        use Binary::*;
        if op.shift() {
            let hint = expected
                .map(|ty| self.types.nullable_underlying(ty).unwrap_or(ty))
                .filter(|ty| ty.integer());
            let lhs = self.expression(left, hint)?;
            let rhs = self.expression(right, Some(Type::I32))?;
            return self.binary_operands(op, lhs, rhs);
        }
        if matches!(op, Coalesce) {
            return self.coalesce(left, right, expected);
        }
        if matches!(op, Eq | Ne) {
            let reference = match (&left.kind, &right.kind) {
                (Expression::Null, _) => Some(right),
                (_, Expression::Null) => Some(left),
                _ => None,
            };
            if let Some(reference) = reference {
                let reference = self.expression(reference, None)?;
                return self.null_test(op, reference);
            }
        }
        if matches!(op, And | Or) {
            let left = self.expression(left, Some(Type::Bool))?;
            let refinements = self.null_refinements(&left, matches!(op, And));
            let right = self.with_nonnull(refinements, |checker| {
                checker.expression(right, Some(Type::Bool))
            })?;
            let result = left.ty;
            let constant = Expr {
                kind: ExprKind::Boolean(matches!(op, Or)),
                ty: Type::Bool,
                span: left.span,
            };
            let (yes, no) = if matches!(op, And) {
                (right, constant)
            } else {
                (constant, right)
            };
            let yes = Block {
                ty: yes.ty,
                statements: Vec::new(),
                tail: Some(Box::new(yes)),
            };
            return Ok((
                ExprKind::If {
                    condition: Box::new(left),
                    yes,
                    no: Some(Box::new(no)),
                    result_local: None,
                },
                result,
            ));
        }
        let comparison = matches!(op, Eq | Ne | Lt | Le | Gt | Ge);
        let hint = if comparison {
            None
        } else {
            expected
                .map(|ty| self.types.nullable_underlying(ty).unwrap_or(ty))
                .filter(|ty| {
                    if op.bitwise() {
                        ty.bitwise_integer() || *ty == Type::Bool
                    } else {
                        ty.numeric() || (matches!(op, Add) && *ty == Type::String)
                    }
                })
        };
        // An unadorned literal on the left can take its type from the right operand.
        let (lhs, rhs) =
            if hint.is_none() && left.is_numeric_literal() && !right.is_numeric_literal() {
                let rhs = self.expression(right, None)?;
                let lhs = self.expression(left, (rhs.ty != Type::Never).then_some(rhs.ty))?;
                (lhs, rhs)
            } else {
                let lhs = self.expression(left, hint)?;
                let rhs =
                    self.expression(right, (lhs.ty != Type::Never).then_some(lhs.ty).or(hint))?;
                (lhs, rhs)
            };
        self.binary_operands(op, lhs, rhs)
    }

    fn binary_operands(&self, op: Binary, lhs: Expr, rhs: Expr) -> Checked<(ExprKind, Type)> {
        use Binary::*;
        if op.shift() {
            if !lhs.ty.integer() && lhs.ty != Type::Never {
                return Err(Diagnostic::new(
                    "E0302",
                    lhs.span,
                    "shift requires an integer left operand",
                ));
            }
            let ty = if rhs.ty == Type::Never {
                Type::Never
            } else {
                lhs.ty
            };
            return Ok((ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), ty));
        }
        for operand in [&lhs, &rhs] {
            if op.bitwise() {
                if !operand.ty.bitwise_integer() && !matches!(operand.ty, Type::Bool | Type::Never)
                {
                    return Err(Diagnostic::new(
                        "E0302",
                        operand.span,
                        "bitwise operators require integers, booleans, or CLR enums",
                    ));
                }
            } else if matches!(op, Eq | Ne) {
                if !operand.ty.numeric()
                    && !matches!(
                        operand.ty,
                        Type::Bool | Type::String | Type::Char | Type::ClrEnum(_) | Type::Never
                    )
                {
                    return Err(Diagnostic::new(
                        "E0302",
                        operand.span,
                        "equality requires boolean, character, numeric, string, or CLR enum operands",
                    ));
                }
            } else if !(matches!(op, Add) && operand.ty == Type::String
                || matches!(op, Lt | Le | Gt | Ge) && operand.ty == Type::Char)
            {
                self.require_numeric(operand.ty, operand.span)?;
            }
        }
        let ty = if lhs.ty == Type::Never || rhs.ty == Type::Never {
            Type::Never
        } else if matches!(op, Eq | Ne | Lt | Le | Gt | Ge) {
            Type::Bool
        } else {
            lhs.ty
        };
        Ok((ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), ty))
    }

    fn call(
        &mut self,
        path: &Path<'src>,
        type_arguments: &[Path<'src>],
        args: &[syntax::Expr<'src>],
        expected: Option<Type>,
    ) -> Checked<(ExprKind, Type)> {
        if let [receiver @ .., (method, method_span)] = path.segments.as_slice()
            && !receiver.is_empty()
            && let Some(receiver) = self.path_value(receiver)?
        {
            return self.instance_call(receiver, method, *method_span, type_arguments, args);
        }
        if let Some(variant) =
            self.variant_constructor(&path.segments, type_arguments, args, expected)?
        {
            return Ok(variant);
        }
        if let Some(query) = constants::TypeQuery::from_path(path) {
            return query.lower(self, path, type_arguments, args);
        }
        // Remember unsuccessful name lookups as well. A later declaration
        // can turn an unknown unqualified call into a valid local call.
        if let [(name, _)] = path.segments.as_slice()
            && let Some(lookups) = &mut self.lookups
        {
            lookups.push(Lookup::Function(name));
        }
        let (target, args, result) = match path.segments.as_slice() {
            [("assert", _)] if !self.names.contains_key("assert") => {
                return self.assertion(path.span, type_arguments, args);
            }
            [("System", _), ("Console", _), ("WriteLine", _)] => {
                if !type_arguments.is_empty() {
                    return Err(Diagnostic::new(
                        "E0205",
                        path.span,
                        "System.Console.WriteLine does not accept type arguments",
                    ));
                }
                if args.len() > 1 {
                    return Err(Diagnostic::new(
                        "E0205",
                        path.span,
                        "System.Console.WriteLine expects zero or one argument",
                    ));
                }
                let mut typed = args
                    .iter()
                    .map(|expr| self.expression(expr, None))
                    .collect::<Checked<Vec<_>>>()?;
                let overload = if let Some(arg) = typed.pop() {
                    let overload = match arg.ty {
                        Type::Void => {
                            return Err(Diagnostic::new(
                                "E0205",
                                arg.span,
                                "System.Console.WriteLine requires a value argument",
                            ));
                        }
                        Type::Never => None,
                        ty if ty.narrow_integer() => Some(Type::I32),
                        ty if ty.numeric()
                            || matches!(ty, Type::Bool | Type::String | Type::Char) =>
                        {
                            Some(ty)
                        }
                        Type::NullableString | Type::Null => Some(Type::String),
                        _ => Some(Type::Object),
                    };
                    typed.push(if overload == Some(Type::Object) {
                        self.coerce(arg, Type::NullableObject)
                    } else {
                        arg
                    });
                    overload
                } else {
                    None
                };
                if typed.is_empty() || overload.is_some() {
                    self.record_clr_call(
                        "System.Console.WriteLine",
                        path.span,
                        overload.map(|ty| (ArgumentMode::Value, ty)),
                        Type::Void,
                        OccurrenceKind::Method,
                        false,
                    );
                }
                (Target::WriteLine(overload), typed, Type::Void)
            }
            [(name, span)] if self.names.contains_key(name) => {
                let index = self.names[name];
                let signature = &self.signatures[index];
                let declaration = &self.functions[index];
                if declaration.test && !self.in_test {
                    return Err(Diagnostic::new(
                        "E0346",
                        *span,
                        "an ordinary function cannot call a test function",
                    ));
                }
                self.track_signature(declaration);
                if self.comptime != declaration.comptime {
                    return Err(Diagnostic::new(
                        "E0501",
                        *span,
                        if self.comptime {
                            "comptime code can only call comptime functions"
                        } else {
                            "comptime functions can only be called during constant evaluation"
                        },
                    ));
                }
                let types = self.types;
                self.record(
                    *span,
                    Some(declaration.name_span),
                    OccurrenceKind::Function,
                    || signature_detail(declaration, signature, types),
                );
                if args.len() != signature.parameters.len() {
                    return Err(Diagnostic::new(
                        "E0205",
                        path.span,
                        format!(
                            "'{name}' expects {} arguments, found {}",
                            signature.parameters.len(),
                            args.len()
                        ),
                    )
                    .with_related(declaration.name_span, format!("'{name}' is declared here")));
                }
                let (type_arguments, typed) =
                    if type_arguments.is_empty() && signature.type_arity != 0 {
                        self.infer_call_arguments(declaration, signature, args, expected, path.span)
                            .map_err(|error| explain_argument(error, declaration, args))?
                    } else {
                        if type_arguments.len() != usize::from(signature.type_arity) {
                            return Err(Diagnostic::new(
                                "E0205",
                                path.span,
                                format!(
                                    "'{name}' expects {} type arguments, found {}",
                                    signature.type_arity,
                                    type_arguments.len()
                                ),
                            ));
                        }
                        let type_arguments = self.resolve_type_arguments(type_arguments)?;
                        let typed = args
                            .iter()
                            .zip(&signature.parameters)
                            .zip(&declaration.parameters)
                            .map(|((expr, &ty), parameter)| {
                                if declaration.body.managed().is_none() {
                                    return self.native_argument(expr, parameter, ty);
                                }
                                let expected = types.specialize(
                                    ty,
                                    &type_arguments,
                                    ParameterKind::Method,
                                    expr.span,
                                )?;
                                self.expression(expr, Some(expected))
                            })
                            .collect::<Checked<Vec<_>>>()
                            .map_err(|error| explain_argument(error, declaration, args))?;
                        (type_arguments, typed)
                    };
                if let Some(bound) =
                    self.unsatisfied_bound(signature, &type_arguments, path.span)?
                {
                    let parameter = declaration.type_parameters[usize::from(bound.parameter)].0;
                    return Err(self.unsatisfied_bound_diagnostic(
                        bound,
                        type_arguments[usize::from(bound.parameter)],
                        parameter,
                        path.span,
                    ));
                }
                let result = types.specialize(
                    signature.result,
                    &type_arguments,
                    ParameterKind::Method,
                    path.span,
                )?;
                (
                    Target::Function(index, type_arguments.into_boxed_slice()),
                    typed,
                    result,
                )
            }
            _ => {
                let name = path.qualified_name();
                self.external_call(
                    &name,
                    path.span,
                    &Member::direct(self.references.candidates(&name)),
                    type_arguments,
                    args,
                    None,
                )?
            }
        };
        Ok(self.call_result(target, args, result))
    }

    fn interpolation(
        &mut self,
        parts: &[syntax::Expr<'src>],
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        if i32::try_from(parts.len()).is_err() {
            return Err(Diagnostic::new(
                "E0309",
                span,
                "interpolated string has too many parts",
            ));
        }
        let mut values = Vec::with_capacity(parts.len());
        let mut result = Type::String;
        for part in parts {
            let value = self.expression(part, None)?;
            let value = match value.ty {
                Type::Never => {
                    result = Type::Never;
                    value
                }
                Type::Void => {
                    return Err(Diagnostic::new(
                        "E0312",
                        part.span,
                        "interpolation requires a value",
                    ));
                }
                Type::String => value,
                Type::Null | Type::NullableString => self.coerce(value, Type::NullableString),
                _ if self.comptime => {
                    return Err(Diagnostic::new(
                        "E0501",
                        part.span,
                        "constant interpolation requires string values",
                    ));
                }
                _ => self.coerce(value, Type::NullableObject),
            };
            values.push(value);
        }
        if values.is_empty() {
            return Ok((ExprKind::String(Arc::from("")), Type::String));
        }
        if let [
            Expr {
                kind: ExprKind::String(value),
                ..
            },
        ] = values.as_slice()
        {
            return Ok((ExprKind::String(Arc::clone(value)), Type::String));
        }
        Ok((ExprKind::Interpolation(values), result))
    }

    fn conversion(
        &mut self,
        source: &syntax::Expr<'src>,
        target: &Path<'src>,
        type_test: bool,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        // The enum's backing type determines whether a literal may use the
        // full u64 range. Delay any target error until the source is checked.
        let literal_target = match source.kind {
            Expression::Int { negative, .. } if !type_test && target.suffixes.is_empty() => {
                self.scoped_value_type(target).ok().map(|ty| (ty, negative))
            }
            _ => None,
        };
        let literal_hint = literal_target.and_then(|(target, negative)| {
            let storage = self.references.integer_storage(target);
            (storage.numeric() || storage == Type::Char).then_some(
                if storage == Type::U64 && !negative {
                    Type::U64
                } else {
                    Type::I64
                },
            )
        });
        let value = self.expression(source, literal_hint)?;
        let ty = match literal_target {
            Some((ty, _)) => ty,
            None => self.scoped_value_type(target)?,
        };
        self.record_annotation(target, ty);
        if value.ty == Type::Never {
            return Ok((value.kind, Type::Never));
        }
        if type_test {
            if ty.is_nullable() || self.types.nullable_underlying(ty).is_some() {
                return Err(Diagnostic::new(
                    "E0311",
                    target.span,
                    "type tests use non-nullable runtime types",
                ));
            }
            self.require(value.ty, Type::NullableObject, source.span)?;
            return Ok((
                ExprKind::TypeTest(Box::new(self.coerce(value, Type::NullableObject)), ty),
                Type::Bool,
            ));
        }
        let from = self.references.integer_storage(value.ty);
        let to = self.references.integer_storage(ty);
        if from.numeric() && to.numeric()
            || from == Type::Char && to.integer()
            || from.integer() && to == Type::Char
        {
            return Ok(if value.ty == ty {
                (value.kind, ty)
            } else {
                (ExprKind::NumericCast(Box::new(value), ty), ty)
            });
        }
        if self.comptime {
            return Err(Diagnostic::new(
                "E0501",
                span,
                "constant evaluation supports only numeric casts",
            ));
        }
        if let Some((layout, payload)) = self.option_nullable(value.ty, ty) {
            let value = self.unpack_nullable_option(value, ty, layout, payload)?;
            return Ok((value.kind, value.ty));
        }
        if self.types.nullable_underlying(value.ty) == Some(ty) {
            let member = self.require_nullable_member(value.ty, "get_Value", span)?;
            return self.constrained_call(member, Box::new([]), value, Vec::new(), ty, span);
        }
        if value.ty == ty || self.can_upcast(value.ty, ty) {
            let value = self.coerce(value, ty);
            Ok((value.kind, ty))
        } else if value.ty == Type::Object
            || (matches!(value.ty, Type::NullableObject | Type::Null)
                && self.types.nullable_underlying(ty).is_some())
            || (ty.imported_value() && self.types.upcasts(ty, value.ty))
            || (ty.reference()
                && value.ty.reference()
                && (!value.ty.is_nullable() || ty.is_nullable())
                && self.can_upcast(ty.non_nullable(), value.ty.non_nullable()))
        {
            Ok((ExprKind::CheckedCast(Box::new(value), ty), ty))
        } else {
            let from = describe_scoped(self.types, self.type_parameters, value.ty);
            let to = describe_scoped(self.types, self.type_parameters, ty);
            Err(Diagnostic::new(
                "E0310",
                span,
                format!("cannot cast {from} to {to}"),
            ))
        }
    }

    fn scoped_value_type(&mut self, path: &Path<'src>) -> Checked<Type> {
        self.track_type_path(path);
        self.types
            .scoped_type(path, self.type_parameters, Some(self.bounds), false, false)
    }

    fn track_type_path(&mut self, path: &Path<'src>) {
        if let Some(lookups) = &mut self.lookups {
            analysis::type_names(path, &mut |name| record_type_lookup(lookups, name));
        }
    }

    fn track_signature(&mut self, function: &syntax::Function<'src>) {
        if let Some(lookups) = &mut self.lookups {
            analysis::signature_type_names(function, |name| record_type_lookup(lookups, name));
        }
    }

    fn track_source_type(&mut self, name: &'src str) {
        if let Some(lookups) = &mut self.lookups {
            record_type_lookup(lookups, name);
        }
    }

    fn resolve_type_arguments(&mut self, arguments: &[Path<'src>]) -> Checked<Vec<Type>> {
        arguments
            .iter()
            .map(|path| {
                let ty = self.scoped_value_type(path)?;
                self.record_annotation(path, ty);
                Ok(ty)
            })
            .collect()
    }

    fn call_result(&self, target: Target, args: Vec<Expr>, result: Type) -> (ExprKind, Type) {
        let ty = if args.iter().any(|arg| arg.ty == Type::Never) {
            Type::Never
        } else {
            result
        };
        (ExprKind::Call(target, args), ty)
    }
    fn instance_call(
        &mut self,
        receiver: Expr,
        method: &str,
        span: Span,
        type_arguments: &[Path<'src>],
        args: &[syntax::Expr<'src>],
    ) -> Checked<(ExprKind, Type)> {
        if receiver.ty == Type::Never {
            return Ok((receiver.kind, receiver.ty));
        }
        self.require_nonnull_receiver(receiver.ty, span)?;
        let owner = describe_scoped(self.types, self.type_parameters, receiver.ty);
        let name = format!("{owner}.{method}");
        let lookup = self
            .references
            .builtin_receiver(receiver.ty)
            .map_or(receiver.ty, Type::External);
        let candidates = self.bound_members(lookup, |owner| {
            self.types.instance_candidates(owner, method)
        });
        let (target, args, result) =
            self.external_call(&name, span, &candidates, type_arguments, args, None)?;
        self.call_clr_instance(receiver, target, args, result, span)
    }

    fn call_clr_instance(
        &mut self,
        receiver: Expr,
        target: Target,
        mut args: Vec<Expr>,
        result: Type,
        span: Span,
    ) -> Checked<(ExprKind, Type)> {
        if receiver.ty.scalar()
            || receiver.ty.imported_value()
            || matches!(receiver.ty, Type::MethodParameter(_))
        {
            let Target::External {
                method,
                owner,
                arguments,
            } = target
            else {
                unreachable!("CLR instance member")
            };
            return self.constrained_call(
                Member { id: method, owner },
                arguments,
                receiver,
                args,
                result,
                span,
            );
        }
        args.insert(0, receiver);
        Ok(self.call_result(target, args, result))
    }

    fn external_call(
        &mut self,
        name: &str,
        span: Span,
        candidates: &[Member],
        type_argument_paths: &[Path<'src>],
        args: &[syntax::Expr<'src>],
        constructor: Option<Type>,
    ) -> Checked<(Target, Vec<Expr>, Type)> {
        let resolved = self.resolve_overload(
            name,
            span,
            candidates,
            type_argument_paths,
            args,
            OverloadUse::Method(constructor),
        )?;
        Ok((
            Target::External {
                method: resolved.member.id,
                arguments: resolved.type_arguments,
                owner: resolved.member.owner,
            },
            resolved.arguments,
            resolved.result,
        ))
    }

    fn resolve_overload(
        &mut self,
        name: &str,
        span: Span,
        candidates: &[Member],
        type_argument_paths: &[Path<'src>],
        args: &[syntax::Expr<'src>],
        usage: OverloadUse,
    ) -> Checked<ResolvedOverload> {
        let resolved = self.resolve_type_arguments(type_argument_paths)?;
        let type_arguments = resolved.as_slice();
        let references = self.references;
        let types = self.types;
        let read_dependencies = |member: Member| {
            let method = match usage {
                OverloadUse::Method(None) => Some(member.id),
                OverloadUse::Indexer(PropertyAccess::Get) => {
                    references.accessor(member.id, PropertyAccess::Get)
                }
                _ => None,
            };
            method.map_or(&[][..], |id| references.method(id).nonnull_when.as_ref())
        };
        let parameter_contract = |member: Member, expanded: Option<(usize, Type)>, index: usize| {
            if matches!(usage, OverloadUse::Method(_))
                && expanded.is_none_or(|(fixed, _)| index < fixed)
            {
                references.method(member.id).parameter_nullability(index)
            } else {
                NullabilityOverride::Declared
            }
        };
        if candidates.is_empty() {
            return Err(Diagnostic::new(
                "E0203",
                span,
                format!("unknown function or CLR method '{name}'"),
            ));
        }
        let mut eligible = Vec::new();
        let mut unsupported_signature = None;
        let mut mismatched_modes = false;
        for &member in candidates {
            let (signature, forms) = match usage {
                OverloadUse::Method(_) => {
                    let method = references.method(member.id);
                    if !type_arguments.is_empty()
                        && usize::from(method.signature.type_arity) != type_arguments.len()
                    {
                        continue;
                    }
                    let forms = [
                        method.omitted_defaults(args.len(), false),
                        method.omitted_defaults(args.len(), true),
                    ];
                    if forms.iter().all(Option::is_none) {
                        continue;
                    }
                    if args
                        .iter()
                        .enumerate()
                        .any(|(index, arg)| arg.argument_mode() != method.parameter_mode(index))
                    {
                        mismatched_modes = true;
                        continue;
                    }
                    let Some(signature) = nullability::supported_clr_signature(
                        types.method_signature(member, span),
                        &mut unsupported_signature,
                    )?
                    else {
                        continue;
                    };
                    (signature, forms)
                }
                OverloadUse::Indexer(access) => {
                    let Some(signature) = types.indexer_signature(member, access, span)? else {
                        continue;
                    };
                    if signature.parameters.len() != args.len() {
                        continue;
                    }
                    (signature, [Some(&[][..]), None])
                }
            };
            let [normal, expanded] = forms;
            let variants = normal
                .zip(expanded)
                .map(|(defaults, _)| (signature.clone(), defaults, false))
                .into_iter()
                .chain(
                    expanded
                        .or(normal)
                        .map(|defaults| (signature, defaults, expanded.is_some())),
                );
            for (mut signature, defaults, expanded) in variants {
                let parameter_array = expanded.then(|| {
                    let array = signature.parameters.pop().expect("CLI parameter array");
                    let fixed = signature.parameters.len();
                    let element = types.array_element(array).expect("CLI vector");
                    signature.parameters.resize(fixed.max(args.len()), element);
                    (fixed, array)
                });
                let hints = signature
                    .parameters
                    .iter()
                    .enumerate()
                    .map(|(index, &ty)| match signature.type_arity {
                        0 => Ok(Some(ty)),
                        _ if type_arguments.is_empty() => types.substitute_parameters(
                            ty,
                            ParameterKind::ImportedMethod,
                            span,
                            |_| None,
                        ),
                        _ => types
                            .specialize(ty, type_arguments, ParameterKind::ImportedMethod, span)
                            .map(|ty| {
                                Some(parameter_contract(member, parameter_array, index).apply(ty))
                            }),
                    })
                    .collect::<Checked<Vec<_>>>();
                let Some(hints) =
                    nullability::supported_clr_signature(hints, &mut unsupported_signature)?
                else {
                    continue;
                };
                eligible.push(ExternalOverload {
                    member,
                    signature,
                    hints,
                    defaults,
                    parameter_array,
                });
            }
        }
        if eligible.is_empty() {
            if let Some(error) = unsupported_signature {
                return Err(error);
            }
            if mismatched_modes {
                return Err(Diagnostic::new(
                    "E0213",
                    span,
                    format!("no CLR overload of '{name}' matches the argument modifiers"),
                ));
            }
            let arguments = if type_arguments.is_empty() {
                format!("{} arguments", args.len())
            } else {
                format!(
                    "{} type arguments and {} value arguments",
                    type_arguments.len(),
                    args.len()
                )
            };
            return Err(Diagnostic::new(
                "E0205",
                span,
                format!("no overload of '{name}' accepts {arguments}"),
            ));
        }

        // The ordinary arguments determine method type arguments before the
        // delegate signature is used to check the lambda body.
        let trailing = args.last().and_then(|argument| match &argument.kind {
            Expression::Lambda(lambda) => Some((argument, lambda.parameters.len())),
            _ => None,
        });
        let positional = &args[..args.len() - usize::from(trailing.is_some())];
        let infer = type_arguments.is_empty()
            && eligible
                .iter()
                .any(|candidate| candidate.signature.type_arity != 0);
        let mut typed: Vec<Option<Expr>> = (0..positional.len()).map(|_| None).collect();
        let mut argument_nonnull = eligible
            .iter()
            .any(|candidate| !read_dependencies(candidate.member).is_empty())
            .then(|| vec![false; positional.len()]);
        for index in inference::argument_order(positional, infer) {
            let arg = &positional[index];
            if let Expression::ByRef(mode, name, span) = arg.kind {
                typed[index] = Some(self.reference_argument(mode, name, span)?);
                continue;
            }
            let mut hints = eligible
                .iter()
                .map(|candidate| candidate.hints[index])
                .filter(|hint| match (hint, &arg.kind) {
                    (Some(ty), Expression::Array(_)) => types.array_element(*ty).is_some(),
                    (Some(ty), Expression::Int { .. } | Expression::Float { .. }) => {
                        types.array_element(*ty).is_none()
                    }
                    _ => true,
                });
            let expected = hints
                .next()
                .flatten()
                .filter(|hint| hints.all(|ty| ty == Some(*hint)));
            let checked = if let Some(facts) = &mut argument_nonnull {
                self.expression_value(arg, expected).and_then(|value| {
                    facts[index] = !matches!(
                        value.ty,
                        Type::Null | Type::Never | Type::Void | Type::MethodParameter(_)
                    ) && types.nullable_payload(value.ty).is_none()
                        && expected.is_none_or(|ty| self.option_nullable(value.ty, ty).is_none());
                    self.contextual_conversion(value, expected)
                })
            } else {
                self.expression(arg, expected)
            };
            let value = match checked {
                Err(error) if error.code == "E0303" => {
                    let Expression::Int { digits, negative } = arg.kind else {
                        return Err(error);
                    };
                    let Some(value) = integer_literal(digits, negative) else {
                        return Err(error);
                    };
                    let fallback =
                        [Type::I64, Type::U32, Type::U64]
                            .into_iter()
                            .find(|&fallback| {
                                fallback.contains_integer(value)
                                    && eligible.iter().any(|candidate| {
                                        let signature = &candidate.signature;
                                        if let Some(parameter) = ParameterKind::ImportedMethod
                                            .index(signature.parameters[index])
                                            && signature.type_arity != 0
                                            && type_arguments.is_empty()
                                        {
                                            inference::external_argument_hint(
                                                types, signature, parameter, &typed,
                                            ) == Some(fallback)
                                        } else {
                                            candidate.hints[index] == Some(fallback)
                                        }
                                    })
                            });
                    let Some(fallback) = fallback else {
                        return Err(error);
                    };
                    if let Some(facts) = &mut argument_nonnull {
                        facts[index] = true;
                    }
                    self.expression(arg, Some(fallback))?
                }
                value => value?,
            };
            typed[index] = Some(value);
        }
        let typed: Vec<_> = typed
            .into_iter()
            .map(|value| value.expect("all arguments checked"))
            .collect();
        let mut best = Vec::new();
        let mut best_cost = (usize::MAX, usize::MAX);
        let mut normal_member = None;
        let mut failed_bound = None;
        let mut mismatched_lambda = None;
        for ExternalOverload {
            member,
            signature,
            defaults,
            parameter_array,
            ..
        } in eligible
        {
            // A member's normal form is tested immediately before its expanded form.
            if parameter_array.is_some() && normal_member == Some(member) {
                continue;
            }
            let signature = &signature;
            let instantiation = if type_arguments.is_empty() && signature.type_arity != 0 {
                let Some(arguments) =
                    inference::infer_external_arguments(types, signature, positional, &typed)
                else {
                    continue;
                };
                std::borrow::Cow::Owned(arguments)
            } else {
                std::borrow::Cow::Borrowed(type_arguments)
            };
            let parameters = signature
                .parameters
                .iter()
                .enumerate()
                .map(|(index, &ty)| {
                    types
                        .specialize(ty, &instantiation, ParameterKind::ImportedMethod, span)
                        .map(|ty| parameter_contract(member, parameter_array, index).apply(ty))
                })
                .collect::<Checked<Vec<_>>>();
            let Some(parameters) =
                nullability::supported_clr_signature(parameters, &mut unsupported_signature)?
            else {
                continue;
            };
            let mut candidate = ExternalCandidate {
                member,
                type_arguments: instantiation,
                parameters,
                result: signature.result,
                defaults,
                parameter_array,
            };
            let mut conversions = 0;
            let mut option_conversions = 0;
            let applicable = typed
                .iter()
                .zip(positional)
                .enumerate()
                .all(|(index, (actual, source))| {
                    let expected = candidate.parameters[index];
                    match source.argument_mode() {
                        ArgumentMode::Ref => return expected == actual.ty,
                        ArgumentMode::Out => return expected.non_nullable() == actual.ty.non_nullable(),
                        ArgumentMode::Value => {}
                    }
                    match (expected, actual.ty, &source.kind) {
                        (expected, actual, _) if expected == actual || actual == Type::Never => {
                            true
                        }
                        (expected, actual, _) if self.option_nullable(actual, expected).is_some() => {
                            conversions += 1;
                            option_conversions += 1;
                            true
                        }
                        (expected, actual, _) if self.can_upcast(actual, expected) => {
                            conversions += 1;
                            true
                        }
                        (expected, ty, Expression::Int { .. })
                            if matches!(actual.kind, ExprKind::Integer(value) if expected.contains_integer(ty.integer_value(value))) =>
                        {
                            conversions += 1;
                            true
                        }
                        (Type::F32, Type::F64, Expression::Float { digits, negative })
                            if float_literal(digits, *negative, Type::F32).is_some() =>
                        {
                            conversions += 1;
                            true
                        }
                        _ => false,
                    }
                });
            if applicable
                && defaults
                    .iter()
                    .zip(&candidate.parameters[args.len()..])
                    .all(|(value, &ty)| self.default_argument_type(value, ty).is_some())
            {
                if let Some((argument, arity)) = trailing {
                    let Some(delegate) = nullability::supported_clr_signature(
                        self.delegate_signature(
                            candidate.parameters[positional.len()],
                            argument.span,
                        ),
                        &mut unsupported_signature,
                    )?
                    else {
                        continue;
                    };
                    if !delegate.is_some_and(|(_, signature)| signature.parameters.len() == arity) {
                        mismatched_lambda = Some(argument.span);
                        continue;
                    }
                }
                if matches!(usage, OverloadUse::Method(_))
                    && let Some(failure) =
                        self.unsatisfied_clr_bound(member, &candidate.type_arguments, span)?
                {
                    failed_bound.get_or_insert(failure);
                    continue;
                }
                let specialize = |ty| {
                    types.specialize(
                        ty,
                        &candidate.type_arguments,
                        ParameterKind::ImportedMethod,
                        span,
                    )
                };
                let result = match usage {
                    OverloadUse::Method(Some(constructor)) => Ok(constructor),
                    OverloadUse::Method(None) => specialize(candidate.result)
                        .map(|ty| references.method(member.id).read_nullability.apply(ty)),
                    OverloadUse::Indexer(_) => specialize(candidate.result),
                };
                let Some(result) =
                    nullability::supported_clr_signature(result, &mut unsupported_signature)?
                else {
                    continue;
                };
                candidate.result = result;
                if let Some((fixed, array)) = candidate.parameter_array {
                    let Some(array) = nullability::supported_clr_signature(
                        specialize(array),
                        &mut unsupported_signature,
                    )?
                    else {
                        continue;
                    };
                    candidate.parameter_array = Some((fixed, array));
                }
                if parameter_array.is_none() {
                    normal_member = Some(member);
                }
                // An exact payload bridge is more specific than boxing an Option.
                let cost = (conversions, conversions - option_conversions);
                if cost > best_cost {
                    continue;
                }
                if cost < best_cost {
                    best.clear();
                    best_cost = cost;
                }
                best.push(candidate);
            }
        }
        let first = best.first().ok_or_else(|| {
            if let Some((bound, actual)) = failed_bound {
                let location = type_argument_paths
                    .get(usize::from(bound.parameter))
                    .map_or(span, |argument| argument.span);
                return self.clr_bound_diagnostic(name, bound, actual, location);
            }
            if let Some(span) = mismatched_lambda {
                return Diagnostic::new(
                    "E0340",
                    span,
                    "no candidate delegate matches the trailing lambda parameter count",
                );
            }
            if let Some(error) = unsupported_signature {
                return error;
            }
            let actual = typed
                .iter()
                .zip(positional)
                .map(|(arg, source)| {
                    format!(
                        "{}{}",
                        source.argument_mode().prefix(),
                        self.types.describe(arg.ty)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            Diagnostic::new(
                "E0205",
                span,
                format!("no overload of '{name}' accepts ({actual})"),
            )
        })?;
        let selected = if best.len() == 1 || typed.iter().any(|arg| arg.ty == Type::Never) {
            first
        } else {
            best.iter()
                .find(|preferred| {
                    best.iter()
                        .filter(|other| other.member != preferred.member)
                        .all(|other| {
                            let mut stricter = false;
                            let compatible = args.iter().enumerate().all(|(index, arg)| {
                                let a = preferred.parameters[index];
                                let b = other.parameters[index];
                                if arg.argument_mode() != ArgumentMode::Value {
                                    return a.non_nullable() == b.non_nullable();
                                }
                                stricter |= a != b;
                                a == b || self.can_upcast(a, b)
                            });
                            if !compatible || stricter {
                                return compatible;
                            }
                            let preferred_rank = (
                                preferred.parameter_array.is_some(),
                                !preferred.type_arguments.is_empty(),
                                !preferred.defaults.is_empty(),
                            );
                            let other_rank = (
                                other.parameter_array.is_some(),
                                !other.type_arguments.is_empty(),
                                !other.defaults.is_empty(),
                            );
                            preferred_rank < other_rank
                        })
                })
                .ok_or_else(|| {
                    Diagnostic::new("E0208", span, format!("ambiguous CLR overload of '{name}'"))
                })?
        };
        let parameter_array = selected.parameter_array;
        let result = if selected.result.is_nullable()
            && read_dependencies(selected.member)
                .iter()
                .map(|&index| usize::from(index))
                .any(|index| {
                    if parameter_array.is_some_and(|(fixed, _)| index == fixed) {
                        return true; // The expanded call builds a non-null array.
                    }
                    if let Some(argument) = typed.get(index) {
                        return argument_nonnull.as_ref().is_some_and(|facts| facts[index])
                            && self
                                .option_nullable(argument.ty, selected.parameters[index])
                                .is_none();
                    }
                    if index < args.len() {
                        return true; // The trailing lambda produces a delegate.
                    }
                    selected
                        .defaults
                        .get(index - args.len())
                        .is_some_and(|value| !matches!(value, MetadataConstant::Null))
                }) {
            selected.result.non_nullable()
        } else {
            selected.result
        };
        if let OverloadUse::Method(constructor) = usage {
            let method = references.method(selected.member.id);
            let kind = if constructor.is_some() {
                OccurrenceKind::Constructor
            } else {
                OccurrenceKind::Method
            };
            let fixed = parameter_array.map_or(selected.parameters.len(), |(fixed, _)| fixed);
            self.record_clr_call(
                name,
                span,
                selected.parameters[..fixed]
                    .iter()
                    .copied()
                    .chain(parameter_array.map(|(_, array)| array))
                    .enumerate()
                    .map(|(index, ty)| (method.parameter_mode(index), ty)),
                result,
                kind,
                method.parameter_array,
            );
        }
        let mut typed: Vec<_> = typed
            .into_iter()
            .enumerate()
            .map(|(index, mut arg)| {
                if args[index].argument_mode() != ArgumentMode::Value {
                    return Ok(arg);
                }
                let expected = selected.parameters[index];
                if let Some((layout, payload)) = self.option_nullable(arg.ty, expected) {
                    return self.unpack_nullable_option(arg, expected, layout, payload);
                }
                if expected == Type::F32
                    && arg.ty == Type::F64
                    && let Expression::Float { digits, negative } = &args[index].kind
                {
                    arg.kind = float_literal(digits, *negative, expected)
                        .expect("selected floating-point literal");
                }
                Ok(self.coerce(arg, expected))
            })
            .collect::<Checked<_>>()?;
        if let Some((argument, _)) = trailing {
            typed.push(self.expression(argument, Some(selected.parameters[positional.len()]))?);
        }
        for (value, &expected) in selected
            .defaults
            .iter()
            .zip(&selected.parameters[args.len()..])
        {
            let ty = self
                .default_argument_type(value, expected)
                .expect("applicable default");
            let kind = if matches!(value, MetadataConstant::Null) && !expected.reference() {
                ExprKind::DefaultValue(self.local(expected, span)?)
            } else {
                constant_expression(value)
            };
            typed.push(self.coerce(Expr { kind, ty, span }, expected));
        }
        if let Some((fixed, array)) = parameter_array {
            let items = typed.split_off(fixed);
            let element = types.array_element(array).expect("specialized CLI vector");
            let ty = if items.iter().any(|item| item.ty == Type::Never) {
                Type::Never
            } else {
                array.non_nullable()
            };
            typed.push(Expr {
                kind: ExprKind::ArrayLiteral { element, items },
                ty,
                span,
            });
        }
        Ok(ResolvedOverload {
            member: selected.member,
            type_arguments: selected.type_arguments.as_ref().into(),
            arguments: typed,
            result,
        })
    }

    fn native_argument(
        &mut self,
        arg: &syntax::Expr<'src>,
        parameter: &syntax::FunctionParameter<'src>,
        expected: Type,
    ) -> Checked<Expr> {
        let expected_mode = parameter.passing.mode();
        if arg.argument_mode() != expected_mode {
            let mode = match expected_mode {
                ArgumentMode::Value => "a by-value",
                ArgumentMode::Ref => "a 'ref'",
                ArgumentMode::Out => "an 'out'",
            };
            return Err(Diagnostic::new(
                "E0213",
                arg.span,
                format!("parameter '{}' expects {mode} argument", parameter.name),
            )
            .with_related(parameter.span, "native parameter declared here"));
        }
        if let Expression::ByRef(mode, name, span) = arg.kind {
            let mut value = self.reference_argument(mode, name, span)?;
            self.types
                .require(value.ty, expected, arg.span, self.type_parameters)?;
            value.span = arg.span;
            Ok(value)
        } else {
            self.expression(arg, Some(expected))
        }
    }

    fn reference_argument(&mut self, mode: ArgumentMode, name: &str, span: Span) -> Checked<Expr> {
        let Some(binding) = self.binding(name, span) else {
            return Err(Diagnostic::new(
                "E0202",
                span,
                format!("unknown local '{name}'"),
            ));
        };
        let ty = match binding.value {
            Value::Local(index) => self.locals[usize::from(index)],
            Value::CapturedCell(_, ty) => ty,
            _ => {
                return Err(Diagnostic::new(
                    "E0213",
                    span,
                    "'ref' and 'out' require a local binding",
                ));
            }
        };
        if !(binding.mutable || mode == ArgumentMode::Out && binding.uninitialized) {
            let error = Diagnostic::new("E0204", span, format!("binding '{name}' is immutable"));
            return Err(if binding.can_make_mutable {
                suggest_mutable(error, binding.definition.start)
            } else {
                error
            });
        }
        let value = ty.clr_struct()
            || self
                .types
                .satisfies_type_bound(ty, self.bounds, TypeBoundKind::Value);
        if !(value || ty.is_nullable()) {
            return Err(Diagnostic::new(
                "E0213",
                span,
                format!(
                    "'{}' requires a value type or nullable reference local",
                    mode.prefix().trim_end()
                ),
            ));
        }
        Ok(Expr {
            kind: ExprKind::LocalAddress(binding.value, mode),
            ty,
            span,
        })
    }

    fn default_argument_type(&self, value: &MetadataConstant, expected: Type) -> Option<Type> {
        if matches!(value, MetadataConstant::Null) {
            // A null CLI constant also represents default(T) in optional value parameters.
            return (expected.reference()
                || expected.scalar()
                || expected.imported_value()
                || matches!(
                    expected,
                    Type::Struct(_) | Type::ConstructedStruct(_) | Type::MethodParameter(_)
                ))
            .then_some(expected);
        }
        if expected.non_nullable() == Type::Object {
            return value.literal_type();
        }
        let enum_storage = match expected {
            Type::ClrEnum(id) => self.references.imported_type(id).enum_storage,
            _ => None,
        };
        value.bind(expected, enum_storage)
    }

    fn require_numeric(&self, ty: Type, span: Span) -> Checked<()> {
        if ty.numeric() || ty == Type::Never {
            Ok(())
        } else {
            Err(Diagnostic::new(
                "E0302",
                span,
                format!("expected a number, found {}", self.types.describe(ty)),
            ))
        }
    }
}

fn constant_expression(value: &MetadataConstant) -> ExprKind {
    match value {
        MetadataConstant::Integer(value, _) => ExprKind::Integer(*value),
        MetadataConstant::F32(value) => ExprKind::Float32(*value),
        MetadataConstant::F64(value) => ExprKind::Float64(*value),
        MetadataConstant::Bool(value) => ExprKind::Boolean(*value),
        MetadataConstant::String(value) => ExprKind::String(Arc::clone(value)),
        MetadataConstant::Null => ExprKind::Null,
    }
}

fn integer_literal(digits: &str, negative: bool) -> Option<i128> {
    let (radix, digits) = syntax::integer_parts(digits);
    let digits = without_separators(digits);
    let magnitude = i128::from(u64::from_str_radix(&digits, radix).ok()?);
    Some(if negative { -magnitude } else { magnitude })
}

fn float_literal(digits: &str, negative: bool, ty: Type) -> Option<ExprKind> {
    let digits = without_separators(digits);
    if ty == Type::F32 {
        let value = digits
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())?;
        Some(ExprKind::Float32(if negative { -value } else { value }))
    } else {
        let value = digits
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())?;
        Some(ExprKind::Float64(if negative { -value } else { value }))
    }
}

fn without_separators(digits: &str) -> Cow<'_, str> {
    if digits.contains('_') {
        Cow::Owned(digits.replace('_', ""))
    } else {
        Cow::Borrowed(digits)
    }
}

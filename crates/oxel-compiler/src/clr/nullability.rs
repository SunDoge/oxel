//! Decodes CLR nullable annotations and directional nullability contracts.

use std::collections::HashMap;

use clrmeta::{Metadata, reader::Reader, tables::TableId};

use super::{PropertyAccess, SignatureType, Type, TypeBound, TypeBoundKind, parameter_rows};

const NAMESPACE: &str = "System.Runtime.CompilerServices";

enum Flags {
    Uniform(u8),
    Sequence(Box<[u8]>),
}

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum NullabilityOverride {
    #[default]
    Declared,
    NonNull,
    Nullable,
}

impl NullabilityOverride {
    pub(super) fn for_signature(
        self,
        ty: Type,
        owner_bounds: &[TypeBound],
        method_bounds: &[TypeBound],
    ) -> Self {
        if self == Self::Declared {
            return self;
        }
        let (parameter, bounds) = match ty.non_nullable() {
            Type::TypeParameter(index) => (index, owner_bounds),
            Type::ImportedMethodParameter(index) => (index, method_bounds),
            _ => return self,
        };
        if bounds
            .iter()
            .any(|bound| bound.parameter == parameter && bound.kind == TypeBoundKind::Value)
        {
            Self::Declared
        } else {
            self
        }
    }

    pub(crate) fn apply(self, ty: Type) -> Type {
        match self {
            Self::Declared => ty,
            Self::Nullable => ty.nullable().unwrap_or(ty),
            Self::NonNull => ty.non_nullable(),
        }
    }

    fn merge_writes(self, other: Self) -> Self {
        if self == Self::NonNull || other == Self::NonNull {
            Self::NonNull
        } else {
            self.max(other)
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ArgumentPostcondition {
    pub not_null: u8,
    pub maybe_null: u8,
    pub does_not_return_if: u8,
}

#[derive(Default)]
pub(super) struct Annotations {
    explicit: HashMap<(u8, u32), Flags>,
    contexts: HashMap<(u8, u32), u8>,
    reads: HashMap<(u8, u32), NullabilityOverride>,
    writes: HashMap<(u8, u32), NullabilityOverride>,
    conditional_reads: HashMap<(u8, u32), Vec<Box<str>>>,
    argument_postconditions: HashMap<u32, ArgumentPostcondition>,
}

impl Annotations {
    pub(super) fn collect(metadata: &Metadata) -> Self {
        let mut annotations = Self::default();
        for attribute in &metadata.custom_attributes {
            let Some((namespace, name, signature)) =
                super::attributes::constructor(metadata, attribute.attr_type)
            else {
                continue;
            };
            let Some(parent) = attribute.parent.table else {
                continue;
            };
            if namespace == "System.Diagnostics.CodeAnalysis"
                && matches!(
                    name,
                    "NotNullWhenAttribute" | "MaybeNullWhenAttribute" | "DoesNotReturnIfAttribute"
                )
                && signature == [0x20, 1, 0x01, 0x02] // instance void .ctor(bool)
                && parent == TableId::Param
            {
                if let Ok([1, 0, result @ 0..=1, 0, 0]) = metadata.blobs.get(attribute.value) {
                    let entry = annotations
                        .argument_postconditions
                        .entry(attribute.parent.row)
                        .or_default();
                    let mask = 1 << result;
                    match name {
                        "NotNullWhenAttribute" => entry.not_null |= mask,
                        "MaybeNullWhenAttribute" => entry.maybe_null |= mask,
                        _ => entry.does_not_return_if |= mask,
                    };
                }
                continue;
            }
            if namespace == "System.Diagnostics.CodeAnalysis"
                && name == "NotNullIfNotNullAttribute"
                && signature == [0x20, 1, 0x01, 0x0e] // instance void .ctor(string)
                && matches!(parent, TableId::Param | TableId::Property)
            {
                if let Ok(blob) = metadata.blobs.get(attribute.value)
                    && let Some(name) = super::attributes::string_argument(blob)
                    && !name.is_empty()
                {
                    annotations
                        .conditional_reads
                        .entry((parent as u8, attribute.parent.row))
                        .or_default()
                        .push(name.into());
                }
                continue;
            }
            if namespace == "System.Diagnostics.CodeAnalysis"
                && signature == [0x20, 0, 0x01] // instance void .ctor()
                && matches!(parent, TableId::Param | TableId::Property | TableId::Field)
            {
                if metadata.blobs.get(attribute.value).ok() == Some(&[1, 0, 0, 0][..]) {
                    use NullabilityOverride::{NonNull, Nullable};
                    let key = (parent as u8, attribute.parent.row);
                    match name {
                        "MaybeNullAttribute" => {
                            annotations.reads.insert(key, Nullable);
                        }
                        "NotNullAttribute" => {
                            annotations.reads.entry(key).or_insert(NonNull);
                        }
                        "AllowNullAttribute" => {
                            annotations.writes.entry(key).or_insert(Nullable);
                        }
                        "DisallowNullAttribute" => {
                            annotations.writes.insert(key, NonNull);
                        }
                        _ => {}
                    }
                }
                continue;
            }
            if namespace != NAMESPACE {
                continue;
            }
            let array = match signature {
                [0x20, 1, 0x01, 0x05] => false,      // instance void .ctor(byte)
                [0x20, 1, 0x01, 0x1d, 0x05] => true, // instance void .ctor(byte[])
                _ => continue,
            };
            let Ok(blob) = metadata.blobs.get(attribute.value) else {
                continue;
            };
            let Some(flags) = nullable_flags(blob, array) else {
                continue;
            };
            let key = (parent as u8, attribute.parent.row);
            match (name, parent, array) {
                (
                    "NullableAttribute",
                    TableId::Param
                    | TableId::Property
                    | TableId::Field
                    | TableId::TypeDef
                    | TableId::InterfaceImpl,
                    _,
                ) => {
                    annotations.explicit.insert(key, flags);
                }
                ("NullableContextAttribute", TableId::TypeDef | TableId::MethodDef, false) => {
                    if let Flags::Uniform(flag) = flags {
                        annotations.contexts.insert(key, flag);
                    }
                }
                _ => {}
            }
        }
        annotations
    }

    pub(super) fn argument_postconditions(
        &self,
        metadata: &Metadata,
        method: u32,
        parameters: &[SignatureType],
        modes: &[crate::ArgumentMode],
        boolean_result: bool,
    ) -> Box<[(u16, ArgumentPostcondition)]> {
        if parameters.is_empty()
            || (self.reads.is_empty() && self.argument_postconditions.is_empty())
        {
            return Box::new([]);
        }
        parameter_rows(metadata, method)
            .filter_map(|(id, row)| {
                let index = row.sequence.checked_sub(1)?;
                let slot = usize::from(index);
                let parameter = parameters.get(slot)?;
                let mut contract = self
                    .argument_postconditions
                    .get(&id)
                    .copied()
                    .unwrap_or_default();
                if !boolean_result {
                    contract.not_null = 0;
                    contract.maybe_null = 0;
                }
                if !matches!(parameter, SignatureType::Primitive(Type::Bool))
                    || modes
                        .get(slot)
                        .is_some_and(|&mode| mode != crate::ArgumentMode::Value)
                {
                    contract.does_not_return_if = 0;
                }
                let has_conditional = contract.not_null != 0 || contract.maybe_null != 0;
                let read = self.read(TableId::Param, id);
                // Unconditional contracts cover both outcomes, including calls without a boolean result.
                match read {
                    NullabilityOverride::Nullable => contract.maybe_null |= 0b11,
                    NullabilityOverride::NonNull => contract.not_null |= 0b11,
                    NullabilityOverride::Declared => {}
                }
                (has_conditional || contract.not_null != 0 || contract.does_not_return_if != 0)
                    .then_some((index, contract))
            })
            .collect()
    }

    pub(super) fn method_read_dependencies(
        &self,
        metadata: &Metadata,
        method: u32,
        property: Option<(u32, PropertyAccess)>,
        count: usize,
        modes: &[crate::ArgumentMode],
    ) -> Box<[u16]> {
        if self.conditional_reads.is_empty() || count == 0 {
            return Box::new([]);
        }
        let dependencies = |table: TableId, row| {
            self.conditional_reads
                .get(&(table as u8, row))
                .map_or(&[][..], Vec::as_slice)
        };
        let on_property = match property {
            Some((row, PropertyAccess::Get)) => dependencies(TableId::Property, row),
            _ => &[],
        };
        let on_return = parameter_rows(metadata, method)
            .find(|(_, row)| row.sequence == 0)
            .map_or(&[][..], |(id, _)| dependencies(TableId::Param, id));
        if on_return.is_empty() && on_property.is_empty() {
            return Box::new([]);
        }
        let mut inputs = parameter_rows(metadata, method)
            .filter_map(|(_, row)| {
                let index = row.sequence.checked_sub(1)?;
                let slot = usize::from(index);
                if slot >= count
                    || modes
                        .get(slot)
                        .is_some_and(|&mode| mode != crate::ArgumentMode::Value)
                {
                    return None;
                }
                let name = metadata.strings.get(row.name).ok()?;
                on_return
                    .iter()
                    .chain(on_property)
                    .any(|dependency| dependency.as_ref() == name)
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        inputs.sort_unstable();
        inputs.dedup();
        inputs.into()
    }

    pub(super) fn field_read(&self, row: u32) -> NullabilityOverride {
        self.read(TableId::Field, row)
    }

    pub(super) fn field_write(&self, row: u32) -> NullabilityOverride {
        self.write(TableId::Field, row)
    }

    pub(super) fn method_read(
        &self,
        metadata: &Metadata,
        method: u32,
        property: Option<(u32, PropertyAccess)>,
    ) -> NullabilityOverride {
        if self.reads.is_empty() {
            return NullabilityOverride::Declared;
        }
        let property = match property {
            Some((row, PropertyAccess::Get)) => self.read(TableId::Property, row),
            _ => NullabilityOverride::Declared,
        };
        let result = parameter_rows(metadata, method)
            .find(|(_, row)| row.sequence == 0)
            .map_or(NullabilityOverride::Declared, |(id, _)| {
                self.read(TableId::Param, id)
            });
        property.max(result)
    }

    pub(super) fn method_writes(
        &self,
        metadata: &Metadata,
        method: u32,
        property: Option<(u32, PropertyAccess)>,
        count: usize,
        modes: &[crate::ArgumentMode],
    ) -> Box<[NullabilityOverride]> {
        use crate::ArgumentMode::Value;

        if self.writes.is_empty() || count == 0 {
            return Box::new([]);
        }
        let mut overrides = Vec::new();
        for (id, row) in parameter_rows(metadata, method) {
            let Some(index) = usize::from(row.sequence).checked_sub(1) else {
                continue;
            };
            if index >= count || modes.get(index).is_some_and(|&mode| mode != Value) {
                continue;
            }
            let contract = self.write(TableId::Param, id);
            if contract != NullabilityOverride::Declared {
                overrides.resize(
                    overrides.len().max(index + 1),
                    NullabilityOverride::Declared,
                );
                overrides[index] = overrides[index].merge_writes(contract);
            }
        }
        if let Some((row, PropertyAccess::Set)) = property {
            let contract = self.write(TableId::Property, row);
            if contract != NullabilityOverride::Declared {
                overrides.resize(count, NullabilityOverride::Declared);
                overrides[count - 1] = overrides[count - 1].merge_writes(contract);
            }
        }
        overrides.into()
    }

    fn read(&self, table: TableId, row: u32) -> NullabilityOverride {
        self.reads
            .get(&(table as u8, row))
            .copied()
            .unwrap_or_default()
    }

    fn write(&self, table: TableId, row: u32) -> NullabilityOverride {
        self.writes
            .get(&(table as u8, row))
            .copied()
            .unwrap_or_default()
    }

    pub(super) fn parent(
        &self,
        signature: &mut SignatureType,
        owner: u32,
        interface: Option<u32>,
    ) -> bool {
        let owner_key = (TableId::TypeDef as u8, owner);
        let key = interface.map_or(owner_key, |row| (TableId::InterfaceImpl as u8, row));
        apply_flags(
            signature,
            self.explicit.get(&key),
            self.contexts.get(&owner_key).copied().unwrap_or(0),
        )
    }

    pub(super) fn field(&self, signature: &mut SignatureType, owner: u32, field: u32) -> bool {
        apply_flags(
            signature,
            self.explicit.get(&(TableId::Field as u8, field)),
            self.contexts
                .get(&(TableId::TypeDef as u8, owner))
                .copied()
                .unwrap_or(0),
        )
    }

    pub(super) fn apply(
        &self,
        metadata: &Metadata,
        owner: u32,
        method: u32,
        property: Option<(u32, PropertyAccess)>,
        parameters: &mut [SignatureType],
        result: &mut SignatureType,
    ) -> bool {
        if self.explicit.is_empty() && self.contexts.is_empty() {
            return true;
        }
        let context = self
            .contexts
            .get(&(TableId::MethodDef as u8, method))
            .or_else(|| self.contexts.get(&(TableId::TypeDef as u8, owner)))
            .copied()
            .unwrap_or(0);
        let (property_flag, property_access) = match property {
            Some((row, access)) => (
                self.explicit.get(&(TableId::Property as u8, row)),
                Some(access),
            ),
            None => (None, None),
        };
        let mut explicit = parameter_rows(metadata, method)
            .filter_map(|(id, row)| {
                self.explicit
                    .get(&(TableId::Param as u8, id))
                    .map(|flags| (row.sequence, flags))
            })
            .peekable();
        let result_flag = explicit
            .next_if(|&(sequence, _)| sequence == 0)
            .map(|(_, flag)| flag)
            .or_else(|| {
                matches!(property_access, Some(PropertyAccess::Get))
                    .then_some(property_flag)
                    .flatten()
            });
        if !apply_flags(result, result_flag, context) {
            return false;
        }
        let last = parameters.len().saturating_sub(1);
        for (index, ty) in parameters.iter_mut().enumerate() {
            let flag = explicit
                .next_if(|&(sequence, _)| usize::from(sequence) == index + 1)
                .map(|(_, flag)| flag)
                .or_else(|| {
                    (index == last && matches!(property_access, Some(PropertyAccess::Set)))
                        .then_some(property_flag)
                        .flatten()
                });
            if !apply_flags(ty, flag, context) {
                return false;
            }
        }
        true
    }
}

fn apply_flags(ty: &mut SignatureType, flags: Option<&Flags>, context: u8) -> bool {
    match flags {
        Some(Flags::Sequence(flags)) => walk(ty, &mut flags.iter().copied()),
        Some(Flags::Uniform(flag)) => walk(ty, &mut std::iter::repeat(*flag)),
        None => walk(ty, &mut std::iter::repeat(context)),
    }
}

fn walk(ty: &mut SignatureType, flags: &mut impl Iterator<Item = u8>) -> bool {
    match ty {
        SignatureType::Value { link, arguments } => {
            // A generic value type occupies one placeholder before its arguments.
            arguments.is_empty()
                || ((link.nullable_value() || flags.next().is_some())
                    && arguments.iter_mut().all(|arg| walk(arg, flags)))
        }
        SignatureType::Primitive(value) => {
            let Some(nullable) = value.nullable() else {
                return true;
            };
            match flags.next() {
                Some(2) => *value = nullable,
                Some(_) => {}
                None => return false,
            }
            true
        }
        SignatureType::Array { element, nullable } => {
            let Some(flag) = flags.next() else {
                return false;
            };
            *nullable = flag == 2;
            walk(element, flags)
        }
        SignatureType::Class {
            arguments,
            nullable,
            ..
        } => {
            let Some(flag) = flags.next() else {
                return false;
            };
            *nullable = flag == 2;
            arguments.iter_mut().all(|argument| walk(argument, flags))
        }
    }
}

fn nullable_flags(blob: &[u8], array: bool) -> Option<Flags> {
    let mut reader = Reader::new(blob);
    if reader.read_u16().ok()? != 1 {
        return None;
    }
    let flags = if array {
        let count = reader.read_u32().ok()? as usize;
        let flags = reader.read_bytes(count).ok()?;
        if flags.is_empty() || !flags.iter().all(|&flag| flag <= 2) {
            return None;
        }
        Flags::Sequence(flags.into())
    } else {
        let flag = reader.read_u8().ok()?;
        if flag > 2 {
            return None;
        }
        Flags::Uniform(flag)
    };
    (reader.read_u16().ok()? == 0 && reader.position() == blob.len()).then_some(flags)
}

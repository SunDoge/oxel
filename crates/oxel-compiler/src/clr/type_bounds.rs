//! Decodes and links supported constraints on CLR generic parameters.

use std::{collections::HashMap, sync::Arc};

use clrmeta::tables::GenericParamRow;

use super::{
    ClassReference, CodedIndex, CodedIndexKind, LocalType, Metadata, Reader, References,
    SignatureType, TableId, Type, class_link, framework_type_ref, signature_type,
};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TypeBound {
    pub parameter: u16,
    pub kind: TypeBoundKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TypeBoundKind {
    Reference,
    Value,
    Unmanaged,
    Constructor,
    Target(Type),
}

impl TypeBoundKind {
    pub(crate) fn special_flag(self) -> u16 {
        match self {
            Self::Reference => 4,
            Self::Value => 8,
            Self::Constructor => 16,
            Self::Unmanaged | Self::Target(_) => 0,
        }
    }
}

#[derive(Default)]
pub(super) struct PendingBounds {
    special: Vec<TypeBound>,
    targets: Vec<(u16, SignatureType)>,
}

pub(super) struct Pending {
    owner: usize,
    bounds: PendingBounds,
}

pub(super) struct BoundReader<'a> {
    pub metadata: &'a Metadata,
    pub types: &'a [Option<LocalType>],
    pub type_refs: &'a [Option<Arc<ClassReference>>],
    pub owner_arity: u16,
    pub method_arity: u16,
}

impl BoundReader<'_> {
    fn is_unmanaged_constraint(&self, index: CodedIndex) -> bool {
        let read = || {
            if index.table != Some(TableId::TypeSpec) {
                return None;
            }
            let spec = self.metadata.get_type_spec(index.row)?;
            let mut signature = Reader::new(self.metadata.blobs.get(spec.signature).ok()?);
            // Roslyn represents unmanaged with modreq(UnmanagedType) class ValueType.
            if signature.read_u8().ok()? != 0x1f {
                return None;
            }
            let modifier = CodedIndex::decode(
                CodedIndexKind::TypeDefOrRef,
                signature.read_compressed_uint().ok()?,
            );
            if !framework_type_ref(
                self.metadata,
                modifier,
                "System.Runtime.InteropServices",
                "UnmanagedType",
            ) || signature.read_u8().ok()? != 0x12
            {
                return None;
            }
            let value = CodedIndex::decode(
                CodedIndexKind::TypeDefOrRef,
                signature.read_compressed_uint().ok()?,
            );
            (signature.remaining() == 0
                && framework_type_ref(self.metadata, value, "System", "ValueType"))
            .then_some(())
        };
        read().is_some()
    }

    fn signature(&self, index: CodedIndex) -> Option<SignatureType> {
        if index.table != Some(TableId::TypeSpec) {
            return Some(SignatureType::Class {
                link: class_link(index, self.types, self.type_refs)?,
                arguments: Vec::new(),
                nullable: false,
            });
        }
        let row = self.metadata.get_type_spec(index.row)?;
        let mut reader = Reader::new(self.metadata.blobs.get(row.signature).ok()?);
        let target = signature_type(
            &mut reader,
            self.method_arity,
            self.owner_arity,
            self.types,
            self.type_refs,
            0,
        )?;
        (reader.remaining() == 0
            && matches!(
                target,
                SignatureType::Class { .. }
                    | SignatureType::Primitive(
                        Type::TypeParameter(_) | Type::ImportedMethodParameter(_)
                    )
            ))
        .then_some(target)
    }
}

impl PendingBounds {
    pub(super) fn add_parameter(
        &mut self,
        parameter: &GenericParamRow,
        constraints: &[CodedIndex],
        reader: &BoundReader<'_>,
    ) -> Option<()> {
        for (bit, kind) in [
            (4, TypeBoundKind::Reference),
            (8, TypeBoundKind::Value),
            (16, TypeBoundKind::Constructor),
        ] {
            // CLR metadata includes the constructor flag for a struct constraint.
            // That implicit flag belongs to the imported value constraint.
            if parameter.flags & bit != 0
                && !(kind == TypeBoundKind::Constructor && parameter.flags & 8 != 0)
            {
                self.special.push(TypeBound {
                    parameter: parameter.number,
                    kind,
                });
            }
        }
        for &constraint in constraints {
            // C# emits a ValueType bound alongside the struct flag.
            if parameter.flags & 8 != 0 && reader.is_unmanaged_constraint(constraint) {
                self.special.push(TypeBound {
                    parameter: parameter.number,
                    kind: TypeBoundKind::Unmanaged,
                });
                continue;
            }
            if parameter.flags & 8 != 0
                && framework_type_ref(reader.metadata, constraint, "System", "ValueType")
            {
                continue;
            }
            self.targets
                .push((parameter.number, reader.signature(constraint)?));
        }
        Some(())
    }

    pub(super) fn bind(&self, references: &mut References) -> Option<Vec<TypeBound>> {
        let mut bounds = self.special.clone();
        for (parameter, target) in &self.targets {
            // Constraints may name the very type being linked.
            let target = references.bind_signature_type(target, false)?;
            if let Type::External(id) = target
                && references.is_value_base(id)
                && references.imported_type(id).name == "ValueType"
                && self.special.iter().any(|bound| {
                    bound.parameter == *parameter && bound.kind == TypeBoundKind::Value
                })
            {
                // System.Runtime's reference assembly names its own ValueType
                // with a TypeDef rather than an external TypeRef.
                continue;
            }
            bounds.push(TypeBound {
                parameter: *parameter,
                kind: TypeBoundKind::Target(target),
            });
        }
        Some(bounds)
    }
}

impl References {
    pub(super) fn import_type_bounds(
        &mut self,
        metadata: &Metadata,
        type_ids: &[Option<usize>],
        signature_types: &[Option<LocalType>],
        type_refs: &[Option<Arc<ClassReference>>],
        rows: &HashMap<u32, Vec<CodedIndex>>,
    ) {
        let mut definitions = HashMap::new();
        for (index, parameter) in metadata.generic_params.iter().enumerate() {
            if parameter.owner.table != Some(TableId::TypeDef) {
                continue;
            }
            let Some(owner) = parameter
                .owner
                .row
                .checked_sub(1)
                .and_then(|row| type_ids.get(row as usize).copied().flatten())
            else {
                continue;
            };
            if self.types[owner].bounds.is_none() {
                continue;
            }
            let constraints = rows.get(&(index as u32 + 1)).map_or(&[][..], Vec::as_slice);
            let flags = parameter.flags & 0x1c;
            if flags == 0 && constraints.is_empty() {
                continue;
            }
            let pending = definitions
                .entry(owner)
                .or_insert_with(|| Some(PendingBounds::default()));
            let reader = BoundReader {
                metadata,
                types: signature_types,
                type_refs,
                owner_arity: self.types[owner].arity,
                method_arity: 0,
            };
            if let Some(bounds) = pending
                && bounds
                    .add_parameter(parameter, constraints, &reader)
                    .is_none()
            {
                *pending = None;
            }
        }
        let mut pending = std::mem::take(&mut self.pending_type_bounds);
        for (owner, bounds) in definitions {
            self.types[owner].bounds = None;
            pending.extend(bounds.map(|bounds| Pending { owner, bounds }));
        }
        for item in pending {
            if let Some(bounds) = item.bounds.bind(self) {
                self.types[item.owner].bounds = Some(bounds);
            } else {
                self.pending_type_bounds.push(item);
            }
        }
    }
}

//! Imports public CLR storage fields without loading the declaring assembly.

use super::constants::ConstantRows;
use super::nullability::{Annotations, NullabilityOverride};
use super::{
    ClassReference, CodedIndex, CodedIndexKind, LocalType, Metadata, MetadataConstant, Reader,
    References, SignatureType, TableId, Type, qualify, signature_type,
};
use clrmeta::tables::FieldRow;
use std::sync::Arc;

pub(crate) struct Field {
    pub owner: usize,
    pub name: String,
    pub ty: Option<Type>,
    pub read_nullability: NullabilityOverride,
    pub write_nullability: NullabilityOverride,
    pub is_static: bool,
    pub readonly: bool,
    pub volatile: bool,
    pub constant: Option<MetadataConstant>,
}

pub(super) fn enum_storage(metadata: &Metadata, owner: usize) -> Option<u8> {
    let mut instance_fields =
        class_fields(metadata, owner).filter(|(_, field)| field.flags & 0x10 == 0);
    let (_, field) = instance_fields.next()?;
    if instance_fields.next().is_some()
        || field.flags & 0x0600 != 0x0600
        || metadata.strings.get(field.name).ok()? != "value__"
    {
        return None;
    }
    match metadata.blobs.get(field.signature).ok()? {
        [0x06, kind @ 0x04..=0x0b] => Some(*kind),
        _ => None,
    }
}

fn class_fields(metadata: &Metadata, owner: usize) -> impl Iterator<Item = (u32, &FieldRow)> {
    let indirect = !metadata.field_ptrs.is_empty();
    let count = if indirect {
        metadata.field_ptrs.len()
    } else {
        metadata.fields.len()
    };
    let first = metadata.type_defs[owner].field_list.checked_sub(1);
    let end = metadata
        .type_defs
        .get(owner + 1)
        .map_or(count, |next| next.field_list.saturating_sub(1) as usize);
    let range = first
        .filter(|_| end <= count)
        .map_or(0..0, |first| first as usize..end);
    range.filter_map(move |slot| {
        let row = if indirect {
            metadata.field_ptrs[slot].field
        } else {
            slot as u32 + 1
        };
        let field = metadata.fields.get(row.checked_sub(1)? as usize)?;
        Some((row, field))
    })
}

pub(super) struct Pending {
    id: usize,
    signature: SignatureType,
}

impl References {
    pub(super) fn import_fields(
        &mut self,
        metadata: &Metadata,
        type_ids: &[Option<usize>],
        signature_types: &[Option<LocalType>],
        type_refs: &[Option<Arc<ClassReference>>],
        nullable: &Annotations,
    ) {
        let mut pending = std::mem::take(&mut self.pending_fields);
        let literals = ConstantRows::new(metadata, TableId::Field);
        for (index, _) in metadata.type_defs.iter().enumerate() {
            let Some(owner) = type_ids[index] else {
                continue;
            };
            for (row, field) in class_fields(metadata, index) {
                let is_static = field.flags & 0x10 != 0;
                let imported = &mut self.types[owner];
                if field.flags & 7 != 6
                    || (!is_static
                        && (!(imported.is_reference || imported.value_struct)
                            || imported.is_interface))
                {
                    continue;
                }
                let Ok(name) = metadata.strings.get(field.name) else {
                    continue;
                };
                // Unsupported declarations still hide inherited members.
                let ids = if is_static {
                    self.static_fields
                        .entry(qualify(&imported.qualified_name, name))
                        .or_default()
                } else {
                    imported.fields.entry(name.into()).or_default()
                };
                let id = self.fields.len();
                let literal = field.flags & 0x40 != 0;
                ids.push(id);
                self.fields.push(Field {
                    owner,
                    name: name.into(),
                    ty: None,
                    read_nullability: if literal {
                        NullabilityOverride::Declared
                    } else {
                        nullable.field_read(row)
                    },
                    write_nullability: nullable.field_write(row),
                    is_static,
                    readonly: field.flags & 0x20 != 0 || literal,
                    volatile: false,
                    constant: None,
                });
                let Ok(blob) = metadata.blobs.get(field.signature) else {
                    continue;
                };
                let mut reader = Reader::new(blob);
                if reader.read_u8().ok() != Some(0x06) {
                    continue;
                }
                let volatile = reader.peek_u8().ok() == Some(0x1f);
                if volatile && (literal || !read_volatile(&mut reader, metadata)) {
                    continue;
                }
                let Some(mut signature) = signature_type(
                    &mut reader,
                    0,
                    imported.arity,
                    signature_types,
                    type_refs,
                    0,
                ) else {
                    continue;
                };
                if reader.remaining() != 0
                    || matches!(signature, SignatureType::Primitive(Type::Void))
                    || !nullable.field(&mut signature, index as u32 + 1, row)
                {
                    continue;
                }
                let field = &mut self.fields[id];
                if literal {
                    let constant = literals.get(row);
                    if !is_static || constant.is_none() {
                        continue;
                    }
                    field.constant = constant;
                }
                field.volatile = volatile;
                pending.push(Pending { id, signature });
            }
        }
        for field in pending {
            if let Some(ty) = self.bind_type(&field.signature) {
                let enum_storage = match ty {
                    Type::ClrEnum(id) => self.types[id].enum_storage,
                    _ => None,
                };
                let field = &mut self.fields[field.id];
                let bounds = self.types[field.owner]
                    .bounds
                    .as_deref()
                    .unwrap_or_default();
                field.read_nullability = field.read_nullability.for_signature(ty, bounds, &[]);
                field.write_nullability = field.write_nullability.for_signature(ty, bounds, &[]);
                if !field.readonly && field.write_nullability == NullabilityOverride::Nullable {
                    // Writing null to a field changes the storage directly.
                    field.read_nullability = NullabilityOverride::Nullable;
                }
                field.ty = match &field.constant {
                    Some(constant) => constant.bind(ty, enum_storage),
                    None => Some(ty),
                };
            } else {
                self.pending_fields.push(field);
            }
        }
    }

    pub(crate) fn field(&self, id: usize) -> &Field {
        &self.fields[id]
    }

    pub(crate) fn static_fields(&self, name: &str) -> &[usize] {
        self.static_fields.get(name).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn declared_fields(&self, owner: usize, name: &str) -> &[usize] {
        self.types[owner]
            .fields
            .get(name)
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn declared_instance_fields(&self, owner: usize) -> impl Iterator<Item = &Field> {
        let mut ids = self.types[owner]
            .fields
            .values()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.into_iter().map(|id| &self.fields[id])
    }
}

fn read_volatile(reader: &mut Reader<'_>, metadata: &Metadata) -> bool {
    if reader.read_u8().is_err() {
        return false;
    }
    reader.read_compressed_uint().is_ok_and(|raw| {
        super::framework_type_ref(
            metadata,
            CodedIndex::decode(CodedIndexKind::TypeDefOrRef, raw),
            "System.Runtime.CompilerServices",
            "IsVolatile",
        )
    })
}

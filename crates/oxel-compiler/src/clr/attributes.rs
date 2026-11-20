//! Reads CLR attribute constructor identity and selected metadata values.

use std::collections::{HashMap, HashSet};

use clrmeta::{
    Metadata,
    reader::Reader,
    tables::{CodedIndex, TableId},
};

fn row<T>(rows: &[T], index: u32) -> Option<&T> {
    rows.get(index.checked_sub(1)? as usize)
}

pub(super) fn constructor(
    metadata: &Metadata,
    constructor: CodedIndex,
) -> Option<(&str, &str, &[u8])> {
    let (name, signature, scope) = match constructor.table? {
        TableId::MemberRef => {
            let member = row(&metadata.member_refs, constructor.row)?;
            (member.name, member.signature, member.class)
        }
        TableId::MethodDef => {
            let method = row(&metadata.method_defs, constructor.row)?;
            // TypeDef method ranges are sorted, and empty ranges may share offsets.
            let owner = metadata
                .type_defs
                .partition_point(|ty| ty.method_list <= constructor.row);
            (
                method.name,
                method.signature,
                CodedIndex {
                    table: Some(TableId::TypeDef),
                    row: owner as u32,
                },
            )
        }
        _ => return None,
    };
    if metadata.strings.get(name).ok()? != ".ctor" {
        return None;
    }
    let (namespace, name) = match scope.table? {
        TableId::TypeDef => {
            let ty = row(&metadata.type_defs, scope.row)?;
            (ty.type_namespace, ty.type_name)
        }
        TableId::TypeRef => {
            let ty = row(&metadata.type_refs, scope.row)?;
            (ty.type_namespace, ty.type_name)
        }
        _ => return None,
    };
    Some((
        metadata.strings.get(namespace).ok()?,
        metadata.strings.get(name).ok()?,
        metadata.blobs.get(signature).ok()?,
    ))
}

pub(super) fn string_argument(blob: &[u8]) -> Option<&str> {
    let mut reader = Reader::new(blob);
    if reader.read_u16().ok()? != 1 {
        return None;
    }
    let length = reader.read_compressed_uint().ok()? as usize;
    let value = std::str::from_utf8(reader.read_bytes(length).ok()?).ok()?;
    (reader.read_u16().ok()? == 0 && reader.position() == blob.len()).then_some(value)
}

pub(super) fn default_members(metadata: &Metadata) -> HashMap<u32, &str> {
    metadata
        .custom_attributes
        .iter()
        .filter_map(|attribute| {
            if attribute.parent.table != Some(TableId::TypeDef) {
                return None;
            }
            const DEFAULT_MEMBER: (&str, &str, &[u8]) = (
                "System.Reflection",
                "DefaultMemberAttribute",
                &[0x20, 1, 0x01, 0x0e],
            );
            if constructor(metadata, attribute.attr_type)? != DEFAULT_MEMBER {
                return None;
            }
            let name = string_argument(metadata.blobs.get(attribute.value).ok()?)?;
            Some((attribute.parent.row, name))
        })
        .collect()
}

pub(super) fn byref_like_types(metadata: &Metadata) -> HashSet<u32> {
    metadata
        .custom_attributes
        .iter()
        .filter(|attribute| {
            attribute.parent.table == Some(TableId::TypeDef)
                && matches!(
                    constructor(metadata, attribute.attr_type),
                    Some((
                        "System.Runtime.CompilerServices",
                        "IsByRefLikeAttribute",
                        [0x20, 0, 0x01]
                    ))
                )
        })
        .map(|attribute| attribute.parent.row)
        .collect()
}

pub(super) fn readonly_parameters(metadata: &Metadata) -> HashSet<u32> {
    metadata
        .custom_attributes
        .iter()
        .filter(|attribute| {
            attribute.parent.table == Some(TableId::Param)
                && matches!(
                    constructor(metadata, attribute.attr_type),
                    Some((
                        "System.Runtime.CompilerServices",
                        "IsReadOnlyAttribute" | "RequiresLocationAttribute",
                        [0x20, 0, 0x01]
                    ))
                )
        })
        .map(|attribute| attribute.parent.row)
        .collect()
}

pub(super) fn parameter_arrays(metadata: &Metadata) -> HashSet<u32> {
    metadata
        .custom_attributes
        .iter()
        .filter(|attribute| {
            attribute.parent.table == Some(TableId::Param)
                && matches!(
                    constructor(metadata, attribute.attr_type),
                    Some(("System", "ParamArrayAttribute", [0x20, 0, 0x01]))
                )
        })
        .map(|attribute| attribute.parent.row)
        .collect()
}

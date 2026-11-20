//! Decodes CLI constants used by fields and optional parameters.

use super::{Metadata, TableId, Type, parameter_rows};
use clrmeta::tables::ConstantRow;
use std::{collections::HashMap, sync::Arc};

pub(crate) enum MetadataConstant {
    Integer(i64, u8),
    F32(f32),
    F64(f64),
    Bool(bool),
    String(Arc<str>),
    Null,
}

impl MetadataConstant {
    fn decode(kind: u8, bytes: &[u8]) -> Option<Self> {
        Some(match kind {
            0x02 => match bytes {
                [0] => Self::Bool(false),
                [1] => Self::Bool(true),
                _ => return None,
            },
            0x03 | 0x07 => Self::Integer(u16::from_le_bytes(bytes.try_into().ok()?).into(), kind),
            0x04 => Self::Integer(i8::from_le_bytes(bytes.try_into().ok()?).into(), kind),
            0x05 => Self::Integer(u8::from_le_bytes(bytes.try_into().ok()?).into(), kind),
            0x06 => Self::Integer(i16::from_le_bytes(bytes.try_into().ok()?).into(), kind),
            0x08 => Self::Integer(i32::from_le_bytes(bytes.try_into().ok()?).into(), kind),
            0x09 => Self::Integer(u32::from_le_bytes(bytes.try_into().ok()?).into(), kind),
            0x0a | 0x0b => Self::Integer(i64::from_le_bytes(bytes.try_into().ok()?), kind),
            0x0c => Self::F32(f32::from_le_bytes(bytes.try_into().ok()?)),
            0x0d => Self::F64(f64::from_le_bytes(bytes.try_into().ok()?)),
            0x0e => {
                let (units, trailing) = bytes.as_chunks::<2>();
                if !trailing.is_empty() {
                    return None;
                }
                let utf16 = units.iter().copied().map(u16::from_le_bytes);
                let string = char::decode_utf16(utf16)
                    .collect::<Result<String, _>>()
                    .ok()?;
                Self::String(string.into())
            }
            0x12 if bytes == [0; 4] => Self::Null,
            _ => return None,
        })
    }

    pub(crate) fn literal_type(&self) -> Option<Type> {
        Some(match self {
            Self::Integer(_, kind) => Type::from_clr_element(*kind)?,
            Self::F32(_) => Type::F32,
            Self::F64(_) => Type::F64,
            Self::Bool(_) => Type::Bool,
            Self::String(_) => Type::String,
            Self::Null => Type::Null,
        })
    }

    pub(crate) fn bind(&self, ty: Type, enum_storage: Option<u8>) -> Option<Type> {
        if matches!(self, Self::Integer(_, kind) if enum_storage == Some(*kind)) {
            return Some(ty);
        }
        let expected = self.literal_type()?;
        if expected == Type::Null {
            return ty.nullable();
        }
        (ty.non_nullable() == expected).then_some(expected)
    }
}

pub(super) struct ConstantRows<'a> {
    metadata: &'a Metadata,
    rows: HashMap<u32, Option<&'a ConstantRow>>,
}

impl<'a> ConstantRows<'a> {
    pub fn new(metadata: &'a Metadata, owner: TableId) -> Self {
        let mut rows = HashMap::new();
        for constant in &metadata.constants {
            if constant.parent.table == Some(owner) && constant.padding == 0 {
                rows.entry(constant.parent.row)
                    .and_modify(|entry| *entry = None)
                    .or_insert(Some(constant));
            }
        }
        Self { metadata, rows }
    }

    pub fn get(&self, row: u32) -> Option<MetadataConstant> {
        let constant = self.rows.get(&row).copied().flatten()?;
        let bytes = self.metadata.blobs.get(constant.value).ok()?;
        MetadataConstant::decode(constant.constant_type, bytes)
    }

    pub fn trailing_defaults(&self, method: u32, count: usize) -> Box<[MetadataConstant]> {
        const OPTIONAL_WITH_DEFAULT: u16 = 0x0010 | 0x1000;
        if count == 0 || self.rows.is_empty() {
            return Box::new([]);
        }
        let mut next = count;
        let mut defaults = Vec::new();
        for (id, row) in parameter_rows(self.metadata, method).rev() {
            if usize::from(row.sequence) > count {
                continue;
            }
            if usize::from(row.sequence) != next
                || row.flags & OPTIONAL_WITH_DEFAULT != OPTIONAL_WITH_DEFAULT
            {
                break;
            }
            let Some(value) = self.get(id) else {
                break;
            };
            defaults.push(value);
            next -= 1;
            if next == 0 {
                break;
            }
        }
        defaults.reverse();
        defaults.into_boxed_slice()
    }
}

//! Reads CLR reference identities and supported member signatures without loading code.

mod attributes;
mod constants;
mod events;
mod fields;
mod nullability;
mod type_bounds;

pub(crate) use constants::MetadataConstant;
pub(crate) use events::EventAccess;
pub(crate) use fields::Field;
pub(crate) use nullability::{ArgumentPostcondition, NullabilityOverride};
pub(crate) use type_bounds::{TypeBound, TypeBoundKind};

pub(crate) const BUILTIN_ASSEMBLIES: [&str; 2] = ["System.Runtime", "System.Console"];

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use clrmeta::{
    AssemblyRefInfo, Metadata,
    reader::Reader,
    root::MetadataRoot,
    tables::{CodedIndex, CodedIndexKind, ParamRow, TableId, TablesHeader},
};
use object::{
    FileKind,
    pe::IMAGE_DIRECTORY_ENTRY_COM_DESCRIPTOR,
    read::pe::{ImageNtHeaders, PeFile, PeFile32, PeFile64},
};

use crate::{ArgumentMode, Type, hir::Signature};

#[derive(Default)]
pub struct References {
    assemblies: Vec<Assembly>,
    methods: Vec<Method>,
    properties: Vec<Property>,
    events: Vec<events::Event>,
    fields: Vec<Field>,
    pending_fields: Vec<fields::Pending>,
    static_fields: HashMap<String, Vec<usize>>,
    static_properties: HashMap<String, Vec<usize>>,
    static_events: HashMap<String, Vec<usize>>,
    pending: Vec<PendingMethod>,
    pending_parents: Vec<PendingParent>,
    pending_type_bounds: Vec<type_bounds::Pending>,
    types: Vec<ImportedType>,
    instances: Vec<SignatureInstance>,
    instance_ids: HashMap<SignatureInstance, usize>,
    array_elements: Vec<Type>,
    array_ids: HashMap<Type, usize>,
    type_names: HashMap<String, Vec<usize>>,
    builtin_receivers: HashMap<Type, usize>,
    pub(crate) nullable_value: Option<usize>,
    pub(crate) exception: Option<usize>,
    pub(crate) disposable: Option<usize>,
    pub(crate) enumerable: Option<usize>,
    pub(crate) enumerator: Option<usize>,
    names: HashMap<String, Vec<usize>>,
}

pub(crate) struct Assembly {
    pub name: String,
    pub version: (u16, u16, u16, u16),
    pub culture: Option<String>,
    pub token: Option<[u8; 8]>,
    pub hash: blake3::Hash,
    framework: bool,
    forwarders: HashMap<String, HashMap<String, Arc<AssemblyRefInfo>>>,
}

impl Assembly {
    fn matches_reference(&self, expected: &AssemblyRefInfo) -> bool {
        self.name.eq_ignore_ascii_case(&expected.name)
            && (self.version == expected.version
                || self.framework && self.token.is_some() && self.version > expected.version)
            && self
                .culture
                .as_deref()
                .unwrap_or("")
                .eq_ignore_ascii_case(expected.culture.as_deref().unwrap_or(""))
            && self.token.as_ref().map(|token| token.as_slice())
                == expected.public_key_token.as_deref()
    }
}

pub(crate) struct ImportedType {
    pub assembly: usize,
    pub namespace: String,
    pub name: String,
    pub qualified_name: String,
    pub arity: u16,
    pub bounds: Option<Vec<TypeBound>>,
    pub variance: Vec<Variance>,
    is_reference: bool,
    pub(crate) is_interface: bool,
    pub(crate) is_delegate: bool,
    pub(crate) builtin: Option<Type>,
    pub(crate) enum_storage: Option<u8>,
    pub(crate) value_struct: bool,
    base: Option<Type>,
    interfaces: Vec<Option<Type>>,
    members: HashMap<String, Vec<usize>>,
    properties: HashMap<String, Vec<usize>>,
    events: HashMap<String, Vec<usize>>,
    indexers: Vec<usize>,
    fields: HashMap<String, Vec<usize>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Variance {
    Invariant,
    Covariant,
    Contravariant,
}

#[derive(Default)]
struct GenericType {
    parameters: Vec<Variance>,
    invalid: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Invocation {
    Static,
    Instance,
    Constructor,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PropertyAccess {
    Get,
    Set,
}

struct Property {
    owner: usize,
    invocation: Invocation,
    index_count: usize,
    getter: Option<usize>,
    setter: Option<usize>,
}

pub(crate) struct Method {
    pub owner: usize,
    pub name: String,
    pub signature: Signature,
    pub read_nullability: NullabilityOverride,
    pub nonnull_when: Box<[u16]>,
    pub argument_postconditions: Box<[(u16, ArgumentPostcondition)]>,
    parameter_nullability: Box<[NullabilityOverride]>,
    pub bounds: Vec<TypeBound>,
    pub invocation: Invocation,
    pub hide_by_name: bool,
    pub defaults: Box<[MetadataConstant]>,
    pub modes: Box<[ArgumentMode]>,
    pub parameter_array: bool,
}

impl Method {
    pub fn omitted_defaults(&self, supplied: usize, expanded: bool) -> Option<&[MetadataConstant]> {
        let count = self.signature.parameters.len();
        let omitted = match (self.parameter_array, expanded) {
            (true, true) => (count - 1).saturating_sub(supplied),
            (true, false) if count == supplied => return Some(&[]),
            (true, false) | (false, true) => return None,
            (false, false) => count.checked_sub(supplied)?,
        };
        if self
            .modes
            .get(supplied..)
            .is_some_and(|modes| modes.iter().any(|&mode| mode != ArgumentMode::Value))
        {
            return None;
        }
        self.defaults
            .get(self.defaults.len().checked_sub(omitted)?..)
    }

    pub fn parameter_mode(&self, index: usize) -> ArgumentMode {
        self.modes
            .get(index)
            .copied()
            .unwrap_or(ArgumentMode::Value)
    }

    pub fn parameter_nullability(&self, index: usize) -> NullabilityOverride {
        self.parameter_nullability
            .get(index)
            .copied()
            .unwrap_or_default()
    }
}

struct PendingParent {
    owner: usize,
    interface_slot: Option<usize>,
    signature: SignatureType,
}

struct PendingMethod {
    owner: usize,
    name: String,
    invocation: Invocation,
    hide_by_name: bool,
    type_arity: u16,
    bounds: type_bounds::PendingBounds,
    property: Option<(usize, PropertyAccess)>,
    event: Option<(usize, EventAccess)>,
    result: SignatureType,
    read_nullability: NullabilityOverride,
    nonnull_when: Box<[u16]>,
    argument_postconditions: Box<[(u16, ArgumentPostcondition)]>,
    parameter_nullability: Box<[NullabilityOverride]>,
    parameters: Vec<SignatureType>,
    defaults: Box<[MetadataConstant]>,
    modes: Box<[ArgumentMode]>,
    parameter_array: bool,
}

struct ParsedMethodSignature {
    parameters: Vec<SignatureType>,
    result: SignatureType,
    modes: Vec<ArgumentMode>,
}

#[derive(Default)]
struct GenericMethod {
    parameters: Vec<usize>,
}

enum SignatureType {
    Primitive(Type),
    Value {
        link: ClassLink,
        arguments: Vec<SignatureType>,
    },
    Array {
        element: Box<SignatureType>,
        nullable: bool,
    },
    Class {
        link: ClassLink,
        arguments: Vec<SignatureType>,
        nullable: bool,
    },
}

/// A constructed CLR signature. VAR and MVAR are relative to the
/// declaring class and method, respectively, until lowered by the checker.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct SignatureInstance {
    pub definition: usize,
    pub arguments: Arc<[Type]>,
}

#[derive(Clone)]
enum ClassLink {
    Local(LocalType),
    Imported(Arc<ClassReference>),
}

#[derive(Clone, Copy)]
struct LocalType {
    id: usize,
    nullable_value: bool,
}

impl ClassLink {
    fn nullable_value(&self) -> bool {
        match self {
            Self::Local(ty) => ty.nullable_value,
            Self::Imported(ty) => {
                ty.namespace == "System"
                    && ty.name == "Nullable`1"
                    && [
                        ("System.Runtime", 0xb03f5f7f11d50a3au64),
                        ("mscorlib", 0xb77a5c561934e089),
                        ("netstandard", 0xcc7b13ffcd2ddd51),
                    ]
                    .iter()
                    .any(|(name, key)| {
                        ty.assembly.name.eq_ignore_ascii_case(name)
                            && ty.assembly.public_key_token.as_deref() == Some(&key.to_be_bytes())
                    })
            }
        }
    }
}

struct ClassReference {
    assembly: Arc<AssemblyRefInfo>,
    namespace: String,
    name: String,
    qualified_name: String,
}

fn qualify(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_string()
    } else {
        format!("{namespace}.{name}")
    }
}

impl References {
    /// Index a PE image and return its managed assembly name. The image is never executed.
    pub fn add(&mut self, bytes: &[u8]) -> Result<&str, String> {
        self.add_assembly(bytes, false)
    }

    /// Index a .NET SDK targeting-pack image used only for compilation.
    pub fn add_framework(&mut self, bytes: &[u8]) -> Result<&str, String> {
        self.add_assembly(bytes, true)
    }

    fn add_assembly(&mut self, bytes: &[u8], framework: bool) -> Result<&str, String> {
        let metadata = read_metadata(bytes)?;
        let assembly = metadata.assembly().ok_or("missing Assembly table")?;
        if !(valid_assembly_name(&assembly.name)
            || framework && builtin_assembly(&assembly.name).is_some())
        {
            return Err(format!(
                "invalid or reserved assembly name '{}'",
                assembly.name
            ));
        }
        if self.contains(&assembly.name) {
            return Err(format!("duplicate reference '{}'", assembly.name));
        }
        let token = assembly.public_key_token();
        let core_facade = framework
            && assembly.name.eq_ignore_ascii_case(BUILTIN_ASSEMBLIES[0])
            && token == Some(0xb03f5f7f11d50a3au64.to_be_bytes());
        let assembly_index = self.assemblies.len();
        let assembly_refs: Vec<_> = metadata.assembly_refs().into_iter().map(Arc::new).collect();
        self.assemblies.push(Assembly {
            name: assembly.name,
            version: assembly.version,
            culture: assembly.culture,
            token,
            hash: blake3::hash(bytes),
            framework,
            forwarders: type_forwarders(&metadata, &assembly_refs),
        });
        let mut generic_types = generic_types(&metadata);
        let byref_like = attributes::byref_like_types(&metadata);
        let mut type_ids = vec![None; metadata.type_defs.len()];
        for (index, class) in metadata.type_defs.iter().enumerate() {
            if class.flags & 7 != 1 {
                continue;
            }
            let generic = generic_types
                .remove(&(index as u32 + 1))
                .unwrap_or_default();
            let arity = generic.parameters.len().min(usize::from(u16::MAX)) as u16;
            let (Ok(namespace), Ok(name)) = (
                metadata.strings.get(class.type_namespace),
                metadata.strings.get(class.type_name),
            ) else {
                continue;
            };
            let qualified_name = qualify(namespace, name);
            let framework_parent = system_parent(&metadata, class.extends, core_facade);
            let enum_storage = if arity == 0
                && class.flags & 0x100 != 0 // sealed
                && framework_parent == Some("Enum")
                && (class.extends.table == Some(TableId::TypeDef)
                    || framework_type_ref(&metadata, class.extends, "System", "Enum"))
            {
                fields::enum_storage(&metadata, index)
            } else {
                None
            };
            let value_type = matches!(framework_parent, Some("ValueType" | "Enum"))
                && !(core_facade && namespace == "System" && name == "Enum");
            let builtin = if core_facade && namespace == "System" && arity == 0 {
                Type::clr_builtin(name).filter(|&ty| ty.scalar() || ty.reference())
            } else {
                None
            };
            let value_struct = class.flags & 0x100 != 0
                && builtin.is_none()
                && !(core_facade && namespace == "System" && name == "Void")
                && framework_parent == Some("ValueType")
                && (class.extends.table == Some(TableId::TypeDef)
                    || framework_type_ref(&metadata, class.extends, "System", "ValueType"))
                && !byref_like.contains(&(index as u32 + 1));
            let is_reference = !value_type && class.flags & 0x180 != 0x180;
            let is_interface = class.flags & 0x20 != 0;
            let is_delegate = framework_parent == Some("MulticastDelegate");
            let variant = (is_interface || is_delegate)
                && generic
                    .parameters
                    .iter()
                    .any(|&parameter| parameter != Variance::Invariant);
            let id = self.types.len();
            if core_facade
                && value_struct
                && namespace == "System"
                && name == "Nullable`1"
                && arity == 1
            {
                self.nullable_value = Some(id);
            }
            if core_facade
                && is_reference
                && namespace == "System"
                && name == "Exception"
                && arity == 0
            {
                self.exception = Some(id);
            }
            if core_facade
                && is_interface
                && namespace == "System"
                && name == "IDisposable"
                && arity == 0
            {
                self.disposable = Some(id);
            }
            if core_facade && is_interface {
                match (namespace, name, arity) {
                    ("System.Collections.Generic", "IEnumerable`1", 1) => {
                        self.enumerable = Some(id);
                    }
                    ("System.Collections", "IEnumerator", 0) => {
                        self.enumerator = Some(id);
                    }
                    _ => {}
                }
            }
            if let Some(ty) = builtin {
                self.builtin_receivers.insert(ty, id);
            }
            let generic_static_class = !value_type && arity != 0 && class.flags & 0x180 == 0x180;
            if is_reference || generic_static_class || enum_storage.is_some() || value_struct {
                self.type_names
                    .entry(qualified_name.clone())
                    .or_default()
                    .push(id);
            }
            self.types.push(ImportedType {
                assembly: assembly_index,
                namespace: namespace.into(),
                name: name.into(),
                qualified_name,
                arity,
                bounds: (!generic.invalid).then(Vec::new),
                variance: if variant {
                    generic.parameters
                } else {
                    Vec::new()
                },
                is_reference,
                is_interface,
                is_delegate,
                builtin,
                enum_storage,
                value_struct,
                base: None,
                interfaces: Vec::new(),
                members: HashMap::new(),
                properties: HashMap::new(),
                events: HashMap::new(),
                indexers: Vec::new(),
                fields: HashMap::new(),
            });
            type_ids[index] = Some(id);
        }
        let signature_types: Vec<_> = type_ids
            .iter()
            .map(|id| {
                id.and_then(|id| {
                    let ty = &self.types[id];
                    (ty.is_reference || ty.enum_storage.is_some() || ty.value_struct).then_some(
                        LocalType {
                            id,
                            nullable_value: self.nullable_value == Some(id),
                        },
                    )
                })
            })
            .collect();
        let type_refs: Vec<_> = metadata
            .type_refs
            .iter()
            .map(|reference| {
                if reference.resolution_scope.table != Some(TableId::AssemblyRef) {
                    return None;
                }
                let row = reference.resolution_scope.row.checked_sub(1)?;
                let assembly = assembly_refs.get(row as usize)?;
                if !valid_reference_name(&assembly.name) || assembly.flags & 1 != 0 {
                    return None;
                }
                let namespace = metadata.strings.get(reference.type_namespace).ok()?;
                let name = metadata.strings.get(reference.type_name).ok()?;
                Some(Arc::new(ClassReference {
                    assembly: Arc::clone(assembly),
                    namespace: namespace.into(),
                    name: name.into(),
                    qualified_name: qualify(namespace, name),
                }))
            })
            .collect();
        let nullable = nullability::Annotations::collect(&metadata);
        let constraints = generic_constraints(&metadata);
        self.import_type_bounds(
            &metadata,
            &type_ids,
            &signature_types,
            &type_refs,
            &constraints,
        );
        let mut parents = std::mem::take(&mut self.pending_parents);
        let parent_type = |index, owner_arity| {
            parent_signature(&metadata, index, owner_arity, &signature_types, &type_refs)
        };
        for (index, class) in metadata.type_defs.iter().enumerate() {
            if let Some(owner) = type_ids[index].filter(|&id| {
                let ty = &self.types[id];
                ty.is_reference
                    || ty.builtin.is_some()
                    || ty.enum_storage.is_some()
                    || ty.value_struct
            }) && let Some(mut signature) = parent_type(class.extends, self.types[owner].arity)
                && nullable.parent(&mut signature, index as u32 + 1, None)
            {
                parents.push(PendingParent {
                    owner,
                    interface_slot: None,
                    signature,
                });
            }
        }
        for (index, implementation) in metadata.interface_impls.iter().enumerate() {
            if let Some(owner) = implementation
                .class
                .checked_sub(1)
                .and_then(|row| type_ids.get(row as usize).copied().flatten())
                .filter(|&owner| {
                    let ty = &self.types[owner];
                    ty.is_reference || ty.value_struct || ty.builtin.is_some()
                })
                && let Some(mut signature) =
                    parent_type(implementation.interface, self.types[owner].arity)
                && nullable.parent(&mut signature, implementation.class, Some(index as u32 + 1))
            {
                let interfaces = &mut self.types[owner].interfaces;
                let slot = interfaces.len();
                interfaces.push(None);
                parents.push(PendingParent {
                    owner,
                    interface_slot: Some(slot),
                    signature,
                });
            }
        }
        for parent in parents {
            if let Some(ty) = self.bind_type(&parent.signature) {
                let ty = ty.non_nullable(); // The root of an inheritance edge cannot be null.
                let owner = &mut self.types[parent.owner];
                if let Some(slot) = parent.interface_slot {
                    owner.interfaces[slot] = Some(ty);
                } else {
                    owner.base = Some(ty);
                }
            } else {
                self.pending_parents.push(parent);
            }
        }
        self.import_fields(
            &metadata,
            &type_ids,
            &signature_types,
            &type_refs,
            &nullable,
        );
        let generic_methods = generic_methods(&metadata);
        let readonly_parameters = attributes::readonly_parameters(&metadata);
        let parameter_arrays = attributes::parameter_arrays(&metadata);
        let parameter_defaults = constants::ConstantRows::new(&metadata, TableId::Param);
        let non_generic = GenericMethod::default();
        let property_accessors = self.collect_properties(&metadata, &type_ids);
        let event_accessors = self.collect_events(&metadata, &type_ids);
        let mut pending = std::mem::take(&mut self.pending);
        for (index, class) in metadata.type_defs.iter().enumerate() {
            let Some(owner) = type_ids[index] else {
                continue;
            };
            let imported = &self.types[owner];
            let first = class.method_list;
            let end = metadata
                .type_defs
                .get(index + 1)
                .map_or(metadata.method_defs.len() as u32 + 1, |next| {
                    next.method_list
                });
            let Some(methods) = first
                .checked_sub(1)
                .zip(end.checked_sub(1))
                .and_then(|(start, end)| metadata.method_defs.get(start as usize..end as usize))
            else {
                continue;
            };
            for (offset, method) in methods.iter().enumerate() {
                if method.flags & 7 != 6 {
                    continue;
                }
                let Ok(name) = metadata.strings.get(method.name) else {
                    continue;
                };
                let invocation = if method.flags & 0x0010 != 0 && !name.starts_with('.') {
                    if method.flags & 0x0400 != 0 || class.flags & 0x20 != 0 {
                        continue;
                    }
                    Invocation::Static
                } else if name == ".ctor"
                    && (imported.is_reference || imported.value_struct)
                    && class.flags & 0xa0 == 0
                {
                    if method.flags & 0x1810 != 0x1800 {
                        continue;
                    }
                    Invocation::Constructor
                } else if (imported.is_reference
                    || imported.builtin.is_some()
                    || imported.value_struct)
                    && method.flags & 0x0010 == 0
                    && !name.starts_with('.')
                {
                    Invocation::Instance
                } else {
                    continue;
                };
                let method_row = first + offset as u32;
                let generics = match generic_methods.get(&method_row) {
                    Some(Some(generics)) => generics,
                    Some(None) => continue,
                    None => &non_generic,
                };
                let type_arity = generics.parameters.len() as u16;
                if type_arity != 0 && invocation == Invocation::Constructor {
                    continue;
                }
                let Some(ParsedMethodSignature {
                    mut parameters,
                    mut result,
                    mut modes,
                }) = metadata.blobs.get(method.signature).ok().and_then(|blob| {
                    signature(
                        blob,
                        invocation,
                        type_arity,
                        imported.arity,
                        &signature_types,
                        &type_refs,
                    )
                })
                else {
                    continue;
                };
                if !modes.is_empty()
                    && !classify_byrefs(&metadata, method_row, &readonly_parameters, &mut modes)
                {
                    continue;
                }
                let property =
                    property_accessors[first as usize - 1 + offset].filter(|&(id, access, _)| {
                        let property = &self.properties[id];
                        modes.is_empty()
                            && type_arity == 0
                            && property.owner == owner
                            && property.invocation == invocation
                            && match access {
                                PropertyAccess::Get => {
                                    parameters.len() == property.index_count
                                        && !matches!(result, SignatureType::Primitive(Type::Void))
                                }
                                PropertyAccess::Set => {
                                    parameters.len() == property.index_count + 1
                                        && matches!(result, SignatureType::Primitive(Type::Void))
                                }
                            }
                    });
                if !nullable.apply(
                    &metadata,
                    index as u32 + 1,
                    method_row,
                    property.map(|(_, access, row)| (row, access)),
                    &mut parameters,
                    &mut result,
                ) {
                    continue;
                }
                let event = event_accessors
                    .get(first as usize - 1 + offset)
                    .copied()
                    .flatten()
                    .filter(|&(id, _)| {
                        let event = &self.events[id];
                        type_arity == 0
                            && modes.is_empty()
                            && parameters.len() == 1
                            && matches!(result, SignatureType::Primitive(Type::Void))
                            && event.owner == owner
                            && event.invocation == invocation
                    });
                let bound_reader = type_bounds::BoundReader {
                    metadata: &metadata,
                    types: &signature_types,
                    type_refs: &type_refs,
                    owner_arity: imported.arity,
                    method_arity: type_arity,
                };
                let mut bounds = type_bounds::PendingBounds::default();
                if !generics.parameters.iter().all(|&index| {
                    let rows = constraints
                        .get(&(index as u32 + 1))
                        .map_or(&[][..], Vec::as_slice);
                    bounds
                        .add_parameter(&metadata.generic_params[index], rows, &bound_reader)
                        .is_some()
                }) {
                    continue;
                }
                let parameter_array = !parameter_arrays.is_empty()
                    && matches!(parameters.last(), Some(SignatureType::Array { .. }))
                    && matches!(modes.last(), None | Some(ArgumentMode::Value))
                    && parameter_rows(&metadata, method_row).any(|(id, row)| {
                        usize::from(row.sequence) == parameters.len()
                            && parameter_arrays.contains(&id)
                    });
                pending.push(PendingMethod {
                    owner,
                    name: name.into(),
                    invocation,
                    hide_by_name: method.flags & 0x80 == 0,
                    type_arity,
                    bounds,
                    property: property.map(|(id, access, _)| (id, access)),
                    event,
                    argument_postconditions: nullable.argument_postconditions(
                        &metadata,
                        method_row,
                        &parameters,
                        &modes,
                        matches!(result, SignatureType::Primitive(Type::Bool)),
                    ),
                    result,
                    read_nullability: nullable.method_read(
                        &metadata,
                        method_row,
                        property.map(|(_, access, row)| (row, access)),
                    ),
                    nonnull_when: nullable.method_read_dependencies(
                        &metadata,
                        method_row,
                        property.map(|(_, access, row)| (row, access)),
                        parameters.len(),
                        &modes,
                    ),
                    defaults: parameter_defaults.trailing_defaults(
                        method_row,
                        parameters.len() - usize::from(parameter_array),
                    ),
                    parameter_nullability: nullable.method_writes(
                        &metadata,
                        method_row,
                        property.map(|(_, access, row)| (row, access)),
                        parameters.len(),
                        &modes,
                    ),
                    modes: modes.into_boxed_slice(),
                    parameters,
                    parameter_array,
                });
            }
        }
        for pending in pending {
            if let Some((signature, bounds)) = self.bind_method(&pending) {
                self.register(pending, signature, bounds);
            } else {
                self.pending.push(pending);
            }
        }
        Ok(&self.assemblies[assembly_index].name)
    }

    fn collect_properties(
        &mut self,
        metadata: &Metadata,
        type_ids: &[Option<usize>],
    ) -> Vec<Option<(usize, PropertyAccess, u32)>> {
        let mut methods = vec![None; metadata.method_defs.len()];
        if metadata.property_maps.is_empty() {
            return methods;
        }
        let default_members = attributes::default_members(metadata);
        let mut properties = vec![None; metadata.properties.len()];
        for (index, map) in metadata.property_maps.iter().enumerate() {
            let Some(owner) = map
                .parent
                .checked_sub(1)
                .and_then(|row| type_ids.get(row as usize).copied().flatten())
            else {
                continue;
            };
            let next = metadata
                .property_maps
                .get(index + 1)
                .map_or(metadata.properties.len() as u32 + 1, |map| {
                    map.property_list
                });
            let Some((start, end)) = map.property_list.checked_sub(1).zip(next.checked_sub(1))
            else {
                continue;
            };
            let Some(rows) = metadata.properties.get(start as usize..end as usize) else {
                continue;
            };
            for (offset, row) in rows.iter().enumerate() {
                let Ok(name) = metadata.strings.get(row.name) else {
                    continue;
                };
                let Ok(blob) = metadata.blobs.get(row.property_type) else {
                    continue;
                };
                let mut reader = Reader::new(blob);
                let invocation = match reader.read_u8().ok() {
                    Some(0x08) => Invocation::Static,
                    Some(0x28)
                        if self.types[owner].is_reference || self.types[owner].value_struct =>
                    {
                        Invocation::Instance
                    }
                    _ => continue,
                };
                let Ok(index_count) = reader.read_compressed_uint() else {
                    continue;
                };
                if index_count != 0
                    && (invocation != Invocation::Instance
                        || default_members.get(&map.parent) != Some(&name))
                {
                    continue;
                }
                properties[start as usize + offset] =
                    Some((owner, name, invocation, index_count as usize));
            }
        }

        let mut registered = vec![None; metadata.properties.len()];
        for row in &metadata.method_semantics {
            if row.association.table != Some(TableId::Property) {
                continue;
            }
            let access = match row.semantics {
                0x01 => PropertyAccess::Set,
                0x02 => PropertyAccess::Get,
                _ => continue,
            };
            let Some(method) = row
                .method
                .checked_sub(1)
                .map(|index| index as usize)
                .filter(|&index| {
                    metadata
                        .method_defs
                        .get(index)
                        .is_some_and(|method| method.flags & 7 == 6)
                })
            else {
                continue;
            };
            let Some(property) = row
                .association
                .row
                .checked_sub(1)
                .map(|index| index as usize)
            else {
                continue;
            };
            let Some(&(owner, name, invocation, index_count)) =
                properties.get(property).and_then(Option::as_ref)
            else {
                continue;
            };
            let id = *registered[property].get_or_insert_with(|| {
                let id = self.properties.len();
                self.properties.push(Property {
                    owner,
                    invocation,
                    index_count,
                    getter: None,
                    setter: None,
                });
                let imported = &mut self.types[owner];
                let ids = if index_count != 0 {
                    &mut imported.indexers
                } else if invocation == Invocation::Static {
                    self.static_properties
                        .entry(qualify(&imported.qualified_name, name))
                        .or_default()
                } else {
                    imported.properties.entry(name.into()).or_default()
                };
                ids.push(id);
                id
            });
            methods[method] = Some((id, access, row.association.row));
        }
        methods
    }

    fn bind_method(&mut self, method: &PendingMethod) -> Option<(Signature, Vec<TypeBound>)> {
        let bounds = method.bounds.bind(self)?;
        let signature = Signature {
            type_arity: method.type_arity,
            bounds: Vec::new(),
            result: self.bind_type(&method.result)?,
            parameters: method
                .parameters
                .iter()
                .map(|ty| self.bind_type(ty))
                .collect::<Option<_>>()?,
        };
        Some((signature, bounds))
    }

    fn bind_type(&mut self, ty: &SignatureType) -> Option<Type> {
        self.bind_signature_type(ty, true)
    }

    fn bind_signature_type(
        &mut self,
        ty: &SignatureType,
        require_linked_bounds: bool,
    ) -> Option<Type> {
        let (link, arguments, nullable, value) = match ty {
            SignatureType::Primitive(value) => return Some(*value),
            SignatureType::Value { link, arguments } => (link, arguments, false, true),
            SignatureType::Array { element, nullable } => {
                let element = self.bind_signature_type(element, require_linked_bounds)?;
                let next = self.array_elements.len();
                let id = *self.array_ids.entry(element).or_insert(next);
                if id == next {
                    self.array_elements.push(element);
                }
                return Some(if *nullable {
                    Type::NullableImportedArray(id)
                } else {
                    Type::ImportedArray(id)
                });
            }
            SignatureType::Class {
                link,
                arguments,
                nullable,
            } => (link, arguments, *nullable, false),
        };
        let definition = self.bind_class(link)?;
        let class = &self.types[definition];
        let valid_kind = if value {
            class.value_struct || class.enum_storage.is_some()
        } else {
            class.is_reference
        };
        if !valid_kind
            || (require_linked_bounds && class.bounds.is_none())
            || arguments.len() != usize::from(class.arity)
        {
            return None;
        }
        let constructed = if arguments.is_empty() {
            if class.enum_storage.is_some() {
                Type::ClrEnum(definition)
            } else if value {
                Type::ClrStruct(definition)
            } else {
                Type::External(definition)
            }
        } else {
            let arguments = arguments
                .iter()
                .map(|arg| self.bind_signature_type(arg, require_linked_bounds))
                .collect::<Option<Vec<_>>>()?;
            let instance = SignatureInstance {
                definition,
                arguments: arguments.into(),
            };
            let next = self.instances.len();
            let id = *self.instance_ids.entry(instance.clone()).or_insert(next);
            if id == next {
                self.instances.push(instance);
            }
            Type::ImportedInstance(id)
        };
        if nullable {
            constructed.nullable()
        } else {
            Some(constructed)
        }
    }

    pub(crate) fn signature_instance(&self, id: usize) -> &SignatureInstance {
        &self.instances[id]
    }

    pub(crate) fn signature_element(&self, id: usize) -> Type {
        self.array_elements[id]
    }

    fn bind_class(&self, link: &ClassLink) -> Option<usize> {
        match link {
            ClassLink::Local(ty) => Some(ty.id),
            ClassLink::Imported(reference) => self.bind_imported_class(reference),
        }
    }

    fn bind_imported_class(&self, reference: &ClassReference) -> Option<usize> {
        let mut expected = reference.assembly.as_ref();
        let mut visited = HashSet::new();
        loop {
            if let Some(id) = self
                .type_candidates(&reference.qualified_name, false)
                .find(|&id| {
                    let ty = self.imported_type(id);
                    ty.namespace == reference.namespace
                        && ty.name == reference.name
                        && self.assembly(ty.assembly).matches_reference(expected)
                })
            {
                return Some(id);
            }
            let (id, assembly) = self
                .assemblies
                .iter()
                .enumerate()
                .find(|(_, assembly)| assembly.matches_reference(expected))?;
            expected = assembly
                .forwarders
                .get(&reference.namespace)?
                .get(&reference.name)?;
            if !visited.insert(id) {
                return None;
            }
        }
    }

    fn register(&mut self, pending: PendingMethod, signature: Signature, bounds: Vec<TypeBound>) {
        let PendingMethod {
            owner,
            name,
            invocation,
            hide_by_name,
            property,
            event,
            read_nullability,
            nonnull_when,
            argument_postconditions,
            mut parameter_nullability,
            defaults,
            modes,
            parameter_array,
            ..
        } = pending;
        let id = self.methods.len();
        let imported = &mut self.types[owner];
        let owner_bounds = imported.bounds.as_deref().unwrap_or_default();
        let read_nullability =
            read_nullability.for_signature(signature.result, owner_bounds, &bounds);
        for (contract, &ty) in parameter_nullability.iter_mut().zip(&signature.parameters) {
            *contract = contract.for_signature(ty, owner_bounds, &bounds);
        }
        match invocation {
            Invocation::Static => {
                self.names
                    .entry(format!("{}.{name}", imported.qualified_name))
                    .or_default()
                    .push(id);
            }
            Invocation::Instance | Invocation::Constructor => {
                imported.members.entry(name.clone()).or_default().push(id);
            }
        }
        if let Some((property, access)) = property {
            let property = &mut self.properties[property];
            match access {
                PropertyAccess::Get => property.getter = Some(id),
                PropertyAccess::Set => property.setter = Some(id),
            }
        }
        if let Some((event, access)) = event {
            self.events[event].methods[access as usize] = Some(id);
        }
        self.methods.push(Method {
            owner,
            name,
            signature,
            read_nullability,
            nonnull_when,
            argument_postconditions,
            parameter_nullability,
            bounds,
            invocation,
            hide_by_name,
            defaults,
            modes,
            parameter_array,
        });
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.assemblies
            .iter()
            .any(|assembly| assembly.name.eq_ignore_ascii_case(name))
    }

    pub(crate) fn assemblies(&self) -> &[Assembly] {
        &self.assemblies
    }

    pub(crate) fn assembly(&self, id: usize) -> &Assembly {
        &self.assemblies[id]
    }

    pub(crate) fn method(&self, id: usize) -> &Method {
        &self.methods[id]
    }

    pub(crate) fn integer_storage(&self, ty: Type) -> Type {
        match ty {
            Type::ClrEnum(id) => self
                .imported_type(id)
                .enum_storage
                .and_then(Type::from_clr_element)
                .expect("imported enum's integer storage"),
            _ => ty,
        }
    }

    pub(crate) fn imported_type(&self, id: usize) -> &ImportedType {
        &self.types[id]
    }

    pub(crate) fn builtin_receiver(&self, ty: Type) -> Option<usize> {
        self.builtin_receivers.get(&ty).copied()
    }

    pub(crate) fn is_task(&self, id: usize) -> bool {
        let ty = self.imported_type(id);
        let assembly = self.assembly(ty.assembly);
        assembly.framework
            && builtin_assembly(&assembly.name) == Some(0)
            && ty.namespace == "System.Threading.Tasks"
            && matches!(ty.name.as_str(), "Task" | "Task`1")
    }

    pub(crate) fn is_value_base(&self, id: usize) -> bool {
        let ty = self.imported_type(id);
        let assembly = self.assembly(ty.assembly);
        assembly.framework
            && builtin_assembly(&assembly.name) == Some(0)
            && ty.namespace == "System"
            && matches!(ty.name.as_str(), "Enum" | "ValueType")
    }

    pub(crate) fn type_candidates(
        &self,
        name: &str,
        allow_static_class: bool,
    ) -> impl Iterator<Item = usize> + '_ {
        self.type_names
            .get(name)
            .into_iter()
            .flatten()
            .copied()
            .filter(move |&id| {
                let ty = &self.types[id];
                allow_static_class
                    || ty.is_reference
                    || ty.enum_storage.is_some()
                    || ty.value_struct
            })
    }

    pub(crate) fn members(&self, owner: usize, name: &str) -> &[usize] {
        self.types[owner]
            .members
            .get(name)
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn parents(
        &self,
        owner: usize,
        include_interfaces: bool,
    ) -> impl Iterator<Item = Type> + '_ {
        let ty = &self.types[owner];
        ty.base.into_iter().chain(
            ty.interfaces
                .iter()
                .copied()
                .flatten()
                .filter(move |_| include_interfaces),
        )
    }

    pub(crate) fn static_properties(&self, qualified_name: &str) -> &[usize] {
        self.static_properties
            .get(qualified_name)
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn property_owner(&self, property: usize) -> usize {
        self.properties[property].owner
    }

    pub(crate) fn declared_properties(&self, owner: usize, name: &str) -> &[usize] {
        self.types[owner]
            .properties
            .get(name)
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn declared_indexers(&self, owner: usize) -> &[usize] {
        &self.types[owner].indexers
    }

    pub(crate) fn indexer_accessor(
        &self,
        property: usize,
        preferred: PropertyAccess,
    ) -> Option<(usize, PropertyAccess)> {
        let other = match preferred {
            PropertyAccess::Get => PropertyAccess::Set,
            PropertyAccess::Set => PropertyAccess::Get,
        };
        self.accessor(property, preferred)
            .map(|method| (method, preferred))
            .or_else(|| self.accessor(property, other).map(|method| (method, other)))
    }

    pub(crate) fn accessor(&self, property: usize, access: PropertyAccess) -> Option<usize> {
        let property = &self.properties[property];
        match access {
            PropertyAccess::Get => property.getter,
            PropertyAccess::Set => property.setter,
        }
    }

    pub(crate) fn candidates(&self, name: &str) -> &[usize] {
        self.names.get(name).map_or(&[], Vec::as_slice)
    }
}

/// Checks the syntax of an assembly name and excludes the compiler's framework names.
pub fn valid_assembly_name(name: &str) -> bool {
    name.trim() == name
        && !matches!(name, "" | "." | "..")
        && !name
            .chars()
            .any(|ch| ch.is_control() || "/\\:,*?\"<>|=".contains(ch))
        && builtin_assembly(name).is_none()
        && !name.eq_ignore_ascii_case("System.Private.CoreLib")
}

pub(crate) fn builtin_assembly(name: &str) -> Option<usize> {
    BUILTIN_ASSEMBLIES
        .iter()
        .position(|candidate| name.eq_ignore_ascii_case(candidate))
}

fn valid_reference_name(name: &str) -> bool {
    valid_assembly_name(name) || builtin_assembly(name).is_some()
}

fn system_parent(metadata: &Metadata, parent: CodedIndex, core_facade: bool) -> Option<&str> {
    let index = parent.row.checked_sub(1)? as usize;
    let (namespace, name) = match parent.table? {
        TableId::TypeRef => {
            let ty = metadata.type_refs.get(index)?;
            (ty.type_namespace, ty.type_name)
        }
        TableId::TypeDef if core_facade => {
            let ty = metadata.type_defs.get(index)?;
            (ty.type_namespace, ty.type_name)
        }
        _ => return None,
    };
    if metadata.strings.get(namespace).ok()? != "System" {
        return None;
    }
    metadata.strings.get(name).ok()
}

fn parameter_rows(
    metadata: &Metadata,
    method: u32,
) -> impl DoubleEndedIterator<Item = (u32, &ParamRow)> {
    let indirect = !metadata.param_ptrs.is_empty();
    let count = if indirect {
        metadata.param_ptrs.len()
    } else {
        metadata.params.len()
    };
    let first = metadata.method_defs[method as usize - 1].param_list;
    let end = metadata
        .method_defs
        .get(method as usize)
        .map_or(count, |next| next.param_list.saturating_sub(1) as usize);
    let range = first
        .checked_sub(1)
        .filter(|_| end <= count)
        .map_or(0..0, |first| first as usize..end);
    range.filter_map(move |index| {
        let id = if indirect {
            metadata.param_ptrs[index].param
        } else {
            index as u32 + 1
        };
        metadata
            .params
            .get(id.checked_sub(1)? as usize)
            .map(|row| (id, row))
    })
}

fn read_metadata(bytes: &[u8]) -> Result<Metadata, String> {
    let data = match FileKind::parse(bytes).map_err(|error| error.to_string())? {
        FileKind::Pe32 => {
            metadata_data(PeFile32::parse(bytes).map_err(|error| error.to_string())?)?
        }
        FileKind::Pe64 => {
            metadata_data(PeFile64::parse(bytes).map_err(|error| error.to_string())?)?
        }
        _ => return Err("expected a managed PE image".into()),
    };
    // Validate the stream ranges before clrmeta indexes into the image.
    let root = MetadataRoot::parse(data).map_err(|error| error.to_string())?;
    for stream in &root.streams {
        if !stream
            .offset
            .checked_add(stream.size)
            .is_some_and(|end| end as usize <= data.len())
        {
            return Err(format!("invalid CLR metadata stream '{}'", stream.name));
        }
    }
    let tables = root.tables_stream().ok_or("missing CLR tables stream")?;
    if tables.name != "#~" {
        return Err("expected compressed CLR metadata tables".into());
    }
    let tables = &data[tables.offset as usize..(tables.offset + tables.size) as usize];
    let mut reader = Reader::new(tables);
    let header = TablesHeader::parse(&mut reader, false).map_err(|error| error.to_string())?;
    // Each row takes at least two bytes. Bound allocations before parsing rows.
    let rows: u64 = header
        .row_counts
        .iter()
        .map(|&count| u64::from(count))
        .sum();
    if rows > (tables.len() / 2) as u64 {
        return Err("invalid CLR metadata row counts".into());
    }
    Metadata::parse(data).map_err(|error| error.to_string())
}

fn metadata_data<'a, Pe: ImageNtHeaders>(pe: PeFile<'a, Pe>) -> Result<&'a [u8], String> {
    let directory = pe
        .data_directory(IMAGE_DIRECTORY_ENTRY_COM_DESCRIPTOR)
        .ok_or("missing CLR header")?;
    let cli = directory
        .data(pe.data(), &pe.section_table())
        .map_err(|error| error.to_string())?;
    let mut reader = Reader::new(cli);
    reader.read_bytes(8).map_err(|error| error.to_string())?;
    let rva = reader.read_u32().map_err(|error| error.to_string())?;
    let size = reader.read_u32().map_err(|error| error.to_string())?;
    pe.section_table()
        .pe_data_at(pe.data(), rva)
        .and_then(|section| section.get(..size as usize))
        .ok_or_else(|| "invalid CLR metadata directory".into())
}

fn type_forwarders(
    metadata: &Metadata,
    assembly_refs: &[Arc<AssemblyRefInfo>],
) -> HashMap<String, HashMap<String, Arc<AssemblyRefInfo>>> {
    const FORWARDER: u32 = 0x0020_0000;
    let mut forwarders = HashMap::<String, HashMap<String, Arc<AssemblyRefInfo>>>::new();
    let mut duplicates = HashSet::new();
    for row in &metadata.exported_types {
        if row.flags & FORWARDER == 0 || row.implementation.table != Some(TableId::AssemblyRef) {
            continue;
        }
        let Some(target) = row
            .implementation
            .row
            .checked_sub(1)
            .and_then(|index| assembly_refs.get(index as usize))
            .filter(|target| valid_reference_name(&target.name) && target.flags & 1 == 0)
        else {
            continue;
        };
        let (Ok(namespace), Ok(name)) = (
            metadata.strings.get(row.type_namespace),
            metadata.strings.get(row.type_name),
        ) else {
            continue;
        };
        let namespace_entries = forwarders.entry(namespace.into()).or_default();
        if duplicates.contains(&(namespace, name)) {
            continue;
        }
        if namespace_entries
            .insert(name.into(), Arc::clone(target))
            .is_some()
        {
            namespace_entries.remove(name);
            duplicates.insert((namespace, name));
        }
    }
    forwarders
}

// Allows ref-like arguments in addition to ordinary ones. Ref-like type
// definitions are still excluded from oxel's supported value types.
const ALLOW_BY_REF_LIKE: u16 = 0x20;

fn generic_types(metadata: &Metadata) -> HashMap<u32, GenericType> {
    let mut types: HashMap<u32, GenericType> = HashMap::new();
    for parameter in &metadata.generic_params {
        if parameter.owner.table == Some(TableId::TypeDef) {
            let generic = types.entry(parameter.owner.row).or_default();
            generic.invalid |= usize::from(parameter.number) != generic.parameters.len()
                || generic.parameters.len() == usize::from(u16::MAX)
                || parameter.flags & !(0x1f | ALLOW_BY_REF_LIKE) != 0
                || parameter.flags & 3 == 3
                || parameter.flags & 0x0c == 0x0c;
            let variance = match parameter.flags & 3 {
                1 => Variance::Covariant,
                2 => Variance::Contravariant,
                _ => Variance::Invariant,
            };
            generic.parameters.push(variance);
        }
    }
    types
}

fn generic_constraints(metadata: &Metadata) -> HashMap<u32, Vec<CodedIndex>> {
    let mut constraints: HashMap<u32, Vec<CodedIndex>> = HashMap::new();
    for constraint in &metadata.generic_param_constraints {
        constraints
            .entry(constraint.owner)
            .or_default()
            .push(constraint.constraint);
    }
    constraints
}

fn generic_methods(metadata: &Metadata) -> HashMap<u32, Option<GenericMethod>> {
    let mut methods = HashMap::new();
    for (index, parameter) in metadata.generic_params.iter().enumerate() {
        if parameter.owner.table != Some(TableId::MethodDef) {
            continue;
        }
        let method = methods
            .entry(parameter.owner.row)
            .or_insert_with(|| Some(GenericMethod::default()));
        let Some(generics) = method.as_mut() else {
            continue;
        };
        if usize::from(parameter.number) != generics.parameters.len()
            || parameter.flags & !(0x1c | ALLOW_BY_REF_LIKE) != 0
            || parameter.flags & 0x0c == 0x0c
            || generics.parameters.len() == usize::from(u16::MAX)
        {
            *method = None;
            continue;
        }
        generics.parameters.push(index);
    }
    methods
}

fn classify_byrefs(
    metadata: &Metadata,
    method: u32,
    readonly: &HashSet<u32>,
    modes: &mut [ArgumentMode],
) -> bool {
    const IN: u16 = 1;
    const OUT: u16 = 2;
    for (id, row) in parameter_rows(metadata, method) {
        if let Some(index) = row.sequence.checked_sub(1)
            && let Some(mode @ ArgumentMode::Ref) = modes.get_mut(usize::from(index))
        {
            match row.flags & (IN | OUT) {
                0 if !readonly.contains(&id) => {}
                OUT => *mode = ArgumentMode::Out,
                _ => return false,
            }
        }
    }
    true
}

fn signature(
    blob: &[u8],
    invocation: Invocation,
    type_arity: u16,
    owner_arity: u16,
    types: &[Option<LocalType>],
    type_refs: &[Option<Arc<ClassReference>>],
) -> Option<ParsedMethodSignature> {
    let mut reader = Reader::new(blob);
    let has_this = if invocation == Invocation::Static {
        0
    } else {
        0x20
    };
    let generic = if type_arity == 0 { 0 } else { 0x10 };
    if reader.read_u8().ok()? != (has_this | generic) {
        return None;
    }
    if type_arity != 0 && reader.read_compressed_uint().ok()? != u32::from(type_arity) {
        return None;
    }
    let count = reader.read_compressed_uint().ok()? as usize;
    let result = signature_type(&mut reader, type_arity, owner_arity, types, type_refs, 0)?;
    if (invocation == Invocation::Constructor
        && !matches!(result, SignatureType::Primitive(Type::Void)))
        || count > blob.len() - reader.position()
    {
        return None;
    }
    let mut parameters = Vec::with_capacity(count);
    let mut modes = Vec::new();
    for index in 0..count {
        if reader.peek_u8().ok() == Some(0x10) {
            reader.read_u8().ok()?; // ELEMENT_TYPE_BYREF
            if modes.is_empty() {
                modes.resize(count, ArgumentMode::Value);
            }
            modes[index] = ArgumentMode::Ref;
        }
        let ty = signature_type(&mut reader, type_arity, owner_arity, types, type_refs, 0)?;
        if matches!(ty, SignatureType::Primitive(Type::Void)) {
            return None;
        }
        parameters.push(ty);
    }
    (reader.position() == blob.len()).then_some(ParsedMethodSignature {
        parameters,
        result,
        modes,
    })
}

fn signature_type(
    reader: &mut Reader<'_>,
    type_arity: u16,
    owner_arity: u16,
    types: &[Option<LocalType>],
    type_refs: &[Option<Arc<ClassReference>>],
    depth: usize,
) -> Option<SignatureType> {
    if depth > crate::syntax::MAX_DEPTH {
        return None;
    }
    let element = reader.read_u8().ok()?;
    if let Some(ty) = Type::from_clr_element(element) {
        return Some(SignatureType::Primitive(ty));
    }
    let ty = match element {
        0x13 => {
            let index = u16::try_from(reader.read_compressed_uint().ok()?).ok()?;
            if index >= owner_arity {
                return None;
            }
            Type::TypeParameter(index)
        }
        0x1e => {
            let index = u16::try_from(reader.read_compressed_uint().ok()?).ok()?;
            if index >= type_arity {
                return None;
            }
            Type::ImportedMethodParameter(index)
        }
        0x1d => {
            let element =
                signature_type(reader, type_arity, owner_arity, types, type_refs, depth + 1)?;
            if matches!(element, SignatureType::Primitive(Type::Void)) {
                return None;
            }
            return Some(SignatureType::Array {
                element: Box::new(element),
                nullable: false,
            });
        }
        0x11 | 0x12 | 0x15 => {
            let generic = element == 0x15;
            let kind = if generic {
                reader.read_u8().ok()?
            } else {
                element
            };
            let index = CodedIndex::decode(
                CodedIndexKind::TypeDefOrRef,
                reader.read_compressed_uint().ok()?,
            );
            let link = class_link(index, types, type_refs)?;
            let arguments = if generic {
                let count = reader.read_compressed_uint().ok()? as usize;
                if count == 0 || count > reader.remaining() || count > usize::from(u16::MAX) {
                    return None;
                }
                (0..count)
                    .map(|_| {
                        signature_type(reader, type_arity, owner_arity, types, type_refs, depth + 1)
                            .filter(|ty| !matches!(ty, SignatureType::Primitive(Type::Void)))
                    })
                    .collect::<Option<Vec<_>>>()?
            } else {
                Vec::new()
            };
            return Some(match kind {
                0x11 => SignatureType::Value { link, arguments },
                0x12 => SignatureType::Class {
                    link,
                    arguments,
                    nullable: false,
                },
                _ => return None,
            });
        }
        _ => return None,
    };
    Some(SignatureType::Primitive(ty))
}

fn parent_signature(
    metadata: &Metadata,
    index: CodedIndex,
    owner_arity: u16,
    types: &[Option<LocalType>],
    type_refs: &[Option<Arc<ClassReference>>],
) -> Option<SignatureType> {
    if index.table == Some(TableId::TypeSpec) {
        let row = metadata.get_type_spec(index.row)?;
        let mut reader = Reader::new(metadata.blobs.get(row.signature).ok()?);
        let result = signature_type(&mut reader, 0, owner_arity, types, type_refs, 0)?;
        return (reader.remaining() == 0 && matches!(result, SignatureType::Class { .. }))
            .then_some(result);
    }
    Some(SignatureType::Class {
        link: class_link(index, types, type_refs)?,
        arguments: Vec::new(),
        nullable: false,
    })
}

// Framework type markers can be named through the public facades or core library.
fn framework_type_ref(metadata: &Metadata, index: CodedIndex, namespace: &str, name: &str) -> bool {
    let assembly = (|| {
        if index.table != Some(TableId::TypeRef) {
            return None;
        }
        let ty = metadata.type_refs.get(index.row.checked_sub(1)? as usize)?;
        if metadata.strings.get(ty.type_name).ok()? != name
            || metadata.strings.get(ty.type_namespace).ok()? != namespace
            || ty.resolution_scope.table != Some(TableId::AssemblyRef)
        {
            return None;
        }
        metadata
            .assembly_refs
            .get(ty.resolution_scope.row.checked_sub(1)? as usize)
    })();
    let Some(assembly) = assembly else {
        return false;
    };
    let (Ok(name), Ok(token)) = (
        metadata.strings.get(assembly.name),
        metadata.blobs.get(assembly.public_key_or_token),
    ) else {
        return false;
    };
    [
        ("System.Runtime", 0xb03f5f7f11d50a3au64),
        ("System.Private.CoreLib", 0x7cec85d7bea7798e),
        ("mscorlib", 0xb77a5c561934e089),
        ("netstandard", 0xcc7b13ffcd2ddd51),
    ]
    .into_iter()
    .any(|(scope, key)| {
        assembly.flags & 1 == 0 && name.eq_ignore_ascii_case(scope) && token == key.to_be_bytes()
    })
}

fn class_link(
    index: CodedIndex,
    types: &[Option<LocalType>],
    type_refs: &[Option<Arc<ClassReference>>],
) -> Option<ClassLink> {
    let row = index.row.checked_sub(1)? as usize;
    match index.table? {
        TableId::TypeDef => Some(ClassLink::Local(types.get(row).copied().flatten()?)),
        TableId::TypeRef => Some(ClassLink::Imported(Arc::clone(
            type_refs.get(row)?.as_ref()?,
        ))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emitted_native_strings_have_explicit_utf8_marshalling_metadata() {
        let source = r#"
            extern "C" fn measure(utf8 text: string, out count: i32, utf8 suffix: string?) = "test";
            fn main() {}
        "#;
        let assembly = crate::compile(source, "native_text").unwrap();
        let metadata = read_metadata(&assembly).unwrap();
        let sequences: Vec<_> = metadata
            .field_marshals
            .iter()
            .map(|row| {
                assert_eq!(row.parent.table, Some(TableId::Param));
                assert_eq!(metadata.blobs.get(row.native_type).unwrap(), &[0x30]);
                let param = &metadata.params[row.parent.row as usize - 1];
                assert_eq!(param.flags, 0x2000);
                param.sequence
            })
            .collect();
        assert_eq!(sequences, [1, 3]);
    }
}

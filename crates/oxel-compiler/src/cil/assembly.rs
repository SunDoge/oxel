//! Packages CIL and CLR metadata in a portable, framework-dependent DLL.

use std::collections::HashMap;

use clrmeta::{
    heaps::{BlobHeap, GuidHeap, StringsHeap},
    root::MetadataRoot,
    stream::StreamHeader,
    tables::{
        AssemblyRefRow, AssemblyRow, CodedIndex, CodedIndexKind, CustomAttributeRow,
        FieldMarshalRow, FieldRow, GenericParamConstraintRow, GenericParamRow, ImplMapRow,
        MemberRefRow, MethodDefRow, MethodSpecRow, ModuleRefRow, ModuleRow, ParamRow,
        StandAloneSigRow, TableId, TablesHeader, TypeDefRow, TypeRefRow, TypeSpecRow,
    },
    writer::Writer,
};
use object::{
    pe,
    write::pe::{NtHeaders, Writer as PeWriter},
};

use super::{
    Body, FIELD_DEF, MEMBER_REF, METHOD_DEF, METHOD_SPEC, STAND_ALONE_SIG, STRING_OPERATIONS,
    StackSlot, TYPE_DEF, TYPE_SPEC, WRITE_LINE_OVERLOADS,
};
use crate::{
    ArgumentMode, AssemblyKind, Diagnostic, References, Span, TestOptions,
    clr::{Assembly, BUILTIN_ASSEMBLIES, Invocation, TypeBoundKind, builtin_assembly},
    hir::{GenericBound, InstanceKind, MethodBody, Program, Signature, Struct, Type, TypeInstance},
    syntax::{NativeImport, ParameterPassing},
};

const CLI_HEADER_SIZE: u32 = 72;
const VALUE_TYPE_REF: u32 = 4;
const INVALID_OPERATION_REF: u32 = 5;
const STRING_REF: u32 = 3;
const FIRST_STRUCT_ROW: u32 = 3;

fn struct_row(id: usize) -> u32 {
    FIRST_STRUCT_ROW + id as u32
}

fn assembly_reference(
    assembly: &Assembly,
    strings: &mut StringsHeap,
    blobs: &mut BlobHeap,
) -> AssemblyRefRow {
    let (major_version, minor_version, build_number, revision_number) = assembly.version;
    AssemblyRefRow {
        major_version,
        minor_version,
        build_number,
        revision_number,
        name: strings.add(&assembly.name),
        culture: assembly
            .culture
            .as_deref()
            .map_or(0, |culture| strings.add(culture)),
        public_key_or_token: assembly.token.as_ref().map_or(0, |token| blobs.add(token)),
        ..Default::default()
    }
}
const SECTION_ALIGNMENT: u32 = 0x2000;
const FILE_ALIGNMENT: u32 = 0x200;
// One header page, followed by .text. Checked against the PE writer's layout.
const TEXT_RVA: u32 = SECTION_ALIGNMENT;

pub(super) fn write(
    program: &Program<'_>,
    sources: &[&str],
    name: &str,
    references: &References,
    tests: Option<TestOptions<'_>>,
) -> Result<Vec<u8>, Diagnostic> {
    let field_count: usize = program
        .structs
        .iter()
        .map(|definition| definition.fields.len())
        .sum();
    // Keep source method IDs stable. Each synthetic method gets a contiguous
    // range for its immediate children, regardless of nesting depth.
    let mut methods: Vec<_> = program.functions.iter().collect();
    let mut lambda_bases = Vec::new();
    while let Some(function) = methods.get(lambda_bases.len()).copied() {
        lambda_bases.push(methods.len());
        methods.extend(&function.lambdas);
    }
    let method_count = methods.len();
    if method_count >= 0x00ff_ffff
        || program.structs.len() > 0x00ff_ffff - (FIRST_STRUCT_ROW as usize - 1)
        || field_count >= 0x00ff_ffff
    {
        return Err(image_too_large());
    }
    // A narrow metadata list cannot represent the one-past-end index 65536.
    let narrow_limit = usize::from(u16::MAX);
    if (!program.structs.is_empty() && method_count == narrow_limit)
        || (field_count == narrow_limit
            && program
                .structs
                .last()
                .is_some_and(|definition| definition.fields.is_empty()))
    {
        return Err(Diagnostic::new(
            "E0402",
            Span::default(),
            "CLR metadata list index exceeds its encoding width",
        ));
    }
    let mut text = Writer::new();
    let namespace = (program.kind == AssemblyKind::Library).then_some(name);
    let mut metadata = Tables::new(
        name,
        namespace,
        sources,
        references,
        &program.instances,
        tests,
    );
    metadata.add_structs(&program.structs, namespace, method_count as u32 + 1);
    for (method_index, (function, lambda_base)) in methods.into_iter().zip(lambda_bases).enumerate()
    {
        metadata.add_type_parameters(
            index(TableId::MethodDef, method_index as u32 + 1),
            &function.type_parameters,
            &function.signature.bounds,
        );
        let (rva, native_flags, impl_flags, parameters) = match &function.body {
            MethodBody::Managed(block) => {
                text.align(4);
                let offset = image_offset(text.len())?;
                let mut body = Body::new(
                    &mut metadata,
                    lambda_base,
                    &function.locals,
                    &function.captured_cells,
                    function.has_exception_regions,
                );
                body.emit(block);
                body.write(&mut text, function.span)?;
                (TEXT_RVA + CLI_HEADER_SIZE + offset, 0, 0, &[][..])
            }
            MethodBody::Native { import, parameters } => {
                metadata.add_native_import(method_index as u32 + 1, import);
                (0, 0x2000, 0x0080, parameters.as_slice()) // PInvokeImpl, PreserveSig
            }
        };
        let signature = metadata.signature_with_modes(
            &function.signature.parameters,
            function.signature.result,
            Invocation::Static,
            function.signature.type_arity,
            parameters.iter().map(|parameter| (parameter.mode(), false)),
        );
        let param_list = metadata.add_parameters(&function.signature, parameters);
        let (flags, name) = if method_index < program.functions.len() {
            let access = if function.internal { 0x0003 } else { 0x0006 }; // assembly or public
            (0x0090 | access, metadata.strings.add(function.name)) // static hidebysig
        } else {
            (
                0x0091,
                metadata.strings.add(&format!("<lambda_{method_index}>")),
            ) // private static hidebysig
        };
        metadata.methods.push(MethodDefRow {
            rva,
            impl_flags,
            flags: flags | native_flags,
            name,
            signature,
            param_list,
        });
    }

    text.align(4);
    let metadata_offset = CLI_HEADER_SIZE + image_offset(text.len())?;
    let metadata = metadata.finish()?;
    let metadata_length = image_offset(metadata.len())?;
    let entry = program
        .entry
        .map_or(0, |method| METHOD_DEF | (method as u32 + 1));
    let mut section = Writer::new();
    section.write_u32(CLI_HEADER_SIZE);
    section.write_u16(2); // minimum CLI major version
    section.write_u16(5); // minimum CLI minor version
    section.write_u32(TEXT_RVA + metadata_offset);
    section.write_u32(metadata_length);
    section.write_u32(1); // IL-only, architecture neutral
    section.write_u32(entry);
    section.write_bytes(&[0; 48]); // remaining CLI directories
    section.write_bytes(text.as_slice());
    section.write_bytes(&metadata);
    pe_image(section.as_slice(), namespace.is_some())
}

fn image_too_large() -> Diagnostic {
    Diagnostic::new(
        "E0402",
        Span::default(),
        "assembly exceeds the supported PE image size",
    )
}

fn image_offset(size: usize) -> Result<u32, Diagnostic> {
    if size >= 0x4000_0000 {
        return Err(image_too_large());
    }
    Ok(size as u32)
}

fn pe_image(data: &[u8], library: bool) -> Result<Vec<u8>, Diagnostic> {
    let size = image_offset(data.len())?;
    let mut bytes = Vec::new();
    // PE32 with the CLI IL-only flag is the AnyCPU format.
    let mut pe = PeWriter::new(false, SECTION_ALIGNMENT, FILE_ALIGNMENT, &mut bytes);
    pe.reserve_dos_header_and_stub();
    pe.reserve_nt_headers(16);
    pe.reserve_section_headers(1);
    let section = pe.reserve_text_section(size);
    if section.virtual_address != TEXT_RVA {
        return Err(Diagnostic::new(
            "E0403",
            Span::default(),
            "unexpected PE section layout",
        ));
    }
    pe.set_data_directory(
        pe::IMAGE_DIRECTORY_ENTRY_COM_DESCRIPTOR,
        TEXT_RVA,
        CLI_HEADER_SIZE,
    );
    pe.write_dos_header_and_stub().map_err(|err| {
        Diagnostic::new(
            "E0403",
            Span::default(),
            format!("could not write PE header: {err}"),
        )
    })?;
    pe.write_nt_headers(NtHeaders {
        machine: pe::IMAGE_FILE_MACHINE_I386,
        time_date_stamp: 0,
        characteristics: pe::IMAGE_FILE_EXECUTABLE_IMAGE
            | pe::IMAGE_FILE_LARGE_ADDRESS_AWARE
            | if library { pe::IMAGE_FILE_DLL } else { 0 },
        major_linker_version: 0,
        minor_linker_version: 0,
        address_of_entry_point: 0, // dotnet invokes the managed entry point in the CLI header
        image_base: 0x0040_0000,
        major_operating_system_version: 4,
        minor_operating_system_version: 0,
        major_image_version: 0,
        minor_image_version: 0,
        major_subsystem_version: 4,
        minor_subsystem_version: 0,
        subsystem: pe::IMAGE_SUBSYSTEM_WINDOWS_CUI,
        dll_characteristics: pe::IMAGE_DLLCHARACTERISTICS_NX_COMPAT,
        size_of_stack_reserve: 0x10_0000,
        size_of_stack_commit: 0x1000,
        size_of_heap_reserve: 0x10_0000,
        size_of_heap_commit: 0x1000,
    });
    pe.write_section_headers();
    pe.write_section(section.file_offset, data);
    Ok(bytes)
}

struct GenericParameter {
    row: GenericParamRow,
    bounds: Vec<CodedIndex>,
}

pub(super) struct Tables<'refs> {
    strings: StringsHeap,
    blobs: BlobHeap,
    user_strings: Writer,
    user_string_offsets: HashMap<Box<str>, u32>,
    user_strings_overflow: bool,
    guids: GuidHeap,
    module: ModuleRow,
    assembly: AssemblyRow,
    references: Vec<AssemblyRefRow>,
    type_refs: Vec<TypeRefRow>,
    primitive_type_indices: HashMap<Type, u32>,
    type_specs: Vec<TypeSpecRow>,
    type_spec_indices: HashMap<Type, u32>,
    cell_array_indices: HashMap<Type, u32>,
    field_ref_indices: HashMap<(usize, usize), u32>,
    instances: &'refs [TypeInstance],
    types: Vec<TypeDefRow>,
    fields: Vec<FieldRow>,
    parameters: Vec<ParamRow>,
    field_marshals: Vec<FieldMarshalRow>,
    custom_attributes: Vec<CustomAttributeRow>,
    nullable_type: Option<u32>,
    volatile_type: Option<u32>,
    in_attribute_type: Option<u32>,
    create_instance: Option<u32>,
    string_from_object: Option<u32>,
    error_writer_members: Option<(u32, u32)>,
    string_array_concat: Option<u32>,
    nullable_ctors: [Option<u32>; 2],
    first_fields: Vec<u32>,
    members: Vec<MemberRefRow>,
    externals: &'refs References,
    assembly_indices: HashMap<usize, u32>,
    type_indices: HashMap<usize, u32>,
    member_indices: HashMap<(usize, Option<Type>), u32>,
    delegate_ctors: HashMap<Type, u32>,
    external_field_indices: HashMap<(usize, Option<Type>), u32>,
    slice_indexers: HashMap<Type, u32>,
    methods: Vec<MethodDefRow>,
    module_refs: Vec<ModuleRefRow>,
    module_ref_indices: HashMap<u32, u32>,
    native_imports: Vec<ImplMapRow>,
    generic_params: Vec<GenericParameter>,
    method_specs: Vec<MethodSpecRow>,
    method_spec_indices: HashMap<u32, HashMap<Box<[Type]>, u32>>,
    locals: Vec<StandAloneSigRow>,
}

impl<'refs> Tables<'refs> {
    fn new(
        name: &str,
        namespace: Option<&str>,
        sources: &[&str],
        externals: &'refs References,
        instances: &'refs [TypeInstance],
        tests: Option<TestOptions<'_>>,
    ) -> Self {
        let mut strings = StringsHeap::new();
        let mut blobs = BlobHeap::new();
        let mut guids = GuidHeap::new();
        let mut hash = blake3::Hasher::new_derive_key("oxel assembly format 49 module id");
        hash.update(name.as_bytes());
        hash.update(b"\0");
        if namespace.is_some() {
            hash.update(b"library\0");
        }
        if let Some(TestOptions {
            filter,
            include_ignored,
        }) = tests
        {
            hash.update(b"tests\0");
            if !filter.is_empty() {
                hash.update(b"filter\0");
                hash.update(&(filter.len() as u64).to_le_bytes());
                hash.update(filter.as_bytes());
            }
            if include_ignored {
                hash.update(b"include-ignored\0");
            }
        }
        hash.update(&(sources.len() as u64).to_le_bytes());
        for source in sources {
            hash.update(&(source.len() as u64).to_le_bytes());
            hash.update(source.as_bytes());
        }
        let mut dependency_hashes: Vec<_> = externals
            .assemblies()
            .iter()
            .map(|assembly| assembly.hash)
            .collect();
        dependency_hashes.sort_unstable_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        for dependency in dependency_hashes {
            hash.update(dependency.as_bytes());
        }
        let mut mvid = [0; 16];
        hash.finalize_xof().fill(&mut mvid);
        let module = ModuleRow {
            name: strings.add(&format!("{name}.dll")),
            mvid: guids.add(&mvid),
            ..Default::default()
        };
        let assembly = AssemblyRow {
            name: strings.add(name),
            major_version: 1,
            ..Default::default()
        };
        let token = blobs.add(&[0xb0, 0x3f, 0x5f, 0x7f, 0x11, 0xd5, 0x0a, 0x3a]);
        let mut references = BUILTIN_ASSEMBLIES.map(|name| AssemblyRefRow {
            major_version: 4,
            name: strings.add(name),
            public_key_or_token: token,
            ..Default::default()
        });
        let mut assembly_indices = HashMap::new();
        for (id, assembly) in externals.assemblies().iter().enumerate() {
            if let Some(index) = builtin_assembly(&assembly.name) {
                references[index] = assembly_reference(assembly, &mut strings, &mut blobs);
                assembly_indices.insert(id, index as u32 + 1);
            }
        }
        let system = strings.add("System");
        let type_refs = [
            (1, "Object"),
            (2, "Console"),
            (1, "String"),
            (1, "ValueType"),
            (1, "InvalidOperationException"),
            (1, "Exception"),
        ]
        .map(|(row, name)| TypeRefRow {
            resolution_scope: index(TableId::AssemblyRef, row),
            type_name: strings.add(name),
            type_namespace: system,
        });
        let types = [
            TypeDefRow {
                type_name: strings.add("<Module>"),
                field_list: 1,
                method_list: 1,
                ..Default::default()
            },
            TypeDefRow {
                flags: 0x0181, // public, abstract, sealed
                type_name: strings.add("Program"),
                type_namespace: namespace.map_or(0, |name| strings.add(name)),
                extends: index(TableId::TypeRef, 1),
                field_list: 1,
                method_list: 1,
            },
        ];
        let mut user_strings = Writer::new();
        user_strings.write_u8(0); // The empty first heap entry.
        let mut tables = Self {
            strings,
            blobs,
            user_strings,
            user_string_offsets: HashMap::new(),
            user_strings_overflow: false,
            guids,
            module,
            assembly,
            references: references.into(),
            type_refs: type_refs.into(),
            primitive_type_indices: HashMap::new(),
            type_specs: Vec::new(),
            type_spec_indices: HashMap::new(),
            cell_array_indices: HashMap::new(),
            field_ref_indices: HashMap::new(),
            instances,
            types: types.into(),
            fields: Vec::new(),
            parameters: Vec::new(),
            field_marshals: Vec::new(),
            custom_attributes: Vec::new(),
            nullable_type: None,
            volatile_type: None,
            in_attribute_type: None,
            create_instance: None,
            string_from_object: None,
            error_writer_members: None,
            string_array_concat: None,
            nullable_ctors: [None; 2],
            first_fields: Vec::new(),
            members: Vec::new(),
            externals,
            assembly_indices,
            type_indices: HashMap::new(),
            member_indices: HashMap::new(),
            delegate_ctors: HashMap::new(),
            external_field_indices: HashMap::new(),
            slice_indexers: HashMap::new(),
            methods: Vec::new(),
            module_refs: Vec::new(),
            module_ref_indices: HashMap::new(),
            native_imports: Vec::new(),
            generic_params: Vec::new(),
            method_specs: Vec::new(),
            method_spec_indices: HashMap::new(),
            locals: Vec::new(),
        };
        for ty in WRITE_LINE_OVERLOADS {
            let signature =
                tables.method_signature(ty.as_slice(), Type::Void, Invocation::Static, 0);
            tables.push_member(2, "WriteLine", signature);
        }
        for (name, result) in STRING_OPERATIONS {
            let signature =
                tables.method_signature(&[Type::String; 2], result, Invocation::Static, 0);
            tables.push_member(3, name, signature);
        }
        let signature = tables.method_signature(&[], Type::Void, Invocation::Constructor, 0);
        tables.push_member(INVALID_OPERATION_REF, ".ctor", signature);
        let signature = tables.method_signature(&[], Type::I32, Invocation::Instance, 0);
        tables.push_member(STRING_REF, "get_Length", signature);
        let signature = tables.method_signature(&[Type::I32], Type::Char, Invocation::Instance, 0);
        tables.push_member(STRING_REF, "get_Chars", signature);
        let signature =
            tables.method_signature(&[Type::String], Type::Void, Invocation::Constructor, 0);
        tables.push_member(INVALID_OPERATION_REF, ".ctor", signature);
        tables
    }

    fn add_structs(&mut self, structs: &[Struct<'_>], namespace: Option<&str>, method_end: u32) {
        let source_namespace = namespace.map_or(0, |name| self.strings.add(name));
        for (id, definition) in structs.iter().enumerate() {
            let first_field = self.fields.len() as u32 + 1;
            self.first_fields.push(first_field);
            let arity = definition.type_parameters.len();
            let name = definition
                .name
                .strip_prefix("compiler.")
                .filter(|_| definition.builtin);
            let type_name = if arity == 0 {
                self.strings.add(name.unwrap_or(definition.name))
            } else {
                self.strings.add(&format!("{}`{arity}", definition.name))
            };
            self.add_type_parameters(
                index(TableId::TypeDef, struct_row(id)),
                &definition.type_parameters,
                &definition.bounds,
            );
            self.types.push(TypeDefRow {
                flags: 0x0108 | u32::from(!definition.internal), // sealed, sequential layout
                type_name,
                type_namespace: if definition.builtin {
                    let suffix = if name.is_some() {
                        "Oxel.Compiler"
                    } else {
                        "Oxel"
                    };
                    match namespace {
                        Some(namespace) => self.strings.add(&format!("{namespace}.{suffix}")),
                        None => self.strings.add(suffix),
                    }
                } else {
                    source_namespace
                },
                extends: index(TableId::TypeRef, VALUE_TYPE_REF),
                field_list: first_field,
                method_list: method_end,
            });
            for field in &definition.fields {
                let mut signature = Writer::new();
                signature.write_u8(0x06); // FIELD
                self.write_type(&mut signature, field.ty);
                self.fields.push(FieldRow {
                    // Generated enum storage is accessible only within the assembly.
                    flags: if definition.union.is_some() {
                        0x0003
                    } else {
                        0x0006
                    },
                    name: self.strings.add(&field.name),
                    signature: self.blobs.add(signature.as_slice()),
                });
                if definition.union.is_none() {
                    let flags = self.nullable_flags(field.ty);
                    self.add_nullable_attribute(
                        index(TableId::Field, self.fields.len() as u32),
                        &flags,
                    );
                }
            }
        }
    }

    fn add_native_import(&mut self, method: u32, import: &NativeImport) {
        let name = self.strings.add(&import.library);
        let scope = *self.module_ref_indices.entry(name).or_insert_with(|| {
            self.module_refs.push(ModuleRefRow { name });
            self.module_refs.len() as u32
        });
        self.native_imports.push(ImplMapRow {
            mapping_flags: 0x0201, // Cdecl and exact spelling
            member_forwarded: index(TableId::MethodDef, method),
            import_name: self.strings.add(&import.symbol),
            import_scope: scope,
        });
    }

    fn add_parameters(&mut self, signature: &Signature, parameters: &[ParameterPassing]) -> u32 {
        let first = self.parameters.len() as u32 + 1;
        for (sequence, ty) in std::iter::once(signature.result)
            .chain(signature.parameters.iter().copied())
            .enumerate()
        {
            let nullable = self.nullable_flags(ty);
            let passing = sequence
                .checked_sub(1)
                .and_then(|index| parameters.get(index));
            let flags = match passing {
                Some(ParameterPassing::Out) => 0x0002,  // Out
                Some(ParameterPassing::Utf8) => 0x2000, // HasFieldMarshal
                _ => 0,
            };
            if nullable.is_empty() && flags == 0 {
                continue;
            }
            self.parameters.push(ParamRow {
                flags,
                sequence: sequence as u16,
                ..Default::default()
            });
            let parent = index(TableId::Param, self.parameters.len() as u32);
            self.add_nullable_attribute(parent, &nullable);
            if passing == Some(&ParameterPassing::Utf8) {
                self.field_marshals.push(FieldMarshalRow {
                    parent,
                    native_type: self.blobs.add(&[0x30]), // NATIVE_TYPE_LPUTF8STR
                });
            }
        }
        first
    }

    fn nullable_flags(&self, ty: Type) -> Vec<u8> {
        let mut flags = Vec::new();
        self.collect_nullable_flags(ty, &mut flags);
        if flags.iter().all(|&flag| flag == 0) {
            flags.clear(); // Only oblivious positions remain.
        }
        flags
    }

    fn collect_nullable_flags(&self, ty: Type, flags: &mut Vec<u8>) {
        match ty {
            Type::Array(id) | Type::NullableArray(id) => {
                flags.push(if ty.is_nullable() { 2 } else { 1 });
                self.collect_nullable_flags(self.instances[id].arguments[0], flags);
            }
            Type::ConstructedStruct(id)
            | Type::ConstructedClrStruct(id)
            | Type::ConstructedExternal(id)
            | Type::NullableConstructedExternal(id) => {
                let instance = &self.instances[id];
                if !self
                    .externals
                    .nullable_value
                    .is_some_and(|definition| instance.kind == InstanceKind::External(definition))
                {
                    flags.push(match ty {
                        Type::NullableConstructedExternal(_) => 2,
                        Type::ConstructedExternal(_) => 1,
                        _ => 0,
                    });
                }
                for &argument in self.instances[id].arguments.iter() {
                    self.collect_nullable_flags(argument, flags);
                }
            }
            Type::TypeParameter(_) | Type::MethodParameter(_) => flags.push(0),
            ty if ty.reference() => flags.push(if ty == ty.non_nullable() { 1 } else { 2 }),
            _ => {} // Non-generic value types occupy no slot.
        }
    }

    fn add_nullable_attribute(&mut self, parent: CodedIndex, flags: &[u8]) {
        let Some(&first) = flags.first() else { return };
        let array = flags.iter().any(|&value| value != first);
        let ctor = self.nullable_constructor(array);
        let mut value = Writer::new();
        value.write_u16(1); // custom attribute prolog
        if array {
            value.write_u32(flags.len() as u32);
            value.write_bytes(flags);
        } else {
            value.write_u8(first);
        }
        value.write_u16(0); // no named arguments
        self.custom_attributes.push(CustomAttributeRow {
            parent,
            attr_type: index(TableId::MemberRef, ctor),
            value: self.blobs.add(value.as_slice()),
        });
    }

    fn nullable_constructor(&mut self, array: bool) -> u32 {
        let variant = usize::from(array);
        if let Some(row) = self.nullable_ctors[variant] {
            return row;
        }
        let owner = if let Some(row) = self.nullable_type {
            row
        } else {
            self.type_refs.push(TypeRefRow {
                resolution_scope: index(TableId::AssemblyRef, 1), // System.Runtime
                type_name: self.strings.add("NullableAttribute"),
                type_namespace: self.strings.add("System.Runtime.CompilerServices"),
            });
            let row = self.type_refs.len() as u32;
            self.nullable_type = Some(row);
            row
        };
        let signature = if array {
            self.blobs.add(&[0x20, 1, 0x01, 0x1d, 0x05]) // instance void .ctor(byte[])
        } else {
            self.blobs.add(&[0x20, 1, 0x01, 0x05]) // instance void .ctor(byte)
        };
        let row = self.push_member(owner, ".ctor", signature);
        self.nullable_ctors[variant] = Some(row);
        row
    }

    pub(super) fn struct_token(&mut self, ty: Type) -> u32 {
        match ty {
            Type::Struct(id) => TYPE_DEF | struct_row(id),
            Type::ConstructedStruct(_) => self.specification_token(ty),
            _ => unreachable!("checked struct type"),
        }
    }

    pub(super) fn field_token(&mut self, owner: Type, field: usize) -> u32 {
        match owner {
            Type::Struct(id) => FIELD_DEF | (self.first_fields[id] + field as u32),
            Type::ConstructedStruct(id) => {
                if let Some(&row) = self.field_ref_indices.get(&(id, field)) {
                    return MEMBER_REF | row;
                }
                let class = index(TableId::TypeSpec, self.type_spec(owner));
                let InstanceKind::Struct(definition) = self.instances[id].kind else {
                    unreachable!("constructed source type")
                };
                let field_row = &self.fields[self.first_fields[definition] as usize - 1 + field];
                self.members.push(MemberRefRow {
                    class,
                    name: field_row.name,
                    signature: field_row.signature,
                });
                let row = self.members.len() as u32;
                self.field_ref_indices.insert((id, field), row);
                MEMBER_REF | row
            }
            _ => unreachable!("checked field receiver"),
        }
    }

    pub(super) fn specification_token(&mut self, ty: Type) -> u32 {
        TYPE_SPEC | self.type_spec(ty)
    }

    pub(super) fn runtime_type_token(&mut self, ty: Type) -> u32 {
        let ty = ty.non_nullable();
        const TYPE_REF: u32 = 0x0100_0000;
        match ty {
            Type::Struct(_) | Type::ConstructedStruct(_) => self.struct_token(ty),
            Type::MethodParameter(_)
            | Type::ConstructedExternal(_)
            | Type::ConstructedClrStruct(_)
            | Type::Array(_) => self.specification_token(ty),
            Type::External(id) | Type::ClrEnum(id) | Type::ClrStruct(id) => {
                TYPE_REF | self.type_reference(id)
            }
            Type::Object => TYPE_REF | 1,
            Type::String => TYPE_REF | 3,
            _ => {
                let row = *self.primitive_type_indices.entry(ty).or_insert_with(|| {
                    let full_name = ty.to_string();
                    let name = full_name
                        .strip_prefix("System.")
                        .expect("checked CLR primitive");
                    self.type_refs.push(TypeRefRow {
                        resolution_scope: index(TableId::AssemblyRef, 1), // System.Runtime
                        type_name: self.strings.add(name),
                        type_namespace: self.strings.add("System"),
                    });
                    self.type_refs.len() as u32
                });
                TYPE_REF | row
            }
        }
    }

    fn type_spec(&mut self, ty: Type) -> u32 {
        if let Some(&row) = self.type_spec_indices.get(&ty) {
            return row;
        }
        let mut signature = Writer::new();
        self.write_type(&mut signature, ty);
        self.type_specs.push(TypeSpecRow {
            signature: self.blobs.add(signature.as_slice()),
        });
        let row = self.type_specs.len() as u32;
        self.type_spec_indices.insert(ty, row);
        row
    }

    pub(super) fn cell_array_token(&mut self, element: Type) -> u32 {
        if let Some(&row) = self.cell_array_indices.get(&element) {
            return TYPE_SPEC | row;
        }
        let mut signature = Writer::new();
        signature.write_u8(0x1d); // ELEMENT_TYPE_SZARRAY
        self.write_type(&mut signature, element);
        self.type_specs.push(TypeSpecRow {
            signature: self.blobs.add(signature.as_slice()),
        });
        let row = self.type_specs.len() as u32;
        self.cell_array_indices.insert(element, row);
        TYPE_SPEC | row
    }

    pub(super) fn user_string(&mut self, value: &str) -> u32 {
        if let Some(&offset) = self.user_string_offsets.get(value) {
            return offset;
        }
        // An ldstr token contains a 24-bit #US offset.
        const HEAP_LIMIT: usize = 1 << 24;
        let offset = self.user_strings.len();
        let units = value.encode_utf16().count();
        if units >= HEAP_LIMIT / 2 {
            self.user_strings_overflow = true;
            return 0;
        }
        let payload_size = units * 2 + 1;
        let prefix_size = match payload_size {
            0..=0x7f => 1,
            0x80..=0x3fff => 2,
            _ => 4,
        };
        if offset + prefix_size + payload_size > HEAP_LIMIT {
            self.user_strings_overflow = true;
            return 0;
        }
        self.user_strings.write_compressed_uint(payload_size as u32);
        let mut needs_special_handling = false;
        for unit in value.encode_utf16() {
            self.user_strings.write_u16(unit);
            // ECMA-335 II.24.2.4 defines this flag by UTF-16 code unit.
            needs_special_handling |=
                unit > 0xff || matches!(unit, 0x01..=0x08 | 0x0e..=0x1f | 0x27 | 0x2d | 0x7f);
        }
        self.user_strings.write_u8(u8::from(needs_special_handling));
        self.user_string_offsets.insert(value.into(), offset as u32);
        offset as u32
    }

    pub(super) fn value_member(&self, id: usize) -> bool {
        let owner = self
            .externals
            .imported_type(self.externals.method(id).owner);
        owner.value_struct || owner.builtin.is_some_and(Type::scalar)
    }

    pub(super) fn wide_integer(&self, ty: Type) -> bool {
        self.integer_storage(ty).wide_integer()
    }

    pub(super) fn integer_storage(&self, ty: Type) -> Type {
        self.externals.integer_storage(ty)
    }

    pub(super) fn member(&mut self, id: usize, owner: Option<Type>) -> (Invocation, u32) {
        let method = self.externals.method(id);
        let invocation = method.invocation;
        if let Some(&row) = self.member_indices.get(&(id, owner)) {
            return (invocation, row);
        }
        let parent = self.external_member_parent(method.owner, owner);
        let Signature {
            parameters,
            result,
            type_arity,
            ..
        } = &method.signature;
        let signature = self.signature_with_modes(
            parameters,
            *result,
            invocation,
            *type_arity,
            method
                .modes
                .iter()
                .copied()
                .enumerate()
                .map(|(index, mode)| (mode, method.required_ins.binary_search(&index).is_ok())),
        );
        let row = self.push_member_for(parent, &method.name, signature);
        self.member_indices.insert((id, owner), row);
        (invocation, row)
    }

    pub(super) fn external_field(&mut self, id: usize, owner: Option<Type>) -> (bool, bool, u32) {
        let field = self.externals.field(id);
        if let Some(&row) = self.external_field_indices.get(&(id, owner)) {
            return (field.is_static, field.volatile, row);
        }
        let parent = self.external_member_parent(field.owner, owner);
        let mut signature = Writer::new();
        signature.write_u8(0x06); // FIELD
        if field.volatile {
            let marker = self.volatile_type();
            signature.write_u8(0x1f); // ELEMENT_TYPE_CMOD_REQD
            signature.write_compressed_uint((marker << 2) | 1); // TypeRef
        }
        self.write_type(&mut signature, field.ty.expect("checked field signature"));
        let signature = self.blobs.add(signature.as_slice());
        let row = self.push_member_for(parent, &field.name, signature);
        self.external_field_indices.insert((id, owner), row);
        (field.is_static, field.volatile, row)
    }

    fn volatile_type(&mut self) -> u32 {
        *self.volatile_type.get_or_insert_with(|| {
            self.type_refs.push(TypeRefRow {
                resolution_scope: index(TableId::AssemblyRef, 1), // System.Runtime
                type_name: self.strings.add("IsVolatile"),
                type_namespace: self.strings.add("System.Runtime.CompilerServices"),
            });
            self.type_refs.len() as u32
        })
    }

    fn in_attribute_type(&mut self) -> u32 {
        *self.in_attribute_type.get_or_insert_with(|| {
            self.type_refs.push(TypeRefRow {
                resolution_scope: index(TableId::AssemblyRef, 1), // System.Runtime
                type_name: self.strings.add("InAttribute"),
                type_namespace: self.strings.add("System.Runtime.InteropServices"),
            });
            self.type_refs.len() as u32
        })
    }

    pub(super) fn slice_indexer(&mut self, owner: Type, readonly: bool) -> u32 {
        if let Some(&row) = self.slice_indexers.get(&owner) {
            return MEMBER_REF | row;
        }
        let parent = index(TableId::TypeSpec, self.type_spec(owner));
        let mut signature = Writer::new();
        signature.write_u8(0x20); // HASTHIS
        signature.write_u8(1); // one parameter
        if readonly {
            signature.write_u8(0x1f); // CMOD_REQD
            signature.write_compressed_uint((self.in_attribute_type() << 2) | 1);
        }
        signature.write_bytes(&[0x10, 0x13, 0, 0x08]); // ref !0 get_Item(int32)
        let signature = self.blobs.add(signature.as_slice());
        let row = self.push_member_for(parent, "get_Item", signature);
        self.slice_indexers.insert(owner, row);
        MEMBER_REF | row
    }

    fn external_member_parent(&mut self, definition: usize, owner: Option<Type>) -> CodedIndex {
        if let Some(owner) = owner {
            index(TableId::TypeSpec, self.type_spec(owner))
        } else {
            index(TableId::TypeRef, self.type_reference(definition))
        }
    }

    pub(super) fn delegate_ctor(&mut self, definition: usize, delegate: Type) -> u32 {
        let delegate = delegate.non_nullable();
        if let Some(&token) = self.delegate_ctors.get(&delegate) {
            return token;
        }
        let parent = self.external_member_parent(
            definition,
            (!matches!(delegate, Type::External(_))).then_some(delegate),
        );
        // instance void .ctor(object target, native int method)
        let signature = self.blobs.add(&[0x20, 0x02, 0x01, 0x1c, 0x18]);
        let token = MEMBER_REF | self.push_member_for(parent, ".ctor", signature);
        self.delegate_ctors.insert(delegate, token);
        token
    }

    fn push_member(&mut self, owner: u32, name: &str, signature: u32) -> u32 {
        self.push_member_for(index(TableId::TypeRef, owner), name, signature)
    }

    pub(super) fn error_writer_members(&mut self) -> (u32, u32) {
        if let Some(members) = self.error_writer_members {
            return members;
        }
        self.type_refs.push(TypeRefRow {
            resolution_scope: index(TableId::AssemblyRef, 1), // System.Runtime
            type_name: self.strings.add("TextWriter"),
            type_namespace: self.strings.add("System.IO"),
        });
        let writer = self.type_refs.len() as u32;
        let mut signature = Writer::new();
        signature.write_bytes(&[0x00, 0, 0x12]); // static TextWriter ()
        signature.write_compressed_uint((writer << 2) | 1);
        let signature = self.blobs.add(signature.as_slice());
        let getter = MEMBER_REF | self.push_member(2, "get_Error", signature);
        let signature = self.method_signature(&[Type::Object], Type::Void, Invocation::Instance, 0);
        let write_line = MEMBER_REF | self.push_member(writer, "WriteLine", signature);
        let members = (getter, write_line);
        self.error_writer_members = Some(members);
        members
    }

    fn push_member_for(&mut self, class: CodedIndex, name: &str, signature: u32) -> u32 {
        self.members.push(MemberRefRow {
            class,
            name: self.strings.add(name),
            signature,
        });
        self.members.len() as u32
    }

    fn type_reference(&mut self, id: usize) -> u32 {
        if let Some(&row) = self.type_indices.get(&id) {
            return row;
        }
        let ty = self.externals.imported_type(id);
        if let Some(builtin) = ty.builtin {
            let row = self.runtime_type_token(builtin) & 0x00ff_ffff;
            self.type_indices.insert(id, row);
            return row;
        }
        let assembly_row = *self.assembly_indices.entry(ty.assembly).or_insert_with(|| {
            let assembly = self.externals.assembly(ty.assembly);
            self.references.push(assembly_reference(
                assembly,
                &mut self.strings,
                &mut self.blobs,
            ));
            self.references.len() as u32
        });
        self.type_refs.push(TypeRefRow {
            resolution_scope: index(TableId::AssemblyRef, assembly_row),
            type_name: self.strings.add(&ty.name),
            type_namespace: self.strings.add(&ty.namespace),
        });
        let row = self.type_refs.len() as u32;
        self.type_indices.insert(id, row);
        row
    }

    fn add_type_parameters(
        &mut self,
        owner: CodedIndex,
        parameters: &[&str],
        bounds: &[GenericBound],
    ) {
        let mut bounds = bounds.iter().peekable();
        for (position, name) in parameters.iter().enumerate() {
            let number = position as u16;
            let mut constraints = Vec::new();
            let mut flags = 0;
            while let Some(bound) = bounds.next_if(|bound| bound.parameter == number) {
                flags |= bound.kind.special_flag();
                if let TypeBoundKind::Target(ty) = bound.kind {
                    let target = match ty {
                        Type::External(id) => index(TableId::TypeRef, self.type_reference(id)),
                        Type::ConstructedExternal(_)
                        | Type::MethodParameter(_)
                        | Type::TypeParameter(_) => index(TableId::TypeSpec, self.type_spec(ty)),
                        _ => unreachable!("checked generic bound"),
                    };
                    constraints.push(target);
                }
            }
            if flags & 8 != 0 {
                flags |= TypeBoundKind::Constructor.special_flag();
                constraints.insert(0, index(TableId::TypeRef, VALUE_TYPE_REF));
            }
            self.generic_params.push(GenericParameter {
                row: GenericParamRow {
                    number,
                    flags,
                    owner,
                    name: self.strings.add(name),
                },
                bounds: constraints,
            });
        }
    }

    pub(super) fn string_from_object(&mut self) -> u32 {
        if let Some(member) = self.string_from_object {
            return MEMBER_REF | member;
        }
        let signature = self.method_signature(&[Type::Object], Type::String, Invocation::Static, 0);
        let member = self.push_member(STRING_REF, "Concat", signature);
        self.string_from_object = Some(member);
        MEMBER_REF | member
    }

    pub(super) fn string_array_concat(&mut self) -> u32 {
        if let Some(member) = self.string_array_concat {
            return MEMBER_REF | member;
        }
        let signature = self.blobs.add(&[0x00, 1, 0x0e, 0x1d, 0x0e]); // string (string[])
        let member = self.push_member(STRING_REF, "Concat", signature);
        self.string_array_concat = Some(member);
        MEMBER_REF | member
    }

    pub(super) fn default_constructor(&mut self, ty: Type) -> u32 {
        let member = if let Some(member) = self.create_instance {
            member
        } else {
            self.type_refs.push(TypeRefRow {
                resolution_scope: index(TableId::AssemblyRef, 1), // System.Runtime
                type_name: self.strings.add("Activator"),
                type_namespace: self.strings.add("System"),
            });
            let signature =
                self.method_signature(&[], Type::ImportedMethodParameter(0), Invocation::Static, 1);
            let member = self.push_member(self.type_refs.len() as u32, "CreateInstance", signature);
            self.create_instance = Some(member);
            member
        };
        self.instantiate_method(MEMBER_REF | member, &[ty])
    }

    pub(super) fn instantiate_method(&mut self, token: u32, arguments: &[Type]) -> u32 {
        if arguments.is_empty() {
            return token;
        }
        if let Some(&row) = self
            .method_spec_indices
            .get(&token)
            .and_then(|rows| rows.get(arguments))
        {
            return METHOD_SPEC | row;
        }
        let table = if token >> 24 == TableId::MethodDef as u32 {
            TableId::MethodDef
        } else {
            TableId::MemberRef
        };
        let mut signature = Writer::new();
        signature.write_u8(0x0a); // GENERICINST method instantiation
        signature.write_compressed_uint(arguments.len() as u32);
        for &ty in arguments {
            self.write_type(&mut signature, ty);
        }
        let row = self.method_specs.len() as u32 + 1;
        self.method_specs.push(MethodSpecRow {
            method: index(table, token & 0x00ff_ffff),
            instantiation: self.blobs.add(signature.as_slice()),
        });
        self.method_spec_indices
            .entry(token)
            .or_default()
            .insert(arguments.into(), row);
        METHOD_SPEC | row
    }

    fn method_signature(
        &mut self,
        parameters: &[Type],
        result: Type,
        invocation: Invocation,
        type_arity: u16,
    ) -> u32 {
        self.signature_with_modes(parameters, result, invocation, type_arity, [])
    }

    fn signature_with_modes(
        &mut self,
        parameters: &[Type],
        result: Type,
        invocation: Invocation,
        type_arity: u16,
        modes: impl IntoIterator<Item = (ArgumentMode, bool)>,
    ) -> u32 {
        let mut signature = Writer::new();
        let has_this = if invocation == Invocation::Static {
            0
        } else {
            0x20
        };
        let generic = if type_arity == 0 { 0 } else { 0x10 };
        signature.write_u8(has_this | generic);
        if type_arity != 0 {
            signature.write_compressed_uint(u32::from(type_arity));
        }
        signature.write_compressed_uint(parameters.len() as u32);
        self.write_type(&mut signature, result);
        let mut modes = modes.into_iter();
        for &parameter in parameters {
            let (mode, required_in) = modes.next().unwrap_or((ArgumentMode::Value, false));
            if required_in {
                signature.write_u8(0x1f); // ELEMENT_TYPE_CMOD_REQD
                signature.write_compressed_uint((self.in_attribute_type() << 2) | 1); // TypeRef
            }
            if mode != ArgumentMode::Value {
                signature.write_u8(0x10); // ELEMENT_TYPE_BYREF
            }
            self.write_type(&mut signature, parameter);
        }
        self.blobs.add(signature.as_slice())
    }

    fn write_type(&mut self, signature: &mut Writer, ty: Type) {
        if let Some(element) = ty.clr_element() {
            signature.write_u8(element);
            return;
        }
        match ty {
            Type::Struct(id) => {
                signature.write_u8(0x11); // ELEMENT_TYPE_VALUETYPE
                signature.write_compressed_uint(struct_row(id) << 2); // TypeDefOrRefEncoded
            }
            Type::Array(id) | Type::NullableArray(id) => {
                signature.write_u8(0x1d); // ELEMENT_TYPE_SZARRAY
                self.write_type(signature, self.instances[id].arguments[0]);
            }
            Type::ImportedArray(id) | Type::NullableImportedArray(id) => {
                signature.write_u8(0x1d); // ELEMENT_TYPE_SZARRAY
                self.write_type(signature, self.externals.signature_element(id));
            }
            Type::ConstructedStruct(id)
            | Type::ConstructedClrStruct(id)
            | Type::ConstructedExternal(id)
            | Type::NullableConstructedExternal(id) => {
                let instance = &self.instances[id];
                let (kind, definition) = match instance.kind {
                    InstanceKind::Struct(id) => (0x11, struct_row(id) << 2),
                    InstanceKind::External(id) => (
                        if ty.clr_struct() { 0x11 } else { 0x12 },
                        (self.type_reference(id) << 2) | 1,
                    ),
                    InstanceKind::Array => unreachable!("generic type instance"),
                };
                self.write_generic_instance(signature, kind, definition, &instance.arguments);
            }
            Type::ImportedInstance(id) | Type::NullableImportedInstance(id) => {
                let instance = self.externals.signature_instance(id);
                let definition = (self.type_reference(instance.definition) << 2) | 1;
                let kind = if self
                    .externals
                    .imported_type(instance.definition)
                    .value_struct
                {
                    0x11
                } else {
                    0x12
                };
                self.write_generic_instance(signature, kind, definition, &instance.arguments);
            }
            Type::TypeParameter(index) | Type::NullableTypeParameter(index) => {
                signature.write_u8(0x13); // ELEMENT_TYPE_VAR
                signature.write_compressed_uint(u32::from(index));
            }
            Type::External(id)
            | Type::NullableExternal(id)
            | Type::ClrEnum(id)
            | Type::ClrStruct(id) => {
                signature.write_u8(if ty.imported_value() { 0x11 } else { 0x12 });
                signature.write_compressed_uint((self.type_reference(id) << 2) | 1); // TypeRef
            }
            Type::MethodParameter(index)
            | Type::ImportedMethodParameter(index)
            | Type::NullableImportedMethodParameter(index) => {
                signature.write_u8(0x1e); // ELEMENT_TYPE_MVAR
                signature.write_compressed_uint(u32::from(index));
            }
            _ => unreachable!("checked expression type has a CLR signature"),
        }
    }

    fn write_generic_instance(
        &mut self,
        signature: &mut Writer,
        kind: u8,
        definition: u32,
        arguments: &[Type],
    ) {
        signature.write_u8(0x15); // GENERICINST
        signature.write_u8(kind);
        signature.write_compressed_uint(definition);
        signature.write_compressed_uint(arguments.len() as u32);
        for &argument in arguments {
            self.write_type(signature, argument);
        }
    }

    pub(super) fn add_locals(
        &mut self,
        locals: &[Type],
        captured_cells: &std::collections::HashSet<u16>,
        spills: &[StackSlot],
    ) -> u32 {
        if locals.is_empty() && spills.is_empty() {
            return 0;
        }
        let mut signature = Writer::new();
        signature.write_u8(0x07); // LOCAL_SIG
        signature.write_compressed_uint((locals.len() + spills.len()) as u32);
        for (index, &ty) in locals.iter().enumerate() {
            if captured_cells.contains(&(index as u16)) {
                signature.write_u8(0x1d); // ELEMENT_TYPE_SZARRAY
            }
            self.write_type(&mut signature, ty);
        }
        for &slot in spills {
            let ty = match slot {
                StackSlot::Value(ty) => ty,
                StackSlot::Address(ty) => {
                    signature.write_u8(0x10); // ELEMENT_TYPE_BYREF
                    ty
                }
                StackSlot::Vector(ty) => {
                    signature.write_u8(0x1d); // ELEMENT_TYPE_SZARRAY
                    ty
                }
            };
            self.write_type(&mut signature, ty);
        }
        self.locals.push(StandAloneSigRow {
            signature: self.blobs.add(signature.as_slice()),
        });
        STAND_ALONE_SIG | self.locals.len() as u32
    }

    fn finish(mut self) -> Result<Vec<u8>, Diagnostic> {
        if self.user_strings_overflow {
            return Err(Diagnostic::new(
                "E0402",
                Span::default(),
                "string literals exceed the CLR user string heap limit",
            ));
        }
        let narrow_limit = usize::from(u16::MAX);
        if self.parameters.len() == narrow_limit
            && self
                .methods
                .iter()
                .any(|method| method.param_list as usize > narrow_limit)
        {
            return Err(Diagnostic::new(
                "E0402",
                Span::default(),
                "CLR parameter list index exceeds its encoding width",
            ));
        }
        if self.members.len() > 0x00ff_ffff
            || self.type_refs.len() > 0x00ff_ffff
            || self.parameters.len() > 0x00ff_ffff
            || self.custom_attributes.len() > 0x00ff_ffff
            || self.type_specs.len() > 0x00ff_ffff
            || self.references.len() > 0x00ff_ffff
            || self.generic_params.len() > 0x00ff_ffff
            || self.generic_constraint_count() > 0x00ff_ffff
            || self.method_specs.len() > 0x00ff_ffff
        {
            return Err(image_too_large());
        }
        self.generic_params.sort_unstable_by_key(|row| {
            (
                row.row.owner.encode(CodedIndexKind::TypeOrMethodDef),
                row.row.number,
            )
        });
        self.custom_attributes
            .sort_unstable_by_key(|row| row.parent.encode(CodedIndexKind::HasCustomAttribute));
        let tables = self.table_stream();
        let streams = [
            ("#~", tables.as_slice()),
            ("#Strings", self.strings.data()),
            ("#US", self.user_strings.as_slice()),
            ("#GUID", self.guids.data()),
            ("#Blob", self.blobs.data()),
        ];
        let mut root = MetadataRoot {
            major_version: 1,
            minor_version: 1,
            reserved: 0,
            version: "v4.0.30319".into(),
            flags: 0,
            streams: streams
                .iter()
                .map(|(name, _)| StreamHeader {
                    name: (*name).into(),
                    size: 0,
                    offset: 0,
                })
                .collect(),
        };
        let mut offset = root.header_size();
        for (header, (_, data)) in root.streams.iter_mut().zip(streams) {
            header.offset = image_offset(offset)?;
            header.size = image_offset(data.len())?;
            offset = (offset + data.len()).next_multiple_of(4);
        }
        let mut output = Writer::with_capacity(image_offset(offset)? as usize);
        root.write_to(&mut output);
        for (_, data) in streams {
            output.write_bytes(data);
            output.align(4);
        }
        Ok(output.into_inner())
    }

    fn generic_constraint_count(&self) -> usize {
        self.generic_params
            .iter()
            .map(|parameter| parameter.bounds.len())
            .sum()
    }

    fn table_stream(&self) -> Vec<u8> {
        let mut header = TablesHeader {
            reserved: 0,
            major_version: 2,
            minor_version: 0,
            reserved2: 1,
            heap_sizes: u8::from(self.strings.uses_wide_indices())
                | (u8::from(self.guids.uses_wide_indices()) << 1)
                | (u8::from(self.blobs.uses_wide_indices()) << 2),
            valid: 0,
            sorted: if self.generic_params.is_empty() {
                0
            } else {
                1 << TableId::GenericParam as u8
            },
            row_counts: [0; 64],
            uncompressed: false,
        };
        if !self.custom_attributes.is_empty() {
            header.sorted |= 1 << TableId::CustomAttribute as u8;
        }
        if !self.field_marshals.is_empty() {
            header.sorted |= 1 << TableId::FieldMarshal as u8;
        }
        if !self.native_imports.is_empty() {
            header.sorted |= 1 << TableId::ImplMap as u8;
        }
        let constraint_count = self.generic_constraint_count();
        if constraint_count != 0 {
            header.sorted |= 1 << TableId::GenericParamConstraint as u8;
        }
        let mut stream = Writer::new();
        // ECMA-335 stores rows in increasing table-number order.
        let context = {
            for (table, count) in [
                (TableId::Module, 1),
                (TableId::TypeRef, self.type_refs.len()),
                (TableId::TypeDef, self.types.len()),
                (TableId::Field, self.fields.len()),
                (TableId::MethodDef, self.methods.len()),
                (TableId::Param, self.parameters.len()),
                (TableId::MemberRef, self.members.len()),
                (TableId::CustomAttribute, self.custom_attributes.len()),
                (TableId::FieldMarshal, self.field_marshals.len()),
                (TableId::StandAloneSig, self.locals.len()),
                (TableId::ModuleRef, self.module_refs.len()),
                (TableId::TypeSpec, self.type_specs.len()),
                (TableId::ImplMap, self.native_imports.len()),
                (TableId::Assembly, 1),
                (TableId::AssemblyRef, self.references.len()),
                (TableId::GenericParam, self.generic_params.len()),
                (TableId::MethodSpec, self.method_specs.len()),
                (TableId::GenericParamConstraint, constraint_count),
            ] {
                header.set_row_count(table, count as u32);
            }
            header.write_to(&mut stream);
            header.context()
        };
        self.module.write(&mut stream, &context);
        for row in &self.type_refs {
            row.write(&mut stream, &context);
        }
        for row in &self.types {
            row.write(&mut stream, &context);
        }
        for row in &self.fields {
            row.write(&mut stream, &context);
        }
        for row in &self.methods {
            row.write(&mut stream, &context);
        }
        for row in &self.parameters {
            row.write(&mut stream, &context);
        }
        for row in &self.members {
            row.write(&mut stream, &context);
        }
        for row in &self.custom_attributes {
            row.write(&mut stream, &context);
        }
        for row in &self.field_marshals {
            row.write(&mut stream, &context);
        }
        for row in &self.locals {
            row.write(&mut stream, &context);
        }
        for row in &self.module_refs {
            row.write(&mut stream, &context);
        }
        for row in &self.type_specs {
            row.write(&mut stream, &context);
        }
        for row in &self.native_imports {
            row.write(&mut stream, &context);
        }
        self.assembly.write(&mut stream, &context);
        for row in &self.references {
            row.write(&mut stream, &context);
        }
        for parameter in &self.generic_params {
            parameter.row.write(&mut stream, &context);
        }
        for row in &self.method_specs {
            row.write(&mut stream, &context);
        }
        for (index, parameter) in self.generic_params.iter().enumerate() {
            for &constraint in &parameter.bounds {
                GenericParamConstraintRow {
                    owner: index as u32 + 1,
                    constraint,
                }
                .write(&mut stream, &context);
            }
        }
        stream.into_inner()
    }
}

fn index(table: TableId, row: u32) -> CodedIndex {
    CodedIndex {
        table: Some(table),
        row,
    }
}

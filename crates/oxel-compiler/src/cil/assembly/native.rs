use super::*;
use crate::{
    hir::{Function, NativeCallback},
    syntax::BufferLength,
};

pub(super) struct Imports<'a> {
    wrappers: Vec<Wrapper<'a>>,
    positions: Vec<Option<usize>>,
    first_callback_method: u32,
    callback_count: usize,
}

pub(super) struct Wrapper<'a> {
    source: &'a Function,
    raw_method: u32,
    callbacks: Vec<Callback<'a>>,
}

struct Callback<'a> {
    ty: u32,
    ctor: u32,
    descriptor: &'a NativeCallback,
}

impl<'a> Imports<'a> {
    pub(super) fn new(
        methods: &[&'a Function],
        order: &[usize],
        first_raw: usize,
        first_type: usize,
        interface_methods: usize,
    ) -> Self {
        let mut wrappers = Vec::new();
        let mut positions = vec![None; methods.len()];
        let mut callback_count = 0;
        for &id in order {
            let source = methods[id];
            let MethodBody::Native {
                parameters,
                callbacks,
                ..
            } = &source.body
            else {
                continue;
            };
            if callbacks.is_empty()
                && !parameters
                    .iter()
                    .any(|mode| matches!(mode, ParameterPassing::Buffer(..)))
            {
                continue;
            }
            positions[id] = Some(wrappers.len());
            let callbacks = callbacks
                .iter()
                .map(|descriptor| {
                    callback_count += 1;
                    Callback {
                        ty: (first_type + callback_count - 1) as u32,
                        ctor: 0,
                        descriptor,
                    }
                })
                .collect();
            wrappers.push(Wrapper {
                source,
                raw_method: (first_raw + wrappers.len() + 1) as u32,
                callbacks,
            });
        }
        let first_callback_method = (first_raw + wrappers.len() + interface_methods + 1) as u32;
        for (index, callback) in wrappers
            .iter_mut()
            .flat_map(|wrapper| &mut wrapper.callbacks)
            .enumerate()
        {
            callback.ctor = first_callback_method + 2 * index as u32;
        }
        Self {
            wrappers,
            positions,
            first_callback_method,
            callback_count,
        }
    }

    pub(super) fn method_end(&self) -> usize {
        self.first_callback_method as usize - 1 + 2 * self.callback_count
    }

    pub(super) fn callback_count(&self) -> usize {
        self.callback_count
    }

    pub(super) fn first_callback_method(&self) -> u32 {
        self.first_callback_method
    }

    pub(super) fn wrapper(&self, id: usize) -> Option<&Wrapper<'_>> {
        self.positions[id].map(|position| &self.wrappers[position])
    }

    pub(super) fn finish_imports(&self, metadata: &mut Tables<'_>) {
        for wrapper in &self.wrappers {
            wrapper.add_raw_import(metadata);
        }
    }

    pub(super) fn finish_callbacks(self, metadata: &mut Tables<'_>) {
        if self.callback_count == 0 {
            return;
        }
        let delegate = metadata.native_type_ref(1, "System", "MulticastDelegate");
        let interop = metadata.native_interop_reference();
        let attr = metadata.native_type_ref(
            interop,
            "System.Runtime.InteropServices",
            "UnmanagedFunctionPointerAttribute",
        );
        let convention = metadata.native_type_ref(
            interop,
            "System.Runtime.InteropServices",
            "CallingConvention",
        );
        let mut signature = Writer::new();
        signature.write_bytes(&[0x20, 1, 0x01, 0x11]); // instance void .ctor(valuetype ...)
        signature.write_compressed_uint((convention << 2) | 1);
        let signature = metadata.blobs.add(signature.as_slice());
        let attr_ctor = metadata.push_member(attr, ".ctor", signature);
        for callback in self.wrappers.iter().flat_map(|wrapper| &wrapper.callbacks) {
            callback.add_type(metadata, delegate, attr_ctor);
        }
    }
}

impl Wrapper<'_> {
    pub(super) fn emit(
        &self,
        output: &mut Writer,
        metadata: &mut Tables<'_>,
        function: &Function,
        parameters: &[ParameterPassing],
        span: Span,
    ) -> Result<(), Diagnostic> {
        let empty = Default::default();
        let mut body = Body::new(metadata, 0, &[], &empty, false);
        let mut callbacks = self.callbacks.iter();
        let mut stack = 0;
        for (index, mode) in parameters.iter().enumerate() {
            if *mode == ParameterPassing::CallbackRoot {
                continue;
            }
            let root = (mode == &ParameterPassing::Callback(true)).then_some(index as u16 + 1);
            if let Some(root) = root {
                body.indexed(0x0e, 0x09, root);
            }
            body.indexed(0x0e, 0x09, index as u16); // ldarg
            stack += 1;
            body.max_stack = body.max_stack.max(stack);
            match mode {
                ParameterPassing::Buffer(length, _) => {
                    body.op(0x25); // dup
                    body.op(0x8e); // ldlen (native unsigned)
                    match length {
                        BufferLength::Native => {}
                        BufferLength::U32 => body.op(0x6d), // conv.u4
                        BufferLength::U64 => body.op(0x6e), // conv.u8
                    }
                    stack += 1;
                    body.max_stack = body.max_stack.max(stack);
                }
                ParameterPassing::Callback(_) => {
                    let callback = callbacks.next().expect("checked native callback");
                    body.op(0x25); // dup
                    let nonnull = body.branch(0x3a); // brtrue
                    body.op(0x26); // pop
                    body.op(0x14); // ldnull
                    let end = body.branch(0x38);
                    body.patch(nonnull);
                    body.op(0x25); // bind the original delegate's Invoke to a Cdecl delegate
                    let invoke = callback.descriptor.invoke;
                    let (_, member) = body.metadata.member(invoke.id, invoke.owner);
                    body.extended(0x07); // ldvirtftn
                    body.code
                        .extend_from_slice(&(MEMBER_REF | member).to_le_bytes());
                    body.token_instruction(0x73, METHOD_DEF | callback.ctor); // newobj
                    body.patch(end);
                    if let Some(root) = root {
                        body.op(0x51); // stind.ref
                        body.indexed(0x0e, 0x09, root);
                        body.op(0x50); // ldind.ref
                        body.token_instruction(0x74, TYPE_DEF | callback.ty);
                    }
                    body.max_stack = body.max_stack.max(stack + 1 + usize::from(root.is_some()));
                }
                _ => {}
            }
        }
        body.call(METHOD_DEF | self.raw_method);
        if function.signature.result != Type::Void {
            body.max_stack = body.max_stack.max(1);
        }
        body.op(0x2a);
        body.write(output, span).map(|_| ())
    }

    fn add_raw_import(&self, metadata: &mut Tables<'_>) {
        let MethodBody::Native {
            import, parameters, ..
        } = &self.source.body
        else {
            unreachable!()
        };
        debug_assert_eq!(self.raw_method as usize, metadata.methods.len() + 1);
        metadata.add_native_import(self.raw_method, import);
        let count: usize = parameters.iter().map(|mode| mode.native_width()).sum();
        let result = self.source.signature.result;
        let mut raw = Signature {
            type_arity: 0,
            bounds: Vec::new(),
            parameters: Vec::with_capacity(count),
            result,
        };
        let mut modes = Vec::with_capacity(count);
        let mut signature = Writer::new();
        signature.write_u8(0); // static default
        signature.write_compressed_uint(count as u32);
        metadata.write_type(&mut signature, result);
        let mut callbacks = self.callbacks.iter();
        for (&mode, &ty) in parameters.iter().zip(&self.source.signature.parameters) {
            if mode == ParameterPassing::CallbackRoot {
                continue;
            }
            if matches!(mode, ParameterPassing::Callback(_)) {
                let callback = callbacks.next().expect("checked native callback");
                signature.write_u8(0x12); // ELEMENT_TYPE_CLASS
                signature.write_compressed_uint(callback.ty << 2);
                raw.parameters.push(if ty.is_nullable() {
                    Type::NullableObject
                } else {
                    Type::Object
                });
            } else {
                if mode.mode() != ArgumentMode::Value {
                    signature.write_u8(0x10); // ELEMENT_TYPE_BYREF
                }
                metadata.write_type(&mut signature, ty);
                raw.parameters.push(ty);
            }
            modes.push(mode);
            if let ParameterPassing::Buffer(length, _) = mode {
                let length = match length {
                    BufferLength::Native => Type::UIntPtr,
                    BufferLength::U32 => Type::U32,
                    BufferLength::U64 => Type::U64,
                };
                metadata.write_type(&mut signature, length);
                raw.parameters.push(length);
                modes.push(ParameterPassing::Value);
            }
        }
        let param_list = metadata.add_parameters(&raw, &modes, &[]);
        metadata.methods.push(MethodDefRow {
            rva: 0,
            impl_flags: 0x0080, // PreserveSig
            flags: 0x2093,      // PInvokeImpl, static, hidebysig, assembly access
            name: metadata
                .strings
                .add(&format!("<native_{}>", self.raw_method)),
            signature: metadata.blobs.add(signature.as_slice()),
            param_list,
        });
    }
}

impl Callback<'_> {
    fn add_type(&self, metadata: &mut Tables<'_>, delegate: u32, attr_ctor: u32) {
        debug_assert_eq!(self.ty as usize, metadata.types.len() + 1);
        debug_assert_eq!(self.ctor as usize, metadata.methods.len() + 1);
        metadata.types.push(TypeDefRow {
            flags: 0x0100, // sealed, assembly visibility
            type_name: metadata
                .strings
                .add(&format!("<native_callback_{}>", self.ty)),
            type_namespace: 0,
            extends: index(TableId::TypeRef, delegate),
            field_list: metadata.fields.len() as u32 + 1,
            method_list: self.ctor,
        });
        metadata.custom_attributes.push(CustomAttributeRow {
            parent: index(TableId::TypeDef, self.ty),
            attr_type: index(TableId::MemberRef, attr_ctor),
            value: metadata.blobs.add(&[1, 0, 2, 0, 0, 0, 0, 0]), // prolog, CallingConvention.Cdecl, no named arguments
        });
        let ctor = metadata.method_signature(
            &[Type::Object, Type::IntPtr],
            Type::Void,
            Invocation::Constructor,
            0,
        );
        let invoke = &self.descriptor.signature;
        let invoke =
            metadata.method_signature(&invoke.parameters, invoke.result, Invocation::Instance, 0);
        for (name, flags, signature) in [(".ctor", 0x1886, ctor), ("Invoke", 0x01c6, invoke)] {
            metadata.methods.push(MethodDefRow {
                rva: 0,
                impl_flags: 0x0003, // Runtime managed
                flags,
                name: metadata.strings.add(name),
                signature,
                param_list: metadata.parameters.len() as u32 + 1,
            });
        }
    }
}

impl Tables<'_> {
    fn native_type_ref(&mut self, assembly: u32, namespace: &str, name: &str) -> u32 {
        self.type_refs.push(TypeRefRow {
            resolution_scope: index(TableId::AssemblyRef, assembly),
            type_name: self.strings.add(name),
            type_namespace: self.strings.add(namespace),
        });
        self.type_refs.len() as u32
    }

    fn native_interop_reference(&mut self) -> u32 {
        let name = "System.Runtime.InteropServices";
        if let Some((id, assembly)) = self
            .externals
            .assemblies()
            .iter()
            .enumerate()
            .find(|(_, assembly)| assembly.name == name)
        {
            return *self.assembly_indices.entry(id).or_insert_with(|| {
                self.references.push(assembly_reference(
                    assembly,
                    &mut self.strings,
                    &mut self.blobs,
                ));
                self.references.len() as u32
            });
        }
        self.references.push(AssemblyRefRow {
            major_version: 4,
            name: self.strings.add(name),
            public_key_or_token: self
                .blobs
                .add(&[0xb0, 0x3f, 0x5f, 0x7f, 0x11, 0xd5, 0x0a, 0x3a]),
            ..Default::default()
        });
        self.references.len() as u32
    }
}

//! Emits CLR interface slots and explicit value-type implementations.

use super::*;

pub(super) fn method_count(definition: &Struct<'_>) -> usize {
    definition.interface.as_ref().map_or(0, Vec::len)
        + definition
            .implementations
            .iter()
            .map(|implementation| implementation.methods.len())
            .sum::<usize>()
}

impl Tables<'_> {
    pub(super) fn interface_type(&mut self, ty: Type) -> CodedIndex {
        if let Type::External(id) = ty {
            return index(TableId::TypeRef, self.type_reference(id));
        }
        if matches!(ty, Type::ConstructedExternal(_)) {
            return index(TableId::TypeSpec, self.type_spec(ty));
        }
        let Type::Interface(id) = ty else {
            unreachable!("checked interface")
        };
        let instance = &self.instances[id];
        let InstanceKind::Interface(id) = instance.kind else {
            unreachable!("source interface instance")
        };
        if instance.arguments.is_empty() {
            index(TableId::TypeDef, self.struct_row(id))
        } else {
            index(TableId::TypeSpec, self.type_spec(ty))
        }
    }

    pub(in crate::cil) fn interface_member(&mut self, member: InterfaceMember) -> u32 {
        if let Some(&row) = self.interface_members.get(&member) {
            return MEMBER_REF | row;
        }
        let Type::Interface(id) = member.owner else {
            unreachable!("checked interface")
        };
        let InstanceKind::Interface(definition) = self.instances[id].kind else {
            unreachable!("source interface instance")
        };
        let method = &self.source_types[definition]
            .interface
            .as_ref()
            .expect("interface methods")[member.slot];
        let signature = self.method_signature(
            &method.signature.parameters,
            method.signature.result,
            Invocation::Instance,
            0,
        );
        let owner = self.interface_type(member.owner);
        let row = self.push_member_for(owner, method.name, signature);
        self.interface_members.insert(member, row);
        MEMBER_REF | row
    }

    pub(super) fn add_interfaces(
        &mut self,
        definitions: &[Struct<'_>],
        text: &mut Writer,
    ) -> Result<(), Diagnostic> {
        for (id, definition) in definitions.iter().enumerate() {
            if let Some(methods) = &definition.interface {
                for method in methods {
                    self.add_interface_method(
                        method.name,
                        &method.signature,
                        &method.parameter_names,
                        0,
                        0x05c6,
                    );
                }
            }
            for (implementation_id, implementation) in definition.implementations.iter().enumerate()
            {
                let class = self.struct_row(id);
                let interface = self.interface_type(implementation.ty);
                self.interface_impls
                    .push(InterfaceImplRow { class, interface });
                for (slot, method) in implementation.methods.iter().enumerate() {
                    let signature = &method.signature;
                    let declaration = match method.declaration {
                        InterfaceDeclaration::Source(member) => {
                            self.interface_member(member) & 0x00ff_ffff
                        }
                        InterfaceDeclaration::Clr(member) => self.member(member.id, member.owner).1,
                    };
                    let method_body = self.methods.len() as u32 + 1;
                    self.method_impls.push(MethodImplRow {
                        class,
                        method_body: index(TableId::MethodDef, method_body),
                        method_declaration: index(TableId::MemberRef, declaration),
                    });
                    text.align(4);
                    let rva = TEXT_RVA + CLI_HEADER_SIZE + image_offset(text.len())?;
                    let empty = Default::default();
                    let mut body = Body::new(self, 0, &[], &empty, false);
                    body.max_stack = signature.parameters.len() + 1;
                    body.op(0x02); // ldarg.0 (managed pointer to the value receiver)
                    let receiver = body.metadata.runtime_type_token(implementation.receiver);
                    body.token_instruction(0x71, receiver); // ldobj (read-only self by value)
                    for position in 1..=signature.parameters.len() {
                        body.indexed(0x0e, 0x09, position as u16); // ldarg
                    }
                    let arguments: Vec<_> = (0..definition.type_parameters.len())
                        .map(|index| Type::TypeParameter(index as u16))
                        .collect();
                    let method = body.metadata.instantiate_method(
                        body.metadata.source_method_token(method.helper),
                        &arguments,
                    );
                    body.call(method);
                    body.op(0x2a); // ret
                    body.write(text, definition.span)?;
                    // Explicit implementation lets different interfaces use the same method name.
                    let name = format!("<interface_{implementation_id}_{slot}>");
                    self.add_interface_method(&name, signature, &[], rva, 0x01e1);
                }
            }
        }
        Ok(())
    }

    fn add_interface_method(
        &mut self,
        name: &str,
        signature: &Signature,
        names: &[Option<Box<str>>],
        rva: u32,
        flags: u16,
    ) {
        let param_list = self.add_parameters(signature, &[], names);
        let signature = self.method_signature(
            &signature.parameters,
            signature.result,
            Invocation::Instance,
            0,
        );
        self.methods.push(MethodDefRow {
            rva,
            impl_flags: 0,
            flags,
            name: self.strings.add(name),
            signature,
            param_list,
        });
    }
}

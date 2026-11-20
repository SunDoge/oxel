use super::*;

impl NativeTypes<'_> {
    pub(super) fn address(&mut self, pointer: &syn::TypePtr) -> syn::Result<String> {
        let ty = pointer.elem.as_ref();
        if is_c_void(ty) || utf8_pointee(ty) || matches!(ty, Type::Ptr(_)) {
            return Ok("nint".into());
        }
        self.by_value(ty).map_err(|_| {
            syn::Error::new(
                pointer.span(),
                "expected a thin address to a supported ABI type or fully qualified core::ffi::c_void",
            )
        })?;
        Ok("nint".into())
    }

    pub(super) fn buffer(
        &mut self,
        ty: &Type,
        length: &Type,
        write: bool,
        span: RustSpan,
    ) -> syn::Result<(&'static str, String)> {
        let Type::Ptr(ptr) = ty else {
            return Err(syn::Error::new(
                span,
                "a buffer annotation requires a raw pointer parameter",
            ));
        };
        if write != ptr.mutability.is_some() {
            return Err(syn::Error::new(
                span,
                "oxel: buffer requires *const T and oxel: buffer_mut requires *mut T",
            ));
        }
        let prefix = match (simple_type(length).as_deref(), write) {
            (Some("usize"), false) => "buffer ",
            (Some("u32"), false) => "buffer32 ",
            (Some("u64"), false) => "buffer64 ",
            (Some("usize"), true) => "buffer_mut ",
            (Some("u32"), true) => "buffer_mut32 ",
            (Some("u64"), true) => "buffer_mut64 ",
            _ => {
                return Err(syn::Error::new(
                    length.span(),
                    "a Rust buffer length must be usize, u32 or u64",
                ));
            }
        };
        Ok((prefix, format!("{}[]", self.by_value(&ptr.elem)?)))
    }

    pub(super) fn callback(&mut self, ty: &Type) -> syn::Result<String> {
        let nullable = option_argument(ty);
        let target = nullable.unwrap_or(ty);
        let function = if let Type::BareFn(function) = target {
            function
        } else if let Some(alias) = simple_type(target).and_then(|name| self.aliases.get(&name)) {
            check_attributes(&alias.attrs, &[])?;
            let Type::BareFn(function) = alias.ty.as_ref() else {
                return Err(callback_error(target));
            };
            if !alias.generics.params.is_empty() {
                return Err(callback_error(target));
            }
            function
        } else {
            return Err(callback_error(target));
        };
        if !function
            .abi
            .as_ref()
            .is_some_and(|abi| abi.name.as_ref().is_none_or(|name| name.value() == "C"))
            || function.variadic.is_some()
            || function.lifetimes.is_some()
            || function.inputs.len() > 16
        {
            return Err(callback_error(target));
        }
        let mut arguments = function
            .inputs
            .iter()
            .map(|arg| {
                check_attributes(&arg.attrs, &[])?;
                self.callback_value(&arg.ty)
            })
            .collect::<syn::Result<Vec<_>>>()?;
        let name = if let ReturnType::Type(_, result) = &function.output
            && !matches!(result.as_ref(), Type::Tuple(tuple) if tuple.elems.is_empty())
        {
            arguments.push(self.callback_value(result)?);
            "Func"
        } else {
            "Action"
        };
        let mut output = format!("System.{name}");
        if !arguments.is_empty() {
            write!(output, "<{}>", arguments.join(", ")).unwrap();
        }
        if nullable.is_some() {
            output.push('?');
        }
        Ok(output)
    }

    fn callback_value(&mut self, ty: &Type) -> syn::Result<String> {
        if let Type::Ptr(pointer) = ty {
            self.address(pointer)
        } else {
            self.by_value(ty)
        }
    }
}

fn is_c_void(ty: &Type) -> bool {
    let Type::Path(path) = ty else { return false };
    if path.qself.is_some()
        || path
            .path
            .segments
            .iter()
            .any(|part| !part.arguments.is_empty())
    {
        return false;
    }
    let names: Vec<_> = path.path.segments.iter().collect();
    matches!(names.as_slice(), [root, module, ty]
        if (root.ident == "core" || root.ident == "std") && module.ident == "ffi" && ty.ident == "c_void")
}

fn option_argument(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else { return None };
    if path.qself.is_some() {
        return None;
    }
    let segments: Vec<_> = path.path.segments.iter().collect();
    let option = match segments.as_slice() {
        [option] => option,
        [root, module, option]
            if (root.ident == "core" || root.ident == "std")
                && module.ident == "option"
                && root.arguments.is_empty()
                && module.arguments.is_empty() =>
        {
            option
        }
        _ => return None,
    };
    if option.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &option.arguments else {
        return None;
    };
    if args.args.len() != 1 {
        return None;
    }
    match args.args.first()? {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    }
}

fn callback_error(ty: &Type) -> syn::Error {
    syn::Error::new(
        ty.span(),
        "a callback requires an extern \"C\" function pointer (or Option of one) with at most 16 parameters",
    )
}

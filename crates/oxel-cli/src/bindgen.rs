//! Generate Oxel declarations for the explicit C ABI of a Rust source file.

use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs,
    io::{self, Write as _},
    process::ExitCode,
};

use oxel::{Diagnostic, Span};
use proc_macro2::Span as RustSpan;
use syn::{
    Attribute, Expr, ExprLit, Fields, FnArg, Item, ItemFn, ItemStruct, Lit, Meta, Pat, ReturnType,
    Type, UseTree, Visibility, ext::IdentExt, spanned::Spanned,
};

use crate::{arguments::BindgenInputs, diagnostics::Format};

const ERROR: &str = "E0011";

pub(super) fn run(inputs: BindgenInputs, format: Format) -> ExitCode {
    let BindgenInputs {
        source,
        library,
        output,
    } = inputs;
    if library.is_empty() || library.contains('\0') {
        return crate::arguments::usage_error(
            format,
            "a native library name must be nonempty and contain no NUL",
        );
    }

    let rust = match fs::read_to_string(&source) {
        Ok(rust) => rust,
        Err(error) => {
            return format.error(
                &source,
                ERROR,
                format!("could not read Rust ABI source: {error}"),
            );
        }
    };
    let abi = rust.strip_prefix('\u{feff}').unwrap_or(&rust);
    let bindings = match generate(abi, &library) {
        Ok(bindings) => bindings,
        Err(error) => {
            let range = error.span().byte_range();
            let bom = rust.len() - abi.len();
            let span = Span {
                start: range.start + bom,
                end: range.end + bom,
            };
            format.print(
                &source,
                &rust,
                &Diagnostic::new(ERROR, span, error.to_string()),
            );
            return ExitCode::FAILURE;
        }
    };

    if let Some(path) = &output {
        if path.file_name().is_none() {
            return format.error(path, ERROR, "the binding output must name a file");
        }
        if fs::canonicalize(path)
            .ok()
            .is_some_and(|path| fs::canonicalize(&source).is_ok_and(|source| path == source))
        {
            return format.error(
                path,
                ERROR,
                "the binding output cannot overwrite the Rust ABI source",
            );
        }
    }
    let result = match &output {
        Some(path) => crate::write_file(path, bindings.as_bytes()),
        None => io::stdout().lock().write_all(bindings.as_bytes()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => format.error(
            output.as_deref().unwrap_or(&source),
            ERROR,
            format!("could not write Oxel bindings: {error}"),
        ),
    }
}

fn generate(rust: &str, library: &str) -> syn::Result<String> {
    let file = syn::parse_str::<syn::File>(rust)?;
    reject_conditional(&file.attrs)?;
    for item in &file.items {
        let name = match item {
            Item::Enum(item) => &item.ident,
            Item::ExternCrate(item) => item.rename.as_ref().map_or(&item.ident, |(_, name)| name),
            Item::Mod(item) => &item.ident,
            Item::Struct(item) => &item.ident,
            Item::Trait(item) => &item.ident,
            Item::TraitAlias(item) => &item.ident,
            Item::Type(item) => &item.ident,
            Item::Union(item) => &item.ident,
            Item::Use(item) => {
                check_import(&item.tree, None)?;
                continue;
            }
            _ => continue,
        };
        check_primitive_name(name)?;
    }
    let structs: Vec<_> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Struct(item) => Some(item),
            _ => None,
        })
        .collect();
    let mut definitions = HashMap::new();
    for &item in &structs {
        let name = item.ident.unraw().to_string();
        if definitions.insert(name.clone(), item).is_some() {
            return Err(syn::Error::new(
                item.ident.span(),
                format!("multiple Rust declarations of '{name}'"),
            ));
        }
    }

    let mut types = NativeTypes {
        definitions,
        used: HashSet::new(),
        pending: Vec::new(),
    };
    let library = string_literal(library);
    let mut functions = String::new();
    let mut names = HashSet::new();
    let mut symbols = HashSet::new();
    for item in &file.items {
        let Item::Fn(function) = item else { continue };
        let symbol = foreign_symbol(&function.attrs, &function.sig.ident)?;
        if symbol.is_none() && !has_conditional_export(&function.attrs)? {
            continue;
        }
        let annotations = Annotations::read(&function.attrs)?;
        if annotations.skip {
            continue;
        }
        reject_conditional(&function.attrs)?;
        let Some(symbol) = symbol else { continue };
        let name = annotations.name.as_ref().map_or_else(
            || function.sig.ident.unraw().to_string(),
            |(name, _)| name.clone(),
        );
        let span = annotations
            .name
            .as_ref()
            .map_or(function.sig.ident.span(), |(_, span)| *span);
        check_name(&name, span)?;
        if !names.insert(name.clone()) {
            return Err(syn::Error::new(
                span,
                format!("duplicate Oxel binding name '{name}'"),
            ));
        }
        if !symbols.insert(symbol.clone()) {
            return Err(syn::Error::new(
                span,
                format!("duplicate native symbol '{symbol}'"),
            ));
        }
        write_function(
            &mut functions,
            function,
            &name,
            &symbol,
            &library,
            annotations,
            &mut types,
        )?;
    }
    if functions.is_empty() {
        return Err(syn::Error::new(
            RustSpan::call_site(),
            "no Rust C ABI exports found",
        ));
    }

    let mut layouts = HashMap::new();
    while let Some(name) = types.pending.pop() {
        let definition = types.definitions[&name];
        layouts.insert(name.clone(), write_struct(&name, definition, &mut types)?);
    }
    let mut output =
        String::from("// Generated by oxel bindgen. Edit the Rust ABI source and regenerate.\n\n");
    for item in structs {
        if let Some(layout) = layouts.remove(&item.ident.unraw().to_string()) {
            output.push_str(&layout);
        }
    }
    output.push_str(&functions);
    Ok(output)
}

fn foreign_symbol(attrs: &[Attribute], ident: &syn::Ident) -> syn::Result<Option<String>> {
    let mut symbol = None;
    for attr in attrs {
        let meta = if attr.path().is_ident("unsafe") {
            attr.parse_args::<Meta>()?
        } else {
            attr.meta.clone()
        };
        let name = if meta.path().is_ident("no_mangle") {
            if !matches!(meta, Meta::Path(_)) {
                return Err(syn::Error::new(
                    meta.span(),
                    "expected a bare no_mangle attribute",
                ));
            }
            ident.unraw().to_string()
        } else if meta.path().is_ident("export_name") {
            let name = attribute_string(&meta)?.value();
            if name.is_empty() || name.contains('\0') {
                return Err(syn::Error::new(
                    meta.span(),
                    "a native symbol must be nonempty and contain no NUL",
                ));
            }
            name
        } else {
            continue;
        };
        if symbol.replace(name).is_some() {
            return Err(syn::Error::new(
                attr.span(),
                "use a single Rust symbol export attribute",
            ));
        }
    }
    Ok(symbol)
}

fn has_conditional_export(attrs: &[Attribute]) -> syn::Result<bool> {
    fn exports(meta: &Meta) -> syn::Result<bool> {
        if meta.path().is_ident("cfg_attr") {
            let Meta::List(list) = meta else {
                return Err(syn::Error::new(
                    meta.span(),
                    "expected a cfg_attr argument list",
                ));
            };
            let args = list.parse_args_with(
                syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
            )?;
            for attribute in args.iter().skip(1) {
                if exports(attribute)? {
                    return Ok(true);
                }
            }
            return Ok(false);
        }
        if let Meta::List(list) = meta
            && meta.path().is_ident("unsafe")
        {
            return exports(&list.parse_args()?);
        }
        Ok(meta.path().is_ident("no_mangle") || meta.path().is_ident("export_name"))
    }

    for attr in attrs.iter().filter(|attr| attr.path().is_ident("cfg_attr")) {
        if exports(&attr.meta)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn attribute_string(meta: &Meta) -> syn::Result<&syn::LitStr> {
    if let Meta::NameValue(value) = meta
        && let Expr::Lit(ExprLit {
            lit: Lit::Str(value),
            ..
        }) = &value.value
    {
        return Ok(value);
    }
    Err(syn::Error::new(
        meta.span(),
        "expected an attribute with a string value",
    ))
}

#[derive(Clone, Copy)]
enum Passing {
    Ref,
    Out,
    Utf8,
    Utf8Nullable,
}

#[derive(Default)]
struct Annotations {
    skip: bool,
    name: Option<(String, RustSpan)>,
    parameters: HashMap<String, (Passing, RustSpan)>,
}

impl Annotations {
    fn read(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut annotations = Self::default();
        for attr in attrs.iter().filter(|attr| attr.path().is_ident("doc")) {
            let Ok(doc) = attribute_string(&attr.meta) else {
                continue;
            };
            let value = doc.value();
            let Some(hint) = value.trim().strip_prefix("oxel:") else {
                continue;
            };
            let words: Vec<_> = hint.split_whitespace().collect();
            let span = doc.span();
            match words.as_slice() {
                ["skip"] if !annotations.skip => annotations.skip = true,
                ["rename", name] if annotations.name.is_none() => {
                    annotations.name = Some(((*name).into(), span));
                }
                [mode @ ("ref" | "out" | "utf8" | "utf8_nullable"), parameter] => {
                    let passing = match *mode {
                        "ref" => Passing::Ref,
                        "out" => Passing::Out,
                        "utf8" => Passing::Utf8,
                        _ => Passing::Utf8Nullable,
                    };
                    if annotations
                        .parameters
                        .insert((*parameter).into(), (passing, span))
                        .is_some()
                    {
                        return Err(syn::Error::new(
                            span,
                            format!("duplicate Oxel annotation for parameter '{parameter}'"),
                        ));
                    }
                }
                _ => {
                    return Err(syn::Error::new(
                        span,
                        "expected 'oxel: skip', 'oxel: rename NAME', or 'oxel: MODE PARAMETER' with ref, out, utf8 or utf8_nullable",
                    ));
                }
            }
        }
        Ok(annotations)
    }
}

struct NativeTypes<'rust> {
    definitions: HashMap<String, &'rust ItemStruct>,
    used: HashSet<String>,
    pending: Vec<String>,
}

impl NativeTypes<'_> {
    fn by_value(&mut self, ty: &Type) -> syn::Result<String> {
        if let Some(name) = simple_type(ty) {
            if numeric(&name) {
                return Ok(name);
            }
            if self.definitions.contains_key(&name) {
                if self.used.insert(name.clone()) {
                    self.pending.push(name.clone());
                }
                return Ok(name);
            }
        }
        Err(syn::Error::new(
            ty.span(),
            "expected a fixed-width number or a #[repr(C)] struct from this Rust file",
        ))
    }

    fn parameter(
        &mut self,
        ty: &Type,
        annotation: Option<(Passing, RustSpan)>,
    ) -> syn::Result<(&'static str, String)> {
        let Some((passing, span)) = annotation else {
            if matches!(ty, Type::Ptr(_)) {
                return Err(syn::Error::new(
                    ty.span(),
                    "a Rust pointer parameter needs an explicit Oxel annotation (ref, out, utf8 or utf8_nullable)",
                ));
            }
            return Ok(("", self.by_value(ty)?));
        };
        match (passing, ty) {
            (Passing::Ref | Passing::Out, Type::Ptr(ptr)) if ptr.mutability.is_some() => {
                let prefix = if matches!(passing, Passing::Ref) {
                    "ref "
                } else {
                    "out "
                };
                Ok((prefix, self.by_value(&ptr.elem)?))
            }
            (Passing::Utf8 | Passing::Utf8Nullable, Type::Ptr(ptr))
                if ptr.const_token.is_some() && utf8_pointee(&ptr.elem) =>
            {
                let ty = if matches!(passing, Passing::Utf8) {
                    "string"
                } else {
                    "string?"
                };
                Ok(("utf8 ", ty.into()))
            }
            (Passing::Ref | Passing::Out, _) => Err(syn::Error::new(
                span,
                "oxel: ref and oxel: out require a *mut parameter",
            )),
            (Passing::Utf8 | Passing::Utf8Nullable, _) => Err(syn::Error::new(
                span,
                "oxel: utf8 and oxel: utf8_nullable require *const u8, *const core::ffi::c_char or *const std::ffi::c_char",
            )),
        }
    }
}

fn simple_type(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else { return None };
    if path.qself.is_some() {
        return None;
    }
    path.path.get_ident().map(|ident| ident.unraw().to_string())
}

fn numeric(name: &str) -> bool {
    matches!(
        name,
        "i8" | "u8" | "i16" | "u16" | "i32" | "u32" | "i64" | "u64" | "f32" | "f64"
    )
}

fn check_primitive_name(ident: &syn::Ident) -> syn::Result<()> {
    if numeric(&ident.unraw().to_string()) {
        return Err(syn::Error::new(
            ident.span(),
            "a Rust ABI source file cannot shadow a fixed-width primitive type",
        ));
    }
    Ok(())
}

fn check_import(tree: &UseTree, parent: Option<&syn::Ident>) -> syn::Result<()> {
    match tree {
        UseTree::Path(path) => check_import(&path.tree, Some(&path.ident)),
        UseTree::Name(name) if name.ident == "self" => parent.map_or(Ok(()), check_primitive_name),
        UseTree::Name(name) => check_primitive_name(&name.ident),
        UseTree::Rename(rename) => check_primitive_name(&rename.rename),
        UseTree::Group(group) => group
            .items
            .iter()
            .try_for_each(|item| check_import(item, parent)),
        UseTree::Glob(glob) => Err(syn::Error::new(
            glob.span(),
            "use explicit imports in the Rust ABI source file so primitive types can be resolved",
        )),
    }
}

fn utf8_pointee(ty: &Type) -> bool {
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
    let segments: Vec<_> = path
        .path
        .segments
        .iter()
        .map(|part| part.ident.to_string())
        .collect();
    match segments.as_slice() {
        [name] => name == "u8",
        [root, module, name] => {
            (root == "std" || root == "core") && module == "ffi" && name == "c_char"
        }
        _ => false,
    }
}

fn write_struct(name: &str, item: &ItemStruct, types: &mut NativeTypes<'_>) -> syn::Result<String> {
    check_attributes(&item.attrs, &["repr", "derive", "non_exhaustive"])?;
    check_name(name, item.ident.span())?;
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new(
            item.generics.span(),
            "an Oxel ABI struct cannot be generic",
        ));
    }
    let mut c_repr = false;
    for attr in item
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("repr"))
    {
        let repr: syn::Ident = attr.parse_args().map_err(|_| {
            syn::Error::new(
                attr.span(),
                "expected #[repr(C)] with default layout and alignment",
            )
        })?;
        if repr != "C" || c_repr {
            return Err(syn::Error::new(
                attr.span(),
                "expected #[repr(C)] with default layout and alignment",
            ));
        }
        c_repr = true;
    }
    if !c_repr {
        return Err(syn::Error::new(
            item.ident.span(),
            "ABI structs require #[repr(C)]",
        ));
    }
    let Fields::Named(fields) = &item.fields else {
        return Err(syn::Error::new(
            item.fields.span(),
            "ABI structs require named fields",
        ));
    };
    if fields.named.is_empty() {
        return Err(syn::Error::new(
            fields.span(),
            "ABI structs must be nonempty",
        ));
    }
    let mut declaration = format!("struct {name} {{\n");
    for field in &fields.named {
        check_attributes(&field.attrs, &[])?;
        let ident = field.ident.as_ref().expect("a named struct field");
        let name = ident.unraw().to_string();
        check_name(&name, ident.span())?;
        let ty = types.by_value(&field.ty)?;
        writeln!(declaration, "    {name}: {ty},").unwrap();
    }
    declaration.push_str("}\n\n");
    Ok(declaration)
}

fn write_function(
    output: &mut String,
    function: &ItemFn,
    name: &str,
    symbol: &str,
    library: &str,
    mut annotations: Annotations,
    types: &mut NativeTypes<'_>,
) -> syn::Result<()> {
    check_attributes(
        &function.attrs,
        &[
            "no_mangle",
            "export_name",
            "unsafe",
            "inline",
            "cold",
            "must_use",
        ],
    )?;
    let sig = &function.sig;
    let c_abi = sig
        .abi
        .as_ref()
        .is_some_and(|abi| abi.name.as_ref().is_none_or(|name| name.value() == "C"));
    if !matches!(function.vis, Visibility::Public(_))
        || !c_abi
        || sig.asyncness.is_some()
        || !sig.generics.params.is_empty()
        || sig.variadic.is_some()
    {
        return Err(syn::Error::new(
            sig.span(),
            "binding exports require a public nongeneric, nonvariadic extern \"C\" function",
        ));
    }
    write!(output, "extern \"C\" fn {name}(").unwrap();
    for (index, input) in sig.inputs.iter().enumerate() {
        let FnArg::Typed(parameter) = input else {
            return Err(syn::Error::new(
                input.span(),
                "expected a named ABI parameter",
            ));
        };
        check_attributes(&parameter.attrs, &[])?;
        let Pat::Ident(parameter_name) = parameter.pat.as_ref() else {
            return Err(syn::Error::new(
                parameter.pat.span(),
                "expected a named ABI parameter",
            ));
        };
        if parameter_name.by_ref.is_some() || parameter_name.subpat.is_some() {
            return Err(syn::Error::new(
                parameter_name.span(),
                "expected a named ABI parameter",
            ));
        }
        let parameter_name = parameter_name.ident.unraw().to_string();
        check_name(&parameter_name, parameter.pat.span())?;
        let annotation = annotations.parameters.remove(&parameter_name);
        let (passing, ty) = types.parameter(&parameter.ty, annotation)?;
        if index != 0 {
            output.push_str(", ");
        }
        write!(output, "{passing}{parameter_name}: {ty}").unwrap();
    }
    if let Some((name, (_, span))) = annotations
        .parameters
        .iter()
        .min_by_key(|(_, (_, span))| span.byte_range().start)
    {
        return Err(syn::Error::new(
            *span,
            format!("no ABI parameter named '{name}'"),
        ));
    }
    output.push(')');
    if let ReturnType::Type(_, ty) = &sig.output
        && !matches!(ty.as_ref(), Type::Tuple(tuple) if tuple.elems.is_empty())
    {
        write!(output, " -> {}", types.by_value(ty)?).unwrap();
    }
    write!(output, " = {library}").unwrap();
    if name != symbol {
        write!(output, " as {}", string_literal(symbol)).unwrap();
    }
    output.push_str(";\n");
    Ok(())
}

fn reject_conditional(attrs: &[Attribute]) -> syn::Result<()> {
    for attr in attrs {
        if attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr") {
            return Err(syn::Error::new(
                attr.span(),
                "conditional Rust ABI declarations are unsupported; supply a target-specific ABI source file",
            ));
        }
    }
    Ok(())
}

fn check_attributes(attrs: &[Attribute], allowed: &[&str]) -> syn::Result<()> {
    reject_conditional(attrs)?;
    for attr in attrs {
        let path = attr.path();
        if [
            "doc",
            "allow",
            "warn",
            "deny",
            "forbid",
            "expect",
            "deprecated",
        ]
        .iter()
        .chain(allowed)
        .any(|name| path.is_ident(name))
        {
            continue;
        }
        return Err(syn::Error::new(
            attr.span(),
            "unsupported attribute on a Rust ABI declaration",
        ));
    }
    Ok(())
}

fn check_name(name: &str, span: RustSpan) -> syn::Result<()> {
    if !oxel::valid_identifier(name) || name == "_" {
        return Err(syn::Error::new(
            span,
            format!("'{name}' is not a usable Oxel binding identifier"),
        ));
    }
    Ok(())
}

fn string_literal(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

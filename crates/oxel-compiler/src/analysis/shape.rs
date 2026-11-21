//! Fingerprint declarations from their parsed shape, without trivia or locations.

use blake3::{Hash, Hasher};

use crate::syntax::{Bound, Function, Path, TypeBound, TypeDefinition, TypeShape, TypeSuffix};

pub(super) fn file_scope(hash: &mut Hasher, scope: &crate::syntax::FileScope<'_>) {
    if let Some(namespace) = &scope.namespace {
        hash.update(&[1]);
        path(hash, namespace);
    } else {
        hash.update(&[0]);
    }
    length(hash, scope.imports.len());
    for import in &scope.imports {
        path(hash, &import.path);
        name(hash, import.alias.0);
    }
}

pub(super) fn signature(function: &Function<'_>) -> Hash {
    let mut hash = Hasher::new();
    hash.update(&[
        u8::from(function.comptime),
        u8::from(function.async_fn),
        u8::from(function.internal),
        u8::from(function.body.managed().is_none()),
        u8::from(function.test.is_some()),
    ]);
    name(&mut hash, function.name);
    generics(&mut hash, &function.type_parameters, &function.type_bounds);
    length(&mut hash, function.parameters.len());
    for parameter in &function.parameters {
        name(&mut hash, parameter.passing.prefix());
        name(&mut hash, parameter.name);
        path(&mut hash, &parameter.annotation);
    }
    if let Some(result) = &function.result {
        hash.update(&[1]);
        path(&mut hash, result);
    } else {
        hash.update(&[0]);
    }
    hash.finalize()
}

pub(super) fn declared_type(definition: &TypeDefinition<'_>) -> Hash {
    let mut hash = Hasher::new();
    hash.update(&[u8::from(definition.internal)]);
    name(&mut hash, definition.name);
    generics(
        &mut hash,
        &definition.type_parameters,
        &definition.type_bounds,
    );
    match &definition.shape {
        TypeShape::Struct(fields) => {
            hash.update(&[0]);
            length(&mut hash, fields.len());
            for (field, _, annotation) in fields {
                name(&mut hash, field);
                path(&mut hash, annotation);
            }
        }
        TypeShape::Union(variants) => {
            hash.update(&[1]);
            length(&mut hash, variants.len());
            for variant in variants {
                name(&mut hash, variant.name);
                paths(&mut hash, &variant.payload);
            }
        }
        TypeShape::Interface(methods) => {
            hash.update(&[2]);
            length(&mut hash, methods.len());
            for method in methods {
                name(&mut hash, method.name);
                length(&mut hash, method.parameters.len());
                for parameter in &method.parameters {
                    name(&mut hash, parameter.name);
                    path(&mut hash, &parameter.annotation);
                }
                length(&mut hash, usize::from(method.result.is_some()));
                if let Some(result) = &method.result {
                    path(&mut hash, result);
                }
            }
        }
    }
    hash.finalize()
}

pub(super) fn implementation(
    hash: &mut Hasher,
    implementation: &crate::syntax::Implementation<'_>,
) {
    generics(
        hash,
        &implementation.type_parameters,
        &implementation.type_bounds,
    );
    path(hash, &implementation.interface);
    path(hash, &implementation.target);
    length(hash, implementation.methods.len());
    for method in &implementation.methods {
        hash.update(signature(method).as_bytes());
    }
}

pub(crate) fn type_names<'src>(annotation: &Path<'src>, visit: &mut impl FnMut(&'src str)) {
    if let [(name, _)] = annotation.segments.as_slice() {
        visit(name);
    }
    for argument in &annotation.type_arguments {
        type_names(argument, visit);
    }
}

pub(crate) fn signature_type_names<'src>(
    function: &Function<'src>,
    mut visit: impl FnMut(&'src str),
) {
    for parameter in &function.parameters {
        type_names(&parameter.annotation, &mut visit);
    }
    if let Some(result) = &function.result {
        type_names(result, &mut visit);
    }
    bound_names(&function.type_bounds, &mut visit);
}

pub(super) fn declared_type_names<'src>(
    definition: &TypeDefinition<'src>,
    mut visit: impl FnMut(&'src str),
) {
    bound_names(&definition.type_bounds, &mut visit);
    match &definition.shape {
        TypeShape::Struct(fields) => {
            for (_, _, annotation) in fields {
                type_names(annotation, &mut visit);
            }
        }
        TypeShape::Union(variants) => {
            for variant in variants {
                for annotation in &variant.payload {
                    type_names(annotation, &mut visit);
                }
            }
        }
        TypeShape::Interface(methods) => {
            for method in methods {
                for parameter in &method.parameters {
                    type_names(&parameter.annotation, &mut visit);
                }
                if let Some(result) = &method.result {
                    type_names(result, &mut visit);
                }
            }
        }
    }
}

fn bound_names<'src>(bounds: &[TypeBound<'src>], visit: &mut impl FnMut(&'src str)) {
    for bound in bounds {
        if let Bound::Type(annotation) = &bound.constraint {
            type_names(annotation, visit);
        }
    }
}

fn generics(hash: &mut Hasher, parameters: &[(&str, crate::Span)], bounds: &[TypeBound<'_>]) {
    length(hash, parameters.len());
    for (parameter, _) in parameters {
        name(hash, parameter);
    }
    length(hash, bounds.len());
    for bound in bounds {
        length(hash, bound.parameter);
        match &bound.constraint {
            Bound::Type(annotation) => {
                hash.update(&[0]);
                path(hash, annotation);
            }
            Bound::Reference => {
                hash.update(&[1]);
            }
            Bound::Value => {
                hash.update(&[2]);
            }
            Bound::Constructor => {
                hash.update(&[3]);
            }
        }
    }
}

fn path(hash: &mut Hasher, annotation: &Path<'_>) {
    length(hash, annotation.segments.len());
    for (segment, _) in &annotation.segments {
        name(hash, segment);
    }
    paths(hash, &annotation.type_arguments);
    length(hash, annotation.suffixes.len());
    for suffix in &annotation.suffixes {
        hash.update(&[match suffix {
            TypeSuffix::Nullable(_) => 0,
            TypeSuffix::Array(_) => 1,
        }]);
    }
}

fn paths(hash: &mut Hasher, annotations: &[Path<'_>]) {
    length(hash, annotations.len());
    for annotation in annotations {
        path(hash, annotation);
    }
}

fn name(hash: &mut Hasher, name: &str) {
    length(hash, name.len());
    hash.update(name.as_bytes());
}

fn length(hash: &mut Hasher, length: usize) {
    hash.update(&(length as u64).to_le_bytes());
}

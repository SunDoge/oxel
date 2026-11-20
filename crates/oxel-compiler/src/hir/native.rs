use super::*;

pub(crate) struct NativeCallback {
    pub invoke: Member,
    pub signature: Signature,
}

pub(super) fn callback(types: &Structs<'_, '_>, ty: Type, span: Span) -> Checked<NativeCallback> {
    let invoke = types
        .ancestor_id(ty.non_nullable())
        .filter(|&id| types.references.imported_type(id).is_delegate)
        .and_then(|id| match types.references.members(id, "Invoke") {
            [invoke] => Some(*invoke),
            _ => None,
        })
        .filter(|&invoke| {
            let method = types.references.method(invoke);
            method.modes.is_empty() && method.signature.type_arity == 0
        })
        .ok_or_else(|| {
            Diagnostic::new(
                "E0343",
                span,
                "a native callback requires a CLR delegate with by-value parameters",
            )
        })?;
    let invoke = Member::declared(invoke, Some(ty.non_nullable()));
    let signature = types.method_signature(invoke, span)?;
    for &ty in &signature.parameters {
        types.require_native_type(ty, span, "native callback parameters")?;
    }
    if signature.result != Type::Void {
        types.require_native_type(signature.result, span, "native callback results")?;
    }
    Ok(NativeCallback { invoke, signature })
}

//! Resolves imported ancestors and members in the receiving type's context.

use super::{InstanceKind, Span, Structs, Type, TypeInstance};
use crate::clr::Variance;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Member {
    pub id: usize,
    // Only constructed owners need a TypeSpec variant of the MemberRef.
    pub owner: Option<Type>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DataMember {
    Field(Member),
    Property(Member),
    Event(Member),
}

impl Member {
    pub(super) fn declared(id: usize, ty: Option<Type>) -> Self {
        Self {
            id,
            owner: ty.filter(|ty| {
                matches!(
                    ty,
                    Type::ConstructedExternal(_) | Type::ConstructedClrStruct(_)
                )
            }),
        }
    }

    pub(super) fn direct(ids: &[usize]) -> Vec<Self> {
        ids.iter().map(|&id| Self::declared(id, None)).collect()
    }
}

impl Structs<'_, '_> {
    pub(super) fn owner_arguments(&self, owner: Option<Type>) -> Option<Arc<[Type]>> {
        match owner {
            Some(Type::ConstructedExternal(id) | Type::ConstructedClrStruct(id)) => {
                Some(self.instance(id).arguments)
            }
            _ => None,
        }
    }

    pub(super) fn upcasts(&self, actual: Type, expected: Type) -> bool {
        if actual == expected
            || actual.is_nullable()
            || self.nullable_underlying(actual).is_some()
            || expected.is_nullable()
            || self.imported_id(expected).is_none()
            || self.ancestor_id(actual).is_none()
        {
            return false;
        }
        let Some(target) = self.variant_target(expected) else {
            return self.inherits(actual, expected);
        };
        let mut check = VarianceCheck {
            types: self,
            cache: HashMap::new(),
            remaining: 32_768,
        };
        check.variant_conversion(actual, expected, target, 0)
    }

    pub(super) fn inherits(&self, actual: Type, expected: Type) -> bool {
        actual != expected && self.ancestors(actual, true).any(|ty| ty == expected)
    }

    pub(super) fn ancestor_id(&self, ty: Type) -> Option<usize> {
        match ty {
            Type::ClrEnum(id) | Type::ClrStruct(id) => Some(id),
            Type::ConstructedClrStruct(id) => match self.instance(id).kind {
                InstanceKind::External(definition) => Some(definition),
                _ => unreachable!("constructed CLR struct"),
            },
            _ => self
                .imported_id(ty)
                .or_else(|| self.references.builtin_receiver(ty)),
        }
    }

    fn variant_target(&self, expected: Type) -> Option<TypeInstance> {
        let Type::ConstructedExternal(id) = expected else {
            return None;
        };
        let target = self.instance(id);
        let InstanceKind::External(definition) = target.kind else {
            unreachable!("imported variant type");
        };
        (!self
            .references
            .imported_type(definition)
            .variance
            .is_empty())
        .then_some(target)
    }

    pub(super) fn ancestors(
        &self,
        root: Type,
        include_interfaces: bool,
    ) -> impl Iterator<Item = Type> + '_ {
        let mut ready = vec![root];
        let mut visited = HashSet::new();
        std::iter::from_fn(move || {
            while let Some(ty) = ready.pop() {
                // Diamonds share visited types. The limit also bounds growth
                // from malformed hierarchies that instantiate fresh types.
                if visited.len() >= 4096 {
                    return None;
                }
                let Some(id) = self.ancestor_id(ty) else {
                    continue;
                };
                if visited.insert(ty) {
                    let arguments = self.owner_arguments(Some(ty));
                    let parents: Vec<_> = self
                        .references
                        .parents(id, include_interfaces)
                        .filter_map(|parent| {
                            self.import_signature_type(
                                parent,
                                arguments.as_deref().unwrap_or_default(),
                                Span::default(),
                            )
                            .ok()
                        })
                        .collect();
                    ready.extend(parents.into_iter().rev());
                    return Some(ty);
                }
            }
            None
        })
    }

    pub(super) fn instance_candidates(&self, owner: Type, name: &str) -> Vec<Member> {
        let Some(id) = self.ancestor_id(owner) else {
            return Vec::new();
        };
        let is_interface = self.references.imported_type(id).is_interface;
        let mut candidates = Vec::new();
        for ancestor in self.ancestors(owner, is_interface) {
            let id = self.ancestor_id(ancestor).expect("imported ancestor");
            for &id in self.references.members(id, name) {
                let candidate = Member::declared(id, Some(ancestor));
                if !candidates
                    .iter()
                    .any(|&other| self.hides(other, candidate, is_interface))
                {
                    if is_interface {
                        candidates.retain(|&other| !self.hides(candidate, other, true));
                    }
                    candidates.push(candidate);
                }
            }
            if !is_interface
                && self
                    .references
                    .members(id, name)
                    .iter()
                    .any(|&method| self.references.method(method).hide_by_name)
            {
                break;
            }
        }
        candidates
    }

    fn hides(&self, near: Member, far: Member, interfaces: bool) -> bool {
        let near_method = self.references.method(near.id);
        let far_method = self.references.method(far.id);
        let near_type = near.owner.unwrap_or(Type::External(near_method.owner));
        let far_type = far.owner.unwrap_or(Type::External(far_method.owner));
        near_type != far_type
            && (!interfaces || self.inherits(near_type, far_type))
            && (near_method.hide_by_name || self.same_signature(near, far))
    }

    fn same_signature(&self, a: Member, b: Member) -> bool {
        let a_method = self.references.method(a.id);
        let b_method = self.references.method(b.id);
        let a_signature = &a_method.signature;
        let b_signature = &b_method.signature;
        if a_signature.type_arity != b_signature.type_arity
            || a_signature.parameters.len() != b_signature.parameters.len()
            || (0..a_signature.parameters.len()).any(|index| {
                (a_method.parameter_mode(index) == crate::ArgumentMode::Value)
                    != (b_method.parameter_mode(index) == crate::ArgumentMode::Value)
            })
        {
            return false;
        }
        let a_arguments = self.owner_arguments(a.owner);
        let b_arguments = self.owner_arguments(b.owner);
        let same_type = |left: Type, right: Type| {
            let a = self.import_signature_type(
                left,
                a_arguments.as_deref().unwrap_or_default(),
                Span::default(),
            );
            let b = self.import_signature_type(
                right,
                b_arguments.as_deref().unwrap_or_default(),
                Span::default(),
            );
            matches!((a, b), (Ok(a), Ok(b)) if self.same_runtime_type(a, b))
        };
        same_type(a_signature.result, b_signature.result)
            && a_signature
                .parameters
                .iter()
                .zip(&b_signature.parameters)
                .all(|(&left, &right)| same_type(left, right))
    }

    fn same_runtime_type(&self, a: Type, b: Type) -> bool {
        let (a, b) = (a.non_nullable(), b.non_nullable());
        if a == b {
            return true;
        }
        let (Some(a), Some(b)) = (a.instance_id(), b.instance_id()) else {
            return false;
        };
        let (a, b) = (self.instance(a), self.instance(b));
        a.kind == b.kind
            && a.arguments.len() == b.arguments.len()
            && a.arguments
                .iter()
                .zip(b.arguments.iter())
                .all(|(&a, &b)| self.same_runtime_type(a, b))
    }

    pub(super) fn instance_data_members(&self, owner: Type, name: &str) -> Vec<DataMember> {
        let Some(id) = self.ancestor_id(owner) else {
            return Vec::new();
        };
        let is_interface = self.references.imported_type(id).is_interface;
        let declaring = self.ancestors(owner, is_interface).filter_map(|ty| {
            let id = self.ancestor_id(ty).expect("imported ancestor");
            let fields = self.references.declared_fields(id, name);
            let properties = self.references.declared_properties(id, name);
            let events = self.references.declared_events(id, name);
            (!fields.is_empty() || !properties.is_empty() || !events.is_empty())
                .then_some((ty, fields, properties, events))
        });
        if !is_interface {
            return declaring.take(1).flat_map(data_members).collect();
        }
        let declaring: Vec<_> = declaring.collect();
        declaring
            .iter()
            .copied()
            .filter(|&(ty, _, _, _)| {
                !declaring
                    .iter()
                    .any(|&(other, _, _, _)| self.inherits(other, ty))
            })
            .flat_map(data_members)
            .collect()
    }
}

struct VarianceCheck<'a, 'src, 'refs> {
    types: &'a Structs<'src, 'refs>,
    cache: HashMap<(Type, Type), bool>,
    remaining: usize,
}

impl VarianceCheck<'_, '_, '_> {
    fn reference_conversion(&mut self, actual: Type, expected: Type, depth: usize) -> bool {
        if actual == expected {
            return true;
        }
        if !actual.reference()
            || !expected.reference()
            || actual == Type::Null
            || expected == Type::Null
            || (actual.is_nullable() && !expected.is_nullable())
        {
            return false;
        }
        let (actual, expected) = (actual.non_nullable(), expected.non_nullable());
        if actual == expected || expected == Type::Object {
            return true;
        }
        if let Some(target) = self.types.variant_target(expected) {
            self.variant_conversion(actual, expected, target, depth)
        } else {
            self.types.inherits(actual, expected)
        }
    }

    fn variant_conversion(
        &mut self,
        actual: Type,
        expected: Type,
        target: TypeInstance,
        depth: usize,
    ) -> bool {
        let key = (actual, expected);
        if let Some(&cached) = self.cache.get(&key) {
            return cached;
        }
        if depth > crate::syntax::MAX_DEPTH || self.remaining == 0 {
            return false;
        }
        // A repeated active pair does not prove a conversion. Reuse completed
        // answers for nested generic arguments and bound malformed metadata.
        self.cache.insert(key, false);
        let types = self.types;
        let InstanceKind::External(definition) = target.kind else {
            unreachable!("imported variant type");
        };
        let variance = &types.references.imported_type(definition).variance;
        let mut compatible = false;
        for ancestor in types.ancestors(actual, true) {
            if self.remaining == 0 {
                break;
            }
            self.remaining -= 1;
            let Type::ConstructedExternal(id) = ancestor else {
                continue;
            };
            let candidate = types.instance(id);
            if candidate.kind != target.kind {
                continue;
            }
            compatible = variance
                .iter()
                .zip(candidate.arguments.iter().zip(target.arguments.iter()))
                .all(|(&variance, (&actual, &expected))| match variance {
                    Variance::Invariant => actual == expected,
                    Variance::Covariant => self.reference_conversion(actual, expected, depth + 1),
                    Variance::Contravariant => {
                        self.reference_conversion(expected, actual, depth + 1)
                    }
                });
            if compatible {
                break;
            }
        }
        self.cache.insert(key, compatible);
        compatible
    }
}

fn data_members<'a>(
    (ty, fields, properties, events): (Type, &'a [usize], &'a [usize], &'a [usize]),
) -> impl Iterator<Item = DataMember> + 'a {
    fields
        .iter()
        .map(move |&id| DataMember::Field(Member::declared(id, Some(ty))))
        .chain(
            properties
                .iter()
                .map(move |&id| DataMember::Property(Member::declared(id, Some(ty)))),
        )
        .chain(
            events
                .iter()
                .map(move |&id| DataMember::Event(Member::declared(id, Some(ty)))),
        )
}

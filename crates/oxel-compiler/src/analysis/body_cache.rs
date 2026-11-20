//! Reuses compiler facts and diagnostics until a body's inputs change.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use crate::{Diagnostic, Occurrence, Span, Type, hir::EvaluationCache};

#[derive(Clone, Copy)]
pub(super) struct FunctionInput<'src> {
    pub name: &'src str,
    pub signature: blake3::Hash,
    pub body: blake3::Hash,
    pub definition: Span,
}

#[derive(Clone, Copy)]
struct FunctionVersion {
    signature: blake3::Hash,
    body: blake3::Hash,
    definition: Span,
    generation: u64,
}

pub(super) struct TypeInput<'src> {
    pub name: &'src str,
    pub shape: blake3::Hash,
    pub definitions: Vec<Span>,
    pub dependencies: Vec<&'src str>,
}

struct TypeVersion {
    shape: blake3::Hash,
    definitions: Vec<Span>,
    dependencies: Vec<Arc<str>>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Lookup<'name> {
    Function(&'name str),
    Constant(&'name str),
    Type(&'name str),
}

struct ConstantVersion {
    value: blake3::Hash,
    definition: Span,
}

#[derive(Default)]
pub(crate) struct BodyCache {
    environment: Option<blake3::Hash>,
    type_environment: Option<blake3::Hash>,
    type_revision: u64,
    generation: u64,
    functions: HashMap<Arc<str>, FunctionVersion>,
    types: HashMap<Arc<str>, TypeVersion>,
    type_definitions: HashMap<usize, (Arc<str>, usize)>,
    function_definitions: HashMap<usize, Arc<str>>,
    constants: HashMap<Arc<str>, ConstantVersion>,
    constant_definitions: HashMap<usize, Arc<str>>,
    results: HashMap<Arc<str>, BodyResult>,
    pub checked_functions: usize,
    pub evaluation: EvaluationCache,
}

struct BodyResult {
    hash: blake3::Hash,
    dependencies: Vec<Dependency>,
    type_dependencies: Vec<(Arc<str>, Option<blake3::Hash>)>,
    type_revision: u64,
    occurrences: Vec<CachedOccurrence>,
    diagnostic: Option<Box<CachedDiagnostic>>,
}

struct CachedDiagnostic {
    error: Diagnostic,
    related: Option<CachedDefinition>,
}

enum Dependency {
    Function(Arc<str>, Option<blake3::Hash>),
    Constant(Arc<str>, Option<blake3::Hash>),
}

struct CachedOccurrence {
    occurrence: Occurrence,
    definition: Option<CachedDefinition>,
}

enum CachedDefinition {
    Local(Span),
    TypeMember(Arc<str>, usize),
    Constant(Arc<str>),
    Function(Arc<str>),
}

impl BodyCache {
    pub(super) fn prepare<'src>(
        &mut self,
        environment: blake3::Hash,
        functions: impl IntoIterator<Item = FunctionInput<'src>>,
        types: impl Iterator<Item = &'src TypeInput<'src>> + Clone,
        constant_sources: impl IntoIterator<Item = (&'src str, blake3::Hash)>,
    ) {
        let reset = self.environment != Some(environment);
        if reset {
            self.results.clear();
            self.environment = Some(environment);
        }
        self.prepare_types(types);
        // Keep the function catalog in place and remove entries absent from this snapshot.
        self.generation = self.generation.wrapping_add(1);
        self.evaluation
            .prepare(reset, self.generation, constant_sources);
        let mut definitions_changed = false;
        let mut live = 0;
        for input in functions {
            let current = FunctionVersion {
                signature: input.signature,
                body: input.body,
                definition: input.definition,
                generation: self.generation,
            };
            if let Some(previous) = self.functions.get_mut(input.name) {
                live += usize::from(previous.generation != self.generation);
                definitions_changed |= previous.definition != current.definition;
                *previous = current;
            } else {
                self.functions.insert(input.name.into(), current);
                live += 1;
                definitions_changed = true;
            }
        }
        if live != self.functions.len() {
            self.functions
                .retain(|_, version| version.generation == self.generation);
            self.results
                .retain(|name, _| self.functions.contains_key(name));
            definitions_changed = true;
        }
        if definitions_changed {
            self.function_definitions.clear();
            self.function_definitions.extend(
                self.functions
                    .iter()
                    .map(|(name, version)| (version.definition.start, Arc::clone(name))),
            );
        }
    }

    fn prepare_types<'src>(&mut self, types: impl Iterator<Item = &'src TypeInput<'src>> + Clone) {
        let mut hash = blake3::Hasher::new();
        let mut count = 0;
        for input in types.clone() {
            hash.update(input.shape.as_bytes());
            count += 1;
        }
        let environment = Some(hash.finalize());
        if self.type_environment == environment
            && count == self.types.len()
            && types
                .clone()
                .all(|input| self.types[input.name].definitions == input.definitions)
        {
            return;
        }
        if self.type_environment != environment {
            self.type_environment = environment;
            self.type_revision = self.type_revision.wrapping_add(1);
        }
        self.type_definitions.clear();
        self.types.clear();
        for input in types {
            let name = Arc::<str>::from(input.name);
            self.type_definitions.extend(
                input
                    .definitions
                    .iter()
                    .enumerate()
                    .map(|(index, span)| (span.start, (Arc::clone(&name), index))),
            );
            self.types.insert(
                name,
                TypeVersion {
                    shape: input.shape,
                    definitions: input.definitions.clone(),
                    dependencies: input.dependencies.iter().copied().map(Arc::from).collect(),
                },
            );
        }
    }

    fn type_dependencies<'src>(
        &'src self,
        mut names: Vec<&'src str>,
    ) -> Vec<(Arc<str>, Option<blake3::Hash>)> {
        let mut seen = HashSet::new();
        let mut dependencies = Vec::new();
        while let Some(name) = names.pop() {
            if !seen.insert(name) {
                continue;
            }
            if let Some((key, ty)) = self.types.get_key_value(name) {
                dependencies.push((Arc::clone(key), Some(ty.shape)));
                names.extend(ty.dependencies.iter().map(Arc::as_ref));
            } else {
                dependencies.push((Arc::from(name), None));
            }
        }
        dependencies
    }

    pub(crate) fn hash_type_dependencies(&self, hash: &mut blake3::Hasher, lookups: &[Lookup<'_>]) {
        let mut dependencies = self.type_dependencies(
            lookups
                .iter()
                .filter_map(|lookup| match *lookup {
                    Lookup::Type(name) => Some(name),
                    _ => None,
                })
                .collect(),
        );
        dependencies.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        hash.update(&(dependencies.len() as u64).to_le_bytes());
        for (name, shape) in dependencies {
            hash.update(&(name.len() as u64).to_le_bytes());
            hash.update(name.as_bytes());
            if let Some(shape) = shape {
                hash.update(&[1]);
                hash.update(shape.as_bytes());
            } else {
                hash.update(&[0]);
            }
        }
    }

    pub fn prepare_constants<'src>(
        &mut self,
        constants: impl IntoIterator<Item = (&'src str, blake3::Hash, Span)>,
    ) {
        self.constants.clear();
        self.constant_definitions.clear();
        for (name, value, definition) in constants {
            let name = self.evaluation.name(name);
            self.constant_definitions
                .insert(definition.start, Arc::clone(&name));
            self.constants
                .insert(name, ConstantVersion { value, definition });
        }
    }

    fn constant_value(&self, name: &str) -> Option<blake3::Hash> {
        self.constants.get(name).map(|constant| constant.value)
    }

    pub fn function_body(&self, name: &str) -> blake3::Hash {
        self.functions[name].body
    }

    fn signature(&self, name: &str) -> Option<blake3::Hash> {
        self.functions.get(name).map(|function| function.signature)
    }

    fn types_unchanged(&self, result: &BodyResult) -> bool {
        if result.type_revision == self.type_revision {
            return true;
        }
        // Diagnostics expose compiler-local type IDs, which can move when
        // declarations change even if the diagnostic's type shape is stable.
        if let Some(diagnostic) = &result.diagnostic
            && [diagnostic.error.expected, diagnostic.error.actual]
                .into_iter()
                .flatten()
                .any(|ty| matches!(ty, Type::Struct(_)) || ty.instance_id().is_some())
        {
            return false;
        }
        result
            .type_dependencies
            .iter()
            .all(|(name, shape)| self.types.get(name).map(|ty| ty.shape) == *shape)
    }

    pub fn reuse(
        &mut self,
        name: &str,
        base: usize,
        occurrences: &mut Vec<Occurrence>,
    ) -> Option<Result<(), Diagnostic>> {
        let hash = self.functions[name].body;
        let result = self.results.get(name).filter(|result| {
            result.hash == hash
                && self.types_unchanged(result)
                && result
                    .dependencies
                    .iter()
                    .all(|dependency| match dependency {
                        Dependency::Function(name, signature) => self.signature(name) == *signature,
                        Dependency::Constant(name, value) => self.constant_value(name) == *value,
                    })
        })?;
        occurrences.extend(result.occurrences.iter().map(|fact| {
            let mut item = fact.occurrence.clone();
            item.span = absolute(item.span, base);
            item.visibility = item.visibility.map(|span| absolute(span, base));
            item.definition = fact
                .definition
                .as_ref()
                .map(|definition| self.resolve_definition(definition, base));
            item
        }));
        let diagnostic = match result.diagnostic.as_deref() {
            Some(cached) => {
                let mut error = cached.error.clone();
                error.map_spans(|span| absolute(span, base));
                if let Some((related, definition)) =
                    error.related_mut().zip(cached.related.as_ref())
                {
                    related.span = self.resolve_definition(definition, base);
                }
                Err(error)
            }
            None => Ok(()),
        };
        if result.type_revision != self.type_revision {
            self.results
                .get_mut(name)
                .expect("reused body")
                .type_revision = self.type_revision;
        }
        Some(diagnostic)
    }

    pub fn store(
        &mut self,
        name: &str,
        base: usize,
        facts: &[Occurrence],
        mut lookups: Vec<Lookup<'_>>,
        result: &Result<(), Diagnostic>,
    ) {
        self.checked_functions += 1;
        lookups.sort_unstable();
        lookups.dedup();
        let mut types = Vec::new();
        let dependencies = lookups
            .into_iter()
            .filter_map(|lookup| match lookup {
                Lookup::Function(name) => Some(Dependency::Function(
                    self.functions
                        .get_key_value(name)
                        .map_or_else(|| Arc::from(name), |(key, _)| Arc::clone(key)),
                    self.signature(name),
                )),
                Lookup::Constant(name) => Some(Dependency::Constant(
                    self.constants
                        .get_key_value(name)
                        .map_or_else(|| Arc::from(name), |(key, _)| Arc::clone(key)),
                    self.constant_value(name),
                )),
                Lookup::Type(name) => {
                    types.push(name);
                    None
                }
            })
            .collect();
        let cached_facts = facts
            .iter()
            .map(|item| {
                let mut occurrence = item.clone();
                occurrence.span = local(occurrence.span, base);
                occurrence.visibility = occurrence.visibility.map(|span| local(span, base));
                let definition = occurrence
                    .definition
                    .take()
                    .map(|span| self.cache_definition(span, base));
                if let Some(CachedDefinition::TypeMember(name, _)) = &definition {
                    // Type members can also be discovered through inferred types.
                    // Use the catalog's name so the dependency borrows its storage.
                    types.push(
                        self.types
                            .get_key_value(name)
                            .expect("catalogued type member")
                            .0,
                    );
                }
                CachedOccurrence {
                    occurrence,
                    definition,
                }
            })
            .collect();
        let type_dependencies = self.type_dependencies(types);
        let diagnostic = result.as_ref().err().map(|error| {
            let mut error = error.clone();
            let related = error
                .related()
                .map(|item| self.cache_definition(item.span, base));
            error.map_spans(|span| local(span, base));
            Box::new(CachedDiagnostic { error, related })
        });
        let key = Arc::clone(
            self.functions
                .get_key_value(name)
                .expect("current function")
                .0,
        );
        self.results.insert(
            key,
            BodyResult {
                hash: self.functions[name].body,
                dependencies,
                type_dependencies,
                type_revision: self.type_revision,
                occurrences: cached_facts,
                diagnostic,
            },
        );
    }

    fn cache_definition(&self, span: Span, base: usize) -> CachedDefinition {
        if let Some(name) = self.function_definitions.get(&span.start) {
            CachedDefinition::Function(Arc::clone(name))
        } else if let Some(name) = self.constant_definitions.get(&span.start) {
            CachedDefinition::Constant(Arc::clone(name))
        } else if let Some((name, index)) = self.type_definitions.get(&span.start) {
            CachedDefinition::TypeMember(Arc::clone(name), *index)
        } else {
            CachedDefinition::Local(local(span, base))
        }
    }

    fn resolve_definition(&self, definition: &CachedDefinition, base: usize) -> Span {
        match definition {
            CachedDefinition::Local(span) => absolute(*span, base),
            CachedDefinition::TypeMember(name, index) => self.types[name].definitions[*index],
            CachedDefinition::Constant(name) => self.constants[name].definition,
            CachedDefinition::Function(name) => self.functions[name].definition,
        }
    }
}

fn local(span: Span, base: usize) -> Span {
    Span {
        start: span.start - base,
        end: span.end - base,
    }
}

fn absolute(span: Span, base: usize) -> Span {
    Span {
        start: span.start + base,
        end: span.end + base,
    }
}

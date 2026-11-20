//! Retains successful constant evaluations across editor snapshots.

use std::{collections::HashMap, sync::Arc};

use super::interpreter::Evaluated;
use crate::{
    analysis::{BodyCache, Lookup},
    hir::Checked,
    syntax,
};

#[derive(Default)]
pub(crate) struct EvaluationCache {
    entries: HashMap<Arc<str>, Entry>,
    pub evaluated_constants: usize,
}

struct Entry {
    source: blake3::Hash,
    generation: u64,
    result: Option<(blake3::Hash, Evaluated)>,
}

impl EvaluationCache {
    pub(crate) fn prepare<'src>(
        &mut self,
        reset: bool,
        generation: u64,
        inputs: impl IntoIterator<Item = (&'src str, blake3::Hash)>,
    ) {
        if reset {
            self.entries.clear();
        }
        let mut live = 0;
        for (name, source) in inputs {
            if let Some(entry) = self.entries.get_mut(name) {
                live += usize::from(entry.generation != generation);
                entry.generation = generation;
                entry.source = source;
            } else {
                self.entries.insert(
                    Arc::from(name),
                    Entry {
                        source,
                        generation,
                        result: None,
                    },
                );
                live += 1;
            }
        }
        if live != self.entries.len() {
            self.entries
                .retain(|_, entry| entry.generation == generation);
        }
    }

    pub(crate) fn name(&self, name: &str) -> Arc<str> {
        Arc::clone(
            self.entries
                .get_key_value(name)
                .expect("current constant")
                .0,
        )
    }

    pub(super) fn value(
        &mut self,
        name: &str,
        fingerprint: blake3::Hash,
        run: impl FnOnce() -> Checked<Evaluated>,
    ) -> Checked<Evaluated> {
        let entry = self.entries.get_mut(name).expect("current constant");
        if let Some((previous, value)) = &entry.result
            && *previous == fingerprint
        {
            return Ok(value.clone());
        }
        self.evaluated_constants += 1;
        let value = run()?;
        entry.result = Some((fingerprint, value.clone()));
        Ok(value)
    }
}

/// Versions graph nodes in dependency order. A constant's dependents observe its
/// typed value. A comptime function has no value until called, so dependents
/// observe its source and everything it might read or call.
pub(super) struct Fingerprints {
    values: Vec<Option<blake3::Hash>>,
}

impl Fingerprints {
    pub(super) fn new(nodes: usize) -> Self {
        Self {
            values: vec![None; nodes],
        }
    }

    pub(super) fn initializer(
        &self,
        cache: &BodyCache,
        declaration: &syntax::ConstDefinition<'_>,
        dependencies: &[usize],
        lookups: &[Lookup<'_>],
    ) -> blake3::Hash {
        let mut hash = blake3::Hasher::new();
        hash.update(cache.evaluation.entries[declaration.name].source.as_bytes());
        self.dependencies(&mut hash, dependencies);
        cache.hash_type_dependencies(&mut hash, lookups);
        hash.finalize()
    }

    pub(super) fn functions(
        &mut self,
        cache: &BodyCache,
        group: &[usize],
        edges: &[Vec<usize>],
        functions: &[&syntax::Function<'_>],
        constant_count: usize,
        lookups: &[Lookup<'_>],
    ) {
        let mut hash = blake3::Hasher::new();
        let function = |node: usize| functions[node - constant_count];
        let mut sorted = Vec::new();
        let members = if group.len() == 1 {
            group
        } else {
            sorted.extend_from_slice(group);
            sorted.sort_unstable_by_key(|&node| function(node).name);
            &sorted
        };
        for &node in members {
            hash.update(cache.function_body(function(node).name).as_bytes());
            self.dependencies(&mut hash, &edges[node]);
        }
        cache.hash_type_dependencies(&mut hash, lookups);
        let fingerprint = hash.finalize();
        for &node in group {
            self.record(node, fingerprint);
        }
    }

    fn dependencies(&self, hash: &mut blake3::Hasher, dependencies: &[usize]) {
        hash.update(&(dependencies.len() as u64).to_le_bytes());
        for &dependency in dependencies {
            if let Some(value) = self.values[dependency] {
                hash.update(&[1]);
                hash.update(value.as_bytes());
            } else {
                // An unversioned edge stays within this mutually recursive group.
                hash.update(&[0]);
            }
        }
    }

    pub(super) fn record(&mut self, node: usize, version: blake3::Hash) {
        self.values[node] = Some(version);
    }
}

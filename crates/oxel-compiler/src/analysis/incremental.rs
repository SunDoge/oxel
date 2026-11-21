//! Keeps parsed source files alive between project analysis requests.

use std::sync::Arc;

use self_cell::self_cell;

use super::{
    BodyCache, DocumentAnalysis,
    body_cache::{FunctionInput, TypeInput},
    inspect_parsed, inspect_project_with_kind, shape,
};
use crate::{
    AssemblyKind, Diagnostic, References, SourceDiagnostic, cil, hir,
    project::{SourceMap, compile_project_with_kind},
    syntax,
};

struct Parsed<'src> {
    file: syntax::File<'src>,
    diagnostics: Vec<Diagnostic>,
    types: Vec<TypeInput<'src>>,
    functions: Vec<FunctionInput<'src>>,
    constant_sources: Vec<(&'src str, blake3::Hash)>,
    interface: blake3::Hash,
}

impl<'src> Parsed<'src> {
    fn new(source: &'src str, base: usize) -> Self {
        let (file, diagnostics) = syntax::parse_at(source, base);
        let text = |start: usize, end: usize| &source.as_bytes()[start - base..end - base];
        let mut functions = Vec::with_capacity(file.functions.len());
        for function in &file.functions {
            let start = function.declaration_span.start;
            functions.push(FunctionInput {
                name: function.name,
                signature: shape::signature(function),
                body: blake3::hash(text(start, function.declaration_span.end)),
                definition: function.name_span,
            });
        }
        let mut types = Vec::with_capacity(file.types.len());
        for structure in &file.types {
            let mut definitions = vec![structure.span];
            definitions.extend(structure.type_parameters.iter().map(|&(_, span)| span));
            match &structure.shape {
                syntax::TypeShape::Struct(fields) => {
                    definitions.extend(fields.iter().map(|(_, span, _)| *span));
                }
                syntax::TypeShape::Union(variants) => {
                    definitions.extend(variants.iter().map(|variant| variant.span));
                }
            }
            let mut dependencies = Vec::new();
            shape::declared_type_names(structure, |name| dependencies.push(name));
            types.push(TypeInput {
                name: structure.name,
                shape: shape::declared_type(structure),
                definitions,
                dependencies,
            });
        }
        let constant_sources: Vec<_> = file
            .constants
            .iter()
            .map(|constant| {
                let span = constant.declaration_span;
                (constant.name, blake3::hash(text(span.start, span.end)))
            })
            .collect();
        let mut interface = blake3::Hasher::new();
        shape::file_scope(&mut interface, &file.scope);
        interface.update(&(types.len() as u64).to_le_bytes());
        for input in &types {
            interface.update(input.shape.as_bytes());
        }
        interface.update(&(functions.len() as u64).to_le_bytes());
        for (function, input) in file.functions.iter().zip(&functions) {
            let shape = if function.comptime || function.body.managed().is_none() {
                input.body
            } else {
                input.signature
            };
            interface.update(shape.as_bytes());
        }
        interface.update(&(constant_sources.len() as u64).to_le_bytes());
        for (_, hash) in &constant_sources {
            interface.update(hash.as_bytes());
        }
        Self {
            file,
            diagnostics,
            types,
            functions,
            constant_sources,
            interface: interface.finalize(),
        }
    }
}

self_cell! {
    struct ParsedSource {
        owner: Arc<str>,

        #[covariant]
        dependent: Parsed,
    }
}

struct CachedFile {
    base: usize,
    reserved: usize,
    source: ParsedSource,
}

#[derive(Default)]
struct FileCache {
    files: Vec<CachedFile>,
    parsed: usize,
}

/// Reuses syntax and checked function bodies across compilations of one module.
/// Its reference assemblies and output kind are fixed for the session.
pub struct ProjectCompiler<'refs> {
    kind: AssemblyKind,
    references: &'refs References,
    files: FileCache,
    cache: hir::CompilationCache,
}

impl<'refs> ProjectCompiler<'refs> {
    pub fn new(kind: AssemblyKind, references: &'refs References) -> Self {
        Self {
            kind,
            references,
            files: FileCache::default(),
            cache: hir::CompilationCache::default(),
        }
    }

    /// Number of input files parsed by the last build.
    pub fn parsed_files(&self) -> usize {
        self.files.parsed
    }

    /// Number of runtime bodies checked by the last incremental build.
    pub fn checked_functions(&self) -> usize {
        self.cache.checked_functions
    }

    pub fn compile(
        &mut self,
        sources: &[Arc<str>],
        assembly_name: &str,
    ) -> Result<Vec<u8>, Vec<SourceDiagnostic>> {
        self.cache.checked_functions = 0;
        let text: Vec<_> = sources.iter().map(Arc::as_ref).collect();
        let Some(map) = self.files.update(sources) else {
            self.cache = hir::CompilationCache::default();
            return compile_project_with_kind(&text, assembly_name, self.references, self.kind);
        };
        let parsed = self.files.parsed();
        let errors = diagnostics(parsed.clone());
        if !errors.is_empty() {
            return Err(map.locate_all(errors));
        }
        let mut interface = blake3::Hasher::new();
        for source in parsed.clone() {
            interface.update(source.interface.as_bytes());
        }
        self.cache.prepare(interface.finalize());
        let keys: Vec<_> = parsed
            .clone()
            .flat_map(|source| source.file.functions.iter().zip(&source.functions))
            .filter(|(function, _)| !function.comptime && function.test.is_none())
            .map(|(function, input)| (function.name_span.start, input.body))
            .collect();
        let module = syntax::Module::new(parsed.map(|source| &source.file));
        let program = hir::lower_cached(module, self.references, self.kind, &mut self.cache, &keys)
            .map_err(|errors| map.locate_all(errors))?;
        cil::emit(&program, &text, assembly_name, self.references, None)
            .map_err(|error| vec![map.locate(error)])
    }
}

/// A reusable analysis cache for the ordered inputs of one oxel module.
///
/// Parsed files keep their syntax across edits. Function checks are reusable
/// while their source, signatures, type dependencies, and evaluated constants stay unchanged.
#[derive(Default)]
pub struct ProjectAnalyzer {
    kind: AssemblyKind,
    files: FileCache,
    bodies: BodyCache,
}

impl ProjectAnalyzer {
    /// Creates an analysis cache for the selected kind of module.
    pub fn new(kind: AssemblyKind) -> Self {
        Self {
            kind,
            ..Self::default()
        }
    }

    /// Returns the number of source files parsed during the last inspection.
    pub fn parsed_files(&self) -> usize {
        self.files.parsed
    }

    /// Returns the number of function bodies checked through the cache in the last inspection.
    pub fn checked_functions(&self) -> usize {
        self.bodies.checked_functions
    }

    /// Returns the number of constant initializers executed through the cache in the last inspection.
    pub fn evaluated_constants(&self) -> usize {
        self.bodies.evaluation.evaluated_constants
    }

    /// Returns a fresh analysis snapshot for each input, reusing unchanged syntax.
    pub fn inspect(
        &mut self,
        sources: &[Arc<str>],
        references: &References,
    ) -> Vec<DocumentAnalysis> {
        self.bodies.checked_functions = 0;
        self.bodies.evaluation.evaluated_constants = 0;
        let Some(map) = self.files.update(sources) else {
            self.bodies = BodyCache::default();
            let text: Vec<_> = sources.iter().map(Arc::as_ref).collect();
            return inspect_project_with_kind(&text, references, self.kind);
        };
        let parsed = self.files.parsed();
        let diagnostics = diagnostics(parsed.clone());
        let scoped = parsed.clone().any(|source| {
            source.file.scope.namespace.is_some() || !source.file.scope.imports.is_empty()
        });
        // Declaration and dependency cache keys in flat projects are unqualified.
        // Scoped projects still reuse parsed files and rebind their names.
        if scoped {
            self.bodies = BodyCache::default();
        } else if diagnostics.is_empty() {
            let mut environment = blake3::Hasher::new();
            environment.update(&(references.assemblies().len() as u64).to_le_bytes());
            for assembly in references.assemblies() {
                environment.update(assembly.hash.as_bytes());
            }
            let functions = parsed
                .clone()
                .flat_map(|source| source.functions.iter().copied());
            let types = parsed.clone().flat_map(|source| &source.types);
            let constant_sources = parsed
                .clone()
                .flat_map(|source| source.constant_sources.iter().copied());
            self.bodies
                .prepare(environment.finalize(), functions, types, constant_sources);
        }
        let module = syntax::Module::new(parsed.map(|source| &source.file));
        inspect_parsed(
            &map,
            module,
            diagnostics,
            references,
            self.kind,
            (!scoped).then_some(&mut self.bodies),
        )
    }
}

fn diagnostics<'a>(parsed: impl Iterator<Item = &'a Parsed<'a>>) -> Vec<Diagnostic> {
    parsed
        .flat_map(|source| source.diagnostics.iter().cloned())
        .collect()
}

impl FileCache {
    fn parsed(&self) -> impl Iterator<Item = &Parsed<'_>> + Clone {
        self.files.iter().map(|file| file.source.borrow_dependent())
    }

    fn update(&mut self, sources: &[Arc<str>]) -> Option<SourceMap> {
        self.parsed = 0;
        self.files.truncate(sources.len());

        let mut base = 0usize;
        for (index, source) in sources.iter().enumerate() {
            let required = source.len() + 1; // Include the EOF span.
            let retained = self.files.get(index);
            let reserved = retained
                .map(|file| file.reserved)
                .filter(|&reserved| reserved >= required)
                .unwrap_or_else(|| {
                    required
                        .max(4096)
                        .checked_next_power_of_two()
                        .unwrap_or(required)
                });
            // On narrow hosts the padded layout may fill before a packed one.
            let Some(next_base) = base.checked_add(reserved) else {
                self.files.clear();
                self.parsed = sources.len();
                return None;
            };
            let reusable = retained.is_some_and(|file| {
                let old = file.source.borrow_owner();
                file.base == base && (Arc::ptr_eq(old, source) || old == source)
            });
            if !reusable {
                let file = CachedFile {
                    base,
                    reserved,
                    source: ParsedSource::new(Arc::clone(source), |text| Parsed::new(text, base)),
                };
                if index == self.files.len() {
                    self.files.push(file);
                } else {
                    self.files[index] = file;
                }
                self.parsed += 1;
            }
            // Reserve room for edits so offsets in later files remain reusable.
            base = next_base;
        }

        Some(SourceMap {
            bases: self.files.iter().map(|file| file.base).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same_image(compiler: &mut ProjectCompiler<'_>, sources: &[Arc<str>]) {
        let text: Vec<_> = sources.iter().map(Arc::as_ref).collect();
        assert_eq!(
            compiler.compile(sources, "Example").unwrap(),
            compile_project_with_kind(&text, "Example", compiler.references, compiler.kind)
                .unwrap()
        );
    }

    #[test]
    fn incremental_emission_reuses_shifted_bodies_and_scoped_generic_types() {
        let references = References::default();
        let mut compiler = ProjectCompiler::new(AssemblyKind::Library, &references);
        let mut sources: [Arc<str>; 2] = [
            "namespace Model; struct Box<T> { value: T } fn box_it<T>(value: T) -> Box<T> { Box::<T> { value: value } }".into(),
            "namespace View; use Model::box_it; fn edited() -> i32 { 1 } fn stable() -> i32 { box_it([42]).value[0] }".into(),
        ];
        same_image(&mut compiler, &sources);
        assert_eq!(compiler.parsed_files(), 2);
        assert_eq!(compiler.checked_functions(), 3);

        sources[1] = sources[1]
            .replace("{ 1 }", "{ let value = 200; value }")
            .into();
        same_image(&mut compiler, &sources);
        assert_eq!(compiler.parsed_files(), 1);
        assert_eq!(compiler.checked_functions(), 1);

        same_image(&mut compiler, &sources);
        assert_eq!(compiler.parsed_files(), 0);
        assert_eq!(compiler.checked_functions(), 0);

        sources[0] = sources[0]
            .replace("value: T }", "value: T, extra: i32 }")
            .replace("value: value }", "value: value, extra: 5 }")
            .into();
        same_image(&mut compiler, &sources);
        assert_eq!(compiler.checked_functions(), 3);

        sources[1] = sources[1]
            .replace("use Model::box_it", "use Model::box_it as pack")
            .into();
        let error = compiler.compile(&sources, "Example").unwrap_err();
        assert_eq!(error[0].source, Some(1));
        sources[1] = sources[1].replace("{ box_it(", "{ pack(").into();
        same_image(&mut compiler, &sources);
    }

    #[test]
    fn incremental_emission_replays_builtins_and_drops_unused_option_types() {
        let references = References::default();
        let mut compiler = ProjectCompiler::new(AssemblyKind::Library, &references);
        let mut sources: [Arc<str>; 2] = [
            "const VALUE = 4; fn edited() -> i32 { VALUE }".into(),
            "fn uses_option() -> i32 { let value: Option<i32[]> = Option.Some([42]); (value ?? [0])[0] }".into(),
        ];
        same_image(&mut compiler, &sources);
        sources[0] = "const VALUE = 4; fn edited() -> i32 { VALUE + 1 }".into();
        same_image(&mut compiler, &sources);
        assert_eq!(compiler.checked_functions(), 1);

        sources[1] = "fn uses_option() -> i32 { 42 }".into();
        same_image(&mut compiler, &sources);
        assert_eq!(compiler.checked_functions(), 1);

        sources[0] = "const VALUE = 8; fn edited() -> i32 { VALUE + 1 }".into();
        same_image(&mut compiler, &sources);
        assert_eq!(compiler.checked_functions(), 2);
    }
}

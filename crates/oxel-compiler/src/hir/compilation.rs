//! Keeps checked bodies and their type IDs across edits to a stable declaration set.

use std::cell::Cell;

use super::*;

pub(super) const OPTION: u8 = 1;
pub(super) const FIELD_INFO: u8 = 2;
pub(super) const RESULT: u8 = 4;

thread_local! {
    // Each dirty body is checked on one worker, including its nested lambdas.
    // Declarations and constants record use directly on the current module.
    static BUILTIN_USE: Cell<Option<u8>> = const { Cell::new(None) };
}

pub(super) fn record_builtin(flag: u8) {
    BUILTIN_USE.with(|usage| {
        if let Some(bits) = usage.get() {
            usage.set(Some(bits | flag));
        }
    });
}

type BodyInput = (usize, blake3::Hash);

struct CheckedBody {
    hash: blake3::Hash,
    function: Arc<Function>,
    builtins: u8,
}

#[derive(Default)]
pub(crate) struct CompilationCache {
    interface: Option<blake3::Hash>,
    instances: Arc<RwLock<TypeInstances>>,
    bodies: Vec<Option<CheckedBody>>,
    pub checked_functions: usize,
}

impl CompilationCache {
    pub fn prepare(&mut self, interface: blake3::Hash) {
        // Source and imported type/method IDs are stable only while the module's
        // declarations have the same shape and order. References are fixed.
        // Cap retained type instances in a long-running editing session.
        if self.interface != Some(interface)
            || self.instances.read().expect("type interner").values.len() > 65_536
        {
            *self = Self {
                interface: Some(interface),
                ..Self::default()
            };
        }
    }

    fn check(
        &mut self,
        scope: &ModuleScope<'_, '_, '_>,
        inputs: &[BodyInput],
    ) -> Result<Vec<Arc<Function>>, Vec<Diagnostic>> {
        let declarations = scope.runtime_functions();
        debug_assert_eq!(declarations.len(), inputs.len());
        self.bodies.resize_with(inputs.len(), || None);
        let dirty: Vec<_> = self
            .bodies
            .iter_mut()
            .zip(inputs)
            .enumerate()
            .filter_map(|(index, (cached, &(_, hash)))| {
                if cached.as_ref().is_some_and(|cached| cached.hash == hash) {
                    None
                } else {
                    *cached = None;
                    Some(index)
                }
            })
            .collect();
        self.checked_functions = dirty.len();
        let mut errors = Vec::new();
        schedule::ordered(
            &dirty,
            |&index| body_bytes(declarations[index]),
            |_, &index| {
                BUILTIN_USE.with(|usage| {
                    usage.set(Some(0));
                    let checked = scope.check(index, None, None);
                    let builtins = usage.replace(None).expect("recording body");
                    let body = checked.map(|function| CheckedBody {
                        hash: inputs[index].1,
                        function: Arc::new(function),
                        builtins,
                    });
                    (index, body)
                })
            },
            |(index, result)| match result {
                Ok(body) => self.bodies[index] = Some(body),
                Err(error) => errors.push(error),
            },
        );
        if !errors.is_empty() {
            return Err(errors);
        }
        let mut builtins = 0;
        let functions = self
            .bodies
            .iter()
            .map(|body| {
                let body = body.as_ref().expect("successfully checked body");
                builtins |= body.builtins;
                Arc::clone(&body.function)
            })
            .collect();
        if builtins & OPTION != 0 {
            scope.types.option_used.store(true, Ordering::Relaxed);
        }
        if builtins & FIELD_INFO != 0 {
            scope.types.field_info_used.store(true, Ordering::Relaxed);
        }
        if builtins & RESULT != 0 {
            scope.types.result_used.store(true, Ordering::Relaxed);
        }
        Ok(functions)
    }
}

pub(crate) fn lower_cached<'src>(
    mut file: syntax::Module<'_, 'src>,
    references: &References,
    kind: AssemblyKind,
    cache: &mut CompilationCache,
    inputs: &[BodyInput],
) -> Result<Program<'src>, Vec<Diagnostic>> {
    file.functions.retain(|function| function.test.is_none());
    let scope = prepare(
        file,
        references,
        EntryPoint::for_build(kind),
        None,
        None,
        Arc::clone(&cache.instances),
    )?;
    let functions = cache.check(&scope, inputs)?;
    let mut program = finish_program(scope, functions, kind, None)?;
    if program
        .functions
        .iter()
        .zip(inputs)
        .any(|(function, &(start, _))| function.span.start != start)
    {
        program.current_function_starts = inputs.iter().map(|&(start, _)| start).collect();
    }
    Ok(program)
}

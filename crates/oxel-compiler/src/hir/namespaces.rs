use std::{borrow::Cow, collections::HashMap};

use crate::{Diagnostic, Span, syntax::FileScope};

#[derive(Default)]
pub(super) struct Scopes<'src> {
    files: Vec<Scope<'src>>,
}

struct Scope<'src> {
    start: usize,
    namespace: Option<String>,
    imports: HashMap<&'src str, String>,
}

impl<'src> Scopes<'src> {
    pub(super) fn new(scopes: &[&FileScope<'src>]) -> Result<Self, Diagnostic> {
        if scopes
            .iter()
            .all(|file| file.namespace.is_none() && file.imports.is_empty())
        {
            return Ok(Self::default());
        }
        let mut files = Vec::with_capacity(scopes.len());
        for file in scopes {
            let mut imports = HashMap::new();
            for import in &file.imports {
                let (alias, span) = import.alias;
                if imports
                    .insert(alias, import.path.qualified_name())
                    .is_some()
                {
                    return Err(Diagnostic::new(
                        "E0352",
                        span,
                        format!("duplicate import name '{alias}'"),
                    ));
                }
            }
            files.push(Scope {
                start: file.start,
                namespace: file.namespace.as_ref().map(|path| path.qualified_name()),
                imports,
            });
        }
        Ok(Self { files })
    }

    fn at(&self, span: Span) -> Option<&Scope<'src>> {
        let index = self
            .files
            .partition_point(|file| file.start <= span.start)
            .checked_sub(1)?;
        self.files.get(index)
    }

    pub(super) fn namespace(&self, span: Span) -> Option<&str> {
        self.at(span)?.namespace.as_deref()
    }

    pub(super) fn declaration<'name>(&self, name: &'name str, span: Span) -> Cow<'name, str> {
        if let Some(namespace) = self.namespace(span) {
            Cow::Owned(format!("{namespace}.{name}"))
        } else {
            Cow::Borrowed(name)
        }
    }

    pub(super) fn expand<'name>(&self, name: &'name str, span: Span) -> Cow<'name, str> {
        let Some(scope) = self.at(span) else {
            return Cow::Borrowed(name);
        };
        let (head, tail) = name.split_once('.').unwrap_or((name, ""));
        match scope.imports.get(head) {
            Some(prefix) if tail.is_empty() => Cow::Owned(prefix.clone()),
            Some(prefix) => Cow::Owned(format!("{prefix}.{tail}")),
            None => Cow::Borrowed(name),
        }
    }

    pub(super) fn lookup<'map, K, T>(
        &self,
        names: &'map HashMap<K, T>,
        name: &str,
        span: Span,
    ) -> Option<&'map T>
    where
        K: std::borrow::Borrow<str> + std::hash::Hash + Eq,
    {
        if !name.contains('.')
            && let Some(namespace) = self.namespace(span)
            && let Some(item) = names.get(format!("{namespace}.{name}").as_str())
        {
            return Some(item);
        }
        names.get(self.expand(name, span).as_ref())
    }
}

pub(super) fn path_name<'src>(segments: &[(&'src str, Span)]) -> Cow<'src, str> {
    match segments {
        [(name, _)] => Cow::Borrowed(name),
        _ => Cow::Owned(
            segments
                .iter()
                .map(|&(name, _)| name)
                .collect::<Vec<_>>()
                .join("."),
        ),
    }
}

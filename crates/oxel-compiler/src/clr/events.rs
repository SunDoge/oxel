//! Links CLR events to their public add and remove methods.

use super::{Invocation, Metadata, References, TableId, qualify};

#[derive(Clone, Copy)]
pub(crate) enum EventAccess {
    Add,
    Remove,
}

impl EventAccess {
    pub fn name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Remove => "remove",
        }
    }
}

pub(super) struct Event {
    pub owner: usize,
    pub invocation: Invocation,
    pub methods: [Option<usize>; 2],
}

impl References {
    pub(super) fn collect_events(
        &mut self,
        metadata: &Metadata,
        type_ids: &[Option<usize>],
    ) -> Vec<Option<(usize, EventAccess)>> {
        if metadata.event_maps.is_empty() {
            return Vec::new();
        }
        let mut methods = vec![None; metadata.method_defs.len()];
        let mut owners = vec![None; metadata.events.len()];
        for (index, map) in metadata.event_maps.iter().enumerate() {
            let Some(owner) = map
                .parent
                .checked_sub(1)
                .and_then(|row| type_ids.get(row as usize).copied().flatten())
            else {
                continue;
            };
            let end = metadata
                .event_maps
                .get(index + 1)
                .map_or(owners.len() + 1, |next| next.event_list as usize);
            if let Some(events) = map.event_list.checked_sub(1).and_then(|start| {
                end.checked_sub(1)
                    .and_then(|end| owners.get_mut(start as usize..end))
            }) {
                events.fill(Some(owner));
            }
        }
        let mut registered = vec![None; metadata.events.len()];
        for row in &metadata.method_semantics {
            if row.association.table != Some(TableId::Event) {
                continue;
            }
            let access = match row.semantics {
                0x08 => EventAccess::Add,
                0x10 => EventAccess::Remove,
                _ => continue,
            };
            let Some(index) = row
                .association
                .row
                .checked_sub(1)
                .map(|index| index as usize)
            else {
                continue;
            };
            let Some(owner) = owners.get(index).copied().flatten() else {
                continue;
            };
            let Ok(name) = metadata.strings.get(metadata.events[index].name) else {
                continue;
            };
            let Some(method) = row.method.checked_sub(1).map(|index| index as usize) else {
                continue;
            };
            let Some(definition) = metadata
                .method_defs
                .get(method)
                .filter(|method| method.flags & 7 == 6)
            else {
                continue;
            };
            let invocation = if definition.flags & 0x10 != 0 {
                Invocation::Static
            } else {
                Invocation::Instance
            };
            let id = *registered[index].get_or_insert_with(|| {
                let id = self.events.len();
                self.events.push(Event {
                    owner,
                    invocation,
                    methods: [None; 2],
                });
                let imported = &mut self.types[owner];
                let ids = if invocation == Invocation::Static {
                    self.static_events
                        .entry(qualify(&imported.qualified_name, name))
                        .or_default()
                } else {
                    imported.events.entry(name.into()).or_default()
                };
                ids.push(id);
                id
            });
            methods[method] = Some((id, access));
        }
        methods
    }

    pub(crate) fn declared_events(&self, owner: usize, name: &str) -> &[usize] {
        self.types[owner]
            .events
            .get(name)
            .map_or(&[], Vec::as_slice)
    }

    pub(crate) fn static_events(&self, name: &str) -> &[usize] {
        self.static_events.get(name).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn event_owner(&self, event: usize) -> usize {
        self.events[event].owner
    }

    pub(crate) fn event_accessor(&self, event: usize, access: EventAccess) -> Option<usize> {
        self.events[event].methods[access as usize]
    }
}

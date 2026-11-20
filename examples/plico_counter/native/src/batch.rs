use std::{collections::HashSet, ops::Range, str};

use plico::{
    UiContext,
    widgets::{button, column, row, text},
};

use crate::Command;

#[derive(Clone, Copy)]
enum Widget {
    Column,
    Row,
    Text,
    Button,
}

impl Widget {
    fn is_container(self) -> bool {
        matches!(self, Self::Column | Self::Row)
    }
}

struct Node {
    widget: Widget,
    key: i32,
    title: Range<usize>,
    disabled: bool,
    children: Vec<usize>,
}

pub(super) struct Batch {
    nodes: Vec<Node>,
    titles: String,
}

impl Batch {
    pub(super) fn decode(commands: &[Command], titles: &[u8]) -> Option<Self> {
        let titles = str::from_utf8(titles).ok()?;
        let mut nodes: Vec<Node> = Vec::with_capacity(commands.len());
        let mut keys = HashSet::new();
        for (index, command) in commands.iter().enumerate() {
            let widget = match command.kind {
                1 => Widget::Column,
                2 => Widget::Row,
                3 => Widget::Text,
                4 => Widget::Button,
                _ => return None,
            };
            if command.key <= 0 || !keys.insert(command.key) {
                return None;
            }
            let start = usize::try_from(command.title_start).ok()?;
            let length = usize::try_from(command.title_len).ok()?;
            let title = start..start.checked_add(length)?;
            titles.get(title.clone())?;
            if command.flags != 0 && (!matches!(widget, Widget::Button) || command.flags != 1) {
                return None;
            }
            if index == 0 {
                if command.parent != -1 || !widget.is_container() {
                    return None;
                }
            } else {
                let parent = nodes.get_mut(usize::try_from(command.parent).ok()?)?;
                if !parent.widget.is_container() {
                    return None;
                }
                parent.children.push(index);
            }
            nodes.push(Node {
                widget,
                key: command.key,
                title,
                disabled: command.flags == 1,
                children: Vec::new(),
            });
        }
        (!nodes.is_empty()).then(|| Self {
            nodes,
            titles: titles.into(),
        })
    }

    pub(super) fn show(&self, ui: &mut UiContext<'_>) -> Vec<i32> {
        let mut events = Vec::new();
        self.show_node(ui, 0, &mut events);
        events
    }

    fn show_node(&self, ui: &mut UiContext<'_>, index: usize, events: &mut Vec<i32>) {
        let node = &self.nodes[index];
        let key = node.key as u64;
        let title = &self.titles[node.title.clone()];
        match node.widget {
            Widget::Column | Widget::Row => {
                let layout = match node.widget {
                    Widget::Column => column(ui).padding(24).gap(16),
                    _ => row(ui).gap(8),
                };
                layout.key(key).children(|ui| {
                    for &child in &node.children {
                        self.show_node(ui, child, events);
                    }
                });
            }
            Widget::Text => {
                text(ui, title).key(key).font_size(30);
            }
            Widget::Button => {
                if button(ui)
                    .key(key)
                    .text(title)
                    .min_height(44)
                    .disabled(node.disabled)
                    .clicked()
                {
                    events.push(node.key);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_keys_topology_and_unicode_ranges() {
        let root = Command {
            kind: 1,
            key: 1,
            parent: -1,
            title_start: 0,
            title_len: 0,
            flags: 0,
        };
        let child = Command {
            kind: 3,
            key: 2,
            parent: 0,
            title_len: 2,
            ..root
        };
        let utf8 = "é".as_bytes();
        assert!(Batch::decode(&[root, child], utf8).is_some());
        for invalid in [
            Command { parent: 1, ..child },
            Command { key: 1, ..child },
            Command {
                title_start: 1,
                title_len: 1,
                ..child
            },
            Command {
                title_len: 3,
                ..child
            },
            Command { flags: 1, ..child },
        ] {
            assert!(Batch::decode(&[root, invalid], utf8).is_none());
        }
    }
}

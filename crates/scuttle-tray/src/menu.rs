//! An item's menu, as sent over the `com.canonical.dbusmenu` protocol.

use zbus::zvariant::Value;

/// A tick box or a radio button on a menu entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toggle {
    /// One of a group where only one can be on.
    pub radio: bool,
    pub on: bool,
}

/// One line of a menu.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MenuEntry {
    /// What the app calls this entry. Sent back when it is clicked.
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    /// A dividing line, not something to click.
    pub separator: bool,
    pub toggle: Option<Toggle>,
    /// A submenu.
    pub children: Vec<MenuEntry>,
}

/// Removes the underscore that marks a keyboard shortcut letter:
/// "_Quit" is shown as "Quit". A doubled underscore is a real one.
pub fn strip_mnemonic(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if chars.peek() == Some(&'_') {
                out.push('_');
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Unwraps a value sent inside a variant, however many deep.
fn inner<'a, 'v>(mut value: &'a Value<'v>) -> &'a Value<'v> {
    while let Value::Value(boxed) = value {
        value = boxed;
    }
    value
}

/// Reads one node of a menu layout, `(ia{sv}av)`, and everything under
/// it. Returns the entry and whether it is meant to be shown.
fn node(value: &Value<'_>, depth: usize) -> Option<(MenuEntry, bool)> {
    // A menu nested this deep is a mistake or an attack, not a menu.
    if depth > 8 {
        return None;
    }
    let Value::Structure(structure) = inner(value) else {
        return None;
    };
    let [id, properties, children] = structure.fields() else {
        return None;
    };
    let Value::I32(id) = inner(id) else {
        return None;
    };
    let mut entry = MenuEntry {
        id: *id,
        enabled: true,
        ..MenuEntry::default()
    };
    let mut visible = true;
    let mut toggle_type = "";
    let mut toggle_state = 0;
    if let Value::Dict(dict) = inner(properties) {
        for (key, value) in dict.iter() {
            let Value::Str(key) = inner(key) else {
                continue;
            };
            match (key.as_str(), inner(value)) {
                ("label", Value::Str(label)) => entry.label = strip_mnemonic(label.as_str()),
                ("enabled", Value::Bool(enabled)) => entry.enabled = *enabled,
                ("visible", Value::Bool(shown)) => visible = *shown,
                ("type", Value::Str(kind)) => entry.separator = kind.as_str() == "separator",
                ("toggle-type", Value::Str(kind)) => {
                    toggle_type = match kind.as_str() {
                        "checkmark" => "checkmark",
                        "radio" => "radio",
                        _ => "",
                    }
                }
                ("toggle-state", Value::I32(state)) => toggle_state = *state,
                _ => {}
            }
        }
    }
    if !toggle_type.is_empty() {
        entry.toggle = Some(Toggle {
            radio: toggle_type == "radio",
            on: toggle_state == 1,
        });
    }
    if let Value::Array(array) = inner(children) {
        // A sane limit on width, as on depth.
        for child in array.inner().iter().take(200) {
            if let Some((child, true)) = node(child, depth + 1) {
                entry.children.push(child);
            }
        }
    }
    entry.children = tidy(std::mem::take(&mut entry.children));
    Some((entry, visible))
}

/// Drops separators that divide nothing: at the top, at the bottom,
/// or straight after another one.
fn tidy(entries: Vec<MenuEntry>) -> Vec<MenuEntry> {
    let mut out: Vec<MenuEntry> = Vec::with_capacity(entries.len());
    for entry in entries {
        let redundant = entry.separator && out.last().is_none_or(|last| last.separator);
        if !redundant {
            out.push(entry);
        }
    }
    while out.last().is_some_and(|last| last.separator) {
        out.pop();
    }
    out
}

/// Reads a whole menu from the reply to `GetLayout`: the entries under
/// the root, in order, without the ones the app has hidden.
pub fn parse_layout(layout: &Value<'_>) -> Vec<MenuEntry> {
    node(layout, 0)
        .map(|(root, _)| root.children)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use zbus::zvariant::{Array, Signature, Structure};

    /// Builds a layout node the way an app sends it.
    fn layout(
        id: i32,
        properties: Vec<(&str, Value<'static>)>,
        children: Vec<Value<'static>>,
    ) -> Value<'static> {
        let properties: HashMap<String, Value<'static>> = properties
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect();
        let mut array = Array::new(&Signature::Variant);
        for child in children {
            array.append(Value::Value(Box::new(child))).unwrap();
        }
        Value::Structure(Structure::from((id, properties, array)))
    }

    fn item(id: i32, label: &str) -> Value<'static> {
        layout(id, vec![("label", Value::from(label.to_string()))], vec![])
    }

    fn separator(id: i32) -> Value<'static> {
        layout(id, vec![("type", Value::from("separator"))], vec![])
    }

    #[test]
    fn mnemonics_are_removed_and_real_underscores_kept() {
        assert_eq!(strip_mnemonic("_Quit"), "Quit");
        assert_eq!(strip_mnemonic("Open _Settings"), "Open Settings");
        assert_eq!(strip_mnemonic("snake__case"), "snake_case");
        assert_eq!(strip_mnemonic("Plain"), "Plain");
        assert_eq!(strip_mnemonic("_"), "");
    }

    #[test]
    fn a_flat_menu_is_read_in_order() {
        let root = layout(
            0,
            vec![("children-display", Value::from("submenu"))],
            vec![item(1, "_Open"), separator(2), item(3, "_Quit")],
        );
        let entries = parse_layout(&root);
        assert_eq!(entries.len(), 3);
        assert_eq!((entries[0].id, entries[0].label.as_str()), (1, "Open"));
        assert!(entries[0].enabled && !entries[0].separator);
        assert!(entries[1].separator);
        assert_eq!((entries[2].id, entries[2].label.as_str()), (3, "Quit"));
    }

    #[test]
    fn submenus_nest() {
        let root = layout(
            0,
            vec![],
            vec![layout(
                5,
                vec![
                    ("label", Value::from("Status")),
                    ("children-display", Value::from("submenu")),
                ],
                vec![item(6, "Online"), item(7, "Away")],
            )],
        );
        let entries = parse_layout(&root);
        assert_eq!(entries[0].label, "Status");
        let labels: Vec<&str> = entries[0]
            .children
            .iter()
            .map(|e| e.label.as_str())
            .collect();
        assert_eq!(labels, ["Online", "Away"]);
    }

    #[test]
    fn hidden_entries_are_left_out_and_disabled_ones_are_marked() {
        let root = layout(
            0,
            vec![],
            vec![
                layout(
                    1,
                    vec![
                        ("label", Value::from("Hidden")),
                        ("visible", Value::from(false)),
                    ],
                    vec![],
                ),
                layout(
                    2,
                    vec![
                        ("label", Value::from("Greyed")),
                        ("enabled", Value::from(false)),
                    ],
                    vec![],
                ),
            ],
        );
        let entries = parse_layout(&root);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Greyed");
        assert!(!entries[0].enabled);
    }

    #[test]
    fn ticks_and_radio_buttons_carry_their_state() {
        let root = layout(
            0,
            vec![],
            vec![
                layout(
                    1,
                    vec![
                        ("label", Value::from("Mute")),
                        ("toggle-type", Value::from("checkmark")),
                        ("toggle-state", Value::from(1i32)),
                    ],
                    vec![],
                ),
                layout(
                    2,
                    vec![
                        ("label", Value::from("Wi-Fi")),
                        ("toggle-type", Value::from("radio")),
                        ("toggle-state", Value::from(0i32)),
                    ],
                    vec![],
                ),
                item(3, "Plain"),
            ],
        );
        let entries = parse_layout(&root);
        assert_eq!(
            entries[0].toggle,
            Some(Toggle {
                radio: false,
                on: true
            })
        );
        assert_eq!(
            entries[1].toggle,
            Some(Toggle {
                radio: true,
                on: false
            })
        );
        assert_eq!(entries[2].toggle, None);
    }

    #[test]
    fn separators_that_divide_nothing_are_dropped() {
        let root = layout(
            0,
            vec![],
            vec![
                separator(1),
                item(2, "One"),
                separator(3),
                separator(4),
                // A hidden entry between two separators leaves them adjacent.
                layout(
                    5,
                    vec![
                        ("label", Value::from("Hidden")),
                        ("visible", Value::from(false)),
                    ],
                    vec![],
                ),
                separator(6),
                item(7, "Two"),
                separator(8),
            ],
        );
        let entries = parse_layout(&root);
        let shape: Vec<&str> = entries
            .iter()
            .map(|e| if e.separator { "-" } else { e.label.as_str() })
            .collect();
        assert_eq!(shape, ["One", "-", "Two"]);
    }

    #[test]
    fn nonsense_is_an_empty_menu_not_a_crash() {
        assert!(parse_layout(&Value::from(42i32)).is_empty());
        assert!(parse_layout(&Value::from("menu")).is_empty());
        // Wrong number of fields.
        let short = Value::Structure(Structure::from((1i32, 2i32)));
        assert!(parse_layout(&short).is_empty());
    }

    #[test]
    fn a_menu_nested_absurdly_deep_is_cut_off() {
        let mut deepest = item(100, "bottom");
        for id in (1..40).rev() {
            deepest = layout(id, vec![("label", Value::from("level"))], vec![deepest]);
        }
        let root = layout(0, vec![], vec![deepest]);
        // It parses without overflowing the stack, and stops descending.
        let mut depth = 0;
        let mut entries = parse_layout(&root);
        while let Some(first) = entries.into_iter().next() {
            depth += 1;
            entries = first.children;
        }
        assert!(depth <= 9, "went {depth} levels deep");
    }
}

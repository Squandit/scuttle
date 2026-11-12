//! Changing one setting in the config file without disturbing the rest
//! of it.
//!
//! The file belongs to the user. Their comments, spacing and ordering
//! are kept, and a change that would make the file invalid is refused
//! before anything is written.

use std::fs;
use std::io;
use std::path::Path;

use toml_edit::{Array, DocumentMut, Item, Table};

use crate::config::Config;

/// A setting's value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<String>),
}

impl Value {
    fn to_toml(&self) -> toml_edit::Value {
        match self {
            Value::Bool(v) => (*v).into(),
            Value::Int(v) => (*v).into(),
            // Sliders land on values like 1.2000000000000002. Two
            // decimal places is as fine as any setting goes.
            Value::Float(v) => ((v * 100.0).round() / 100.0).into(),
            Value::Str(v) => v.as_str().into(),
            Value::List(items) => {
                toml_edit::Value::Array(items.iter().map(String::as_str).collect::<Array>())
            }
        }
    }

    /// The value as it is written in the file: `"tide"`, `1.25`, `true`.
    pub fn written(&self) -> String {
        self.to_toml().to_string().trim().to_string()
    }
}

/// Splits `section.name` into its two halves.
fn split(key: &str) -> Result<(&str, &str), String> {
    key.split_once('.')
        .filter(|(section, name)| !section.is_empty() && !name.is_empty() && !name.contains('.'))
        .ok_or_else(|| format!("\"{key}\" is not a setting."))
}

fn parse(text: &str) -> Result<DocumentMut, String> {
    text.parse::<DocumentMut>()
        .map_err(|e| format!("The config file has a mistake in it, so it was not changed: {e}"))
}

/// Checks the edited file is one Scuttle would accept.
fn checked(doc: DocumentMut) -> Result<String, String> {
    let text = doc.to_string();
    Config::parse(&text).map_err(|e| {
        // The error names a file. Here there is only the one.
        e.to_string()
    })?;
    Ok(text)
}

/// The line a value is written as, for showing beside its control:
/// `[appearance] theme = "tide"`.
pub fn line(key: &str, value: Option<&Value>) -> String {
    let (section, name) = key.split_once('.').unwrap_or(("", key));
    match value {
        Some(value) => format!("[{section}] {name} = {}", value.written()),
        None => format!("[{section}] {name} is not set"),
    }
}

/// Returns `text` with `key` set to `value`. Everything else in the
/// file, comments included, stays as it was.
pub fn set(text: &str, key: &str, value: &Value) -> Result<String, String> {
    let (section, name) = split(key)?;
    let mut doc = parse(text)?;
    if !doc.contains_key(section) {
        // Comments at the end of the file belong above a section added
        // after them, not below it. Without this, a file that is only
        // a comment would have the comment pushed to the bottom.
        let trailing = doc.trailing().as_str().unwrap_or("").to_string();
        let mut table = Table::new();
        if !trailing.trim().is_empty() {
            let mut prefix = trailing;
            if !prefix.ends_with('\n') {
                prefix.push('\n');
            }
            if !doc.is_empty() || !prefix.ends_with("\n\n") {
                prefix.push('\n');
            }
            table.decor_mut().set_prefix(prefix);
            doc.set_trailing("");
        }
        doc.insert(section, Item::Table(table));
    }
    let table = doc
        .get_mut(section)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| format!("[{section}] in the config file is not a table."))?;
    let mut new = value.to_toml();
    // Keep a comment that sits on the same line as the old value.
    if let Some(old) = table.get(name).and_then(Item::as_value) {
        *new.decor_mut() = old.decor().clone();
    }
    table.insert(name, Item::Value(new));
    checked(doc)
}

/// Returns `text` with `key` removed, which puts it back to its default.
pub fn unset(text: &str, key: &str) -> Result<String, String> {
    let (section, name) = split(key)?;
    let mut doc = parse(text)?;
    if let Some(table) = doc.get_mut(section).and_then(Item::as_table_mut) {
        table.remove(name);
    }
    checked(doc)
}

/// Whether the file sets `key` itself, rather than leaving the default.
pub fn is_set(text: &str, key: &str) -> bool {
    let Ok((section, name)) = split(key) else {
        return false;
    };
    text.parse::<DocumentMut>()
        .ok()
        .and_then(|doc| doc.get(section)?.as_table()?.get(name).map(|_| ()))
        .is_some()
}

/// The value `key` has in `config`. `None` for an optional setting
/// that is not set.
pub fn get(config: &Config, key: &str) -> Option<Value> {
    let (section, name) = split(key).ok()?;
    let table = toml::Table::try_from(config).ok()?;
    match table.get(section)?.as_table()?.get(name)? {
        toml::Value::Boolean(v) => Some(Value::Bool(*v)),
        toml::Value::Integer(v) => Some(Value::Int(*v)),
        toml::Value::Float(v) => Some(Value::Float((v * 100.0).round() / 100.0)),
        toml::Value::String(v) => Some(Value::Str(v.clone())),
        toml::Value::Array(items) => Some(Value::List(
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect(),
        )),
        _ => None,
    }
}

/// Splits what was typed into a list box: words separated by commas.
pub fn parse_list(typed: &str) -> Vec<String> {
    typed
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// Reads the config file as text. A missing file is an empty one.
pub fn read(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e),
    }
}

/// Writes the config file in one step, so a reader never sees half of
/// it: the text goes to a neighbouring file which then takes its place.
pub fn write(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // A config that is a symlink (dotfile managers do this) is written
    // through, not replaced by a plain file.
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut temp = target.clone().into_os_string();
    temp.push(".scuttle-new");
    let temp = Path::new(&temp);
    fs::write(temp, text)?;
    fs::rename(temp, &target).inspect_err(|_| {
        let _ = fs::remove_file(temp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"# My Scuttle config.

[appearance]
theme = "ferris"   # the orange one
# I like it big.
scale = 1.5

[bar]
style = "islands"
right = ["cpu", "memory"]
"#;

    fn s(text: &str) -> Value {
        Value::Str(text.to_string())
    }

    #[test]
    fn changing_a_value_keeps_every_comment_and_line() {
        let out = set(FILE, "appearance.theme", &s("tide")).unwrap();
        assert_eq!(out, FILE.replace("\"ferris\"", "\"tide\""));
    }

    #[test]
    fn a_new_setting_joins_its_section() {
        let out = set(FILE, "appearance.radius", &Value::Int(8)).unwrap();
        assert!(out.contains("scale = 1.5\nradius = 8\n\n[bar]"), "{out}");
        assert!(out.starts_with("# My Scuttle config."));
        assert_eq!(Config::parse(&out).unwrap().appearance.radius, 8);
    }

    #[test]
    fn a_new_section_is_added_when_needed() {
        let out = set(FILE, "sound.feedback", &s("always")).unwrap();
        assert!(out.starts_with(FILE), "{out}");
        assert!(out.ends_with("[sound]\nfeedback = \"always\"\n"), "{out}");
    }

    #[test]
    fn comments_stay_above_a_section_added_after_them() {
        // A file that is nothing but a comment.
        assert_eq!(
            set("# Mine.\n", "bar.position", &s("bottom")).unwrap(),
            "# Mine.\n\n[bar]\nposition = \"bottom\"\n"
        );
        // Without a final newline.
        assert_eq!(
            set("# Mine.", "bar.position", &s("bottom")).unwrap(),
            "# Mine.\n\n[bar]\nposition = \"bottom\"\n"
        );
        // A comment after the last section stays where it was, and the
        // new section goes below it.
        let file = "[bar]\nstyle = \"strip\"\n\n# Sound next.\n";
        assert_eq!(
            set(file, "sound.feedback", &s("always")).unwrap(),
            "[bar]\nstyle = \"strip\"\n\n# Sound next.\n\n[sound]\nfeedback = \"always\"\n"
        );
        // Adding to a section that exists leaves trailing comments be.
        assert_eq!(
            set(file, "bar.height", &Value::Int(44)).unwrap(),
            "[bar]\nstyle = \"strip\"\nheight = 44\n\n# Sound next.\n"
        );
    }

    #[test]
    fn an_empty_file_gets_just_the_one_setting() {
        assert_eq!(
            set("", "bar.position", &s("bottom")).unwrap(),
            "[bar]\nposition = \"bottom\"\n"
        );
    }

    #[test]
    fn every_kind_of_value_is_written_properly() {
        assert_eq!(Value::Bool(false).written(), "false");
        assert_eq!(Value::Int(42).written(), "42");
        assert_eq!(Value::Float(1.25).written(), "1.25");
        assert_eq!(Value::Float(1.2000000000000002).written(), "1.2");
        assert_eq!(Value::Float(2.0).written(), "2.0");
        assert_eq!(s("a \"quoted\" word").written(), r#"'a "quoted" word'"#);
        assert_eq!(
            Value::List(vec!["cpu".into(), "clock".into()]).written(),
            r#"["cpu", "clock"]"#
        );
        let out = set(FILE, "bar.left", &Value::List(vec!["clock".into()])).unwrap();
        assert_eq!(
            Config::parse(&out).unwrap().bar.left,
            vec![crate::config::Module::Clock]
        );
    }

    #[test]
    fn a_change_that_would_break_the_config_is_refused() {
        for (key, value) in [
            ("appearance.theme", s("nope")),
            ("appearance.scale", Value::Float(9.0)),
            ("bar.left", Value::List(vec!["spaceship".into()])),
            ("appearance.accent", s("not a colour")),
            ("bar.typo", Value::Int(1)),
            ("nonsense", Value::Int(1)),
            ("a.b.c", Value::Int(1)),
        ] {
            assert!(set(FILE, key, &value).is_err(), "{key} should be refused");
        }
    }

    #[test]
    fn a_file_with_a_mistake_is_left_alone() {
        let broken = "[appearance\ntheme = ";
        let error = set(broken, "appearance.theme", &s("tide")).unwrap_err();
        assert!(error.contains("was not changed"), "{error}");
    }

    #[test]
    fn unsetting_goes_back_to_the_default() {
        let out = unset(FILE, "appearance.scale").unwrap();
        assert!(!out.contains("scale"));
        assert!(out.contains("theme = \"ferris\"   # the orange one"));
        assert_eq!(Config::parse(&out).unwrap().appearance.scale, 1.25);
        // Unsetting what is not there changes nothing.
        assert_eq!(unset(FILE, "sound.volume").unwrap(), FILE);
    }

    #[test]
    fn knows_what_the_file_sets_itself() {
        assert!(is_set(FILE, "appearance.scale"));
        assert!(is_set(FILE, "bar.right"));
        assert!(!is_set(FILE, "appearance.radius"));
        assert!(!is_set(FILE, "sound.volume"));
        assert!(!is_set("", "appearance.scale"));
    }

    #[test]
    fn reads_current_values_from_a_config() {
        let config = Config::parse(FILE).unwrap();
        assert_eq!(get(&config, "appearance.theme"), Some(s("ferris")));
        assert_eq!(get(&config, "appearance.scale"), Some(Value::Float(1.5)));
        assert_eq!(get(&config, "appearance.blur"), Some(Value::Bool(true)));
        assert_eq!(get(&config, "bar.height"), Some(Value::Int(40)));
        assert_eq!(
            get(&config, "bar.right"),
            Some(Value::List(vec!["cpu".into(), "memory".into()]))
        );
        assert_eq!(get(&config, "appearance.accent"), None);
        assert_eq!(get(&config, "bar.nonsense"), None);
    }

    #[test]
    fn default_scale_reads_back_without_float_noise() {
        // 1.25 is stored as a 32-bit float; it must not come back as
        // 1.2500000001 or the like.
        assert_eq!(
            get(&Config::default(), "appearance.scale"),
            Some(Value::Float(1.25))
        );
        assert_eq!(
            get(&Config::default(), "sound.volume"),
            Some(Value::Float(0.25))
        );
    }

    #[test]
    fn the_line_shown_beside_a_control() {
        assert_eq!(
            line("appearance.theme", Some(&s("tide"))),
            "[appearance] theme = \"tide\""
        );
        assert_eq!(
            line("bar.height", Some(&Value::Int(44))),
            "[bar] height = 44"
        );
        assert_eq!(
            line("appearance.accent", None),
            "[appearance] accent is not set"
        );
    }

    #[test]
    fn typed_lists_are_split_on_commas() {
        assert_eq!(
            parse_list(" cpu, memory ,,clock "),
            ["cpu", "memory", "clock"]
        );
        assert!(parse_list("  ").is_empty());
    }

    #[test]
    fn writing_replaces_the_file_in_one_step_and_follows_symlinks() {
        let dir = std::env::temp_dir().join(format!("scuttle-edit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let real = dir.join("dotfiles/config.toml");
        let link = dir.join("config/scuttle/config.toml");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();

        write(&link, "new").unwrap();
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!real.with_file_name("config.toml.scuttle-new").exists());

        // A file that does not exist yet is created, folders and all.
        let fresh = dir.join("fresh/scuttle/config.toml");
        assert_eq!(read(&fresh).unwrap(), "");
        write(&fresh, "[bar]\n").unwrap();
        assert_eq!(read(&fresh).unwrap(), "[bar]\n");
    }
}

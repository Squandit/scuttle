//! Turns a query into a ranked, grouped list of results.

use std::path::{Path, PathBuf};

use nucleo_matcher::{Config as MatcherConfig, Matcher};

use crate::calc;
use crate::clipboard::ClipEntry;
use crate::desktop::{exec_command, DesktopEntry};
use crate::files::{display_path, FileEntry};
use crate::history::History;
use crate::item::{Action, Internal, Item, Kind};
use crate::matching::{Match, Query};

/// Which providers a query is sent to. A prefix character at the start
/// of the query selects a single mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// No prefix: search everything.
    All,
    /// `>` system commands.
    System,
    /// `/` files.
    Files,
    /// `?` web search.
    Web,
    /// `=` calculator.
    Calc,
    /// `@` open windows.
    Windows,
    /// `;` clipboard history.
    Clipboard,
    /// `:` emoji.
    Emoji,
}

impl Mode {
    /// Splits a raw query into its mode and the text to search for.
    pub fn parse(query: &str) -> (Mode, &str) {
        let query = query.trim_start();
        let mode = match query.chars().next() {
            Some('>') => Mode::System,
            Some('/') => Mode::Files,
            Some('?') => Mode::Web,
            Some('=') => Mode::Calc,
            Some('@') => Mode::Windows,
            Some(';') => Mode::Clipboard,
            Some(':') => Mode::Emoji,
            _ => return (Mode::All, query.trim()),
        };
        (mode, query[1..].trim())
    }

    /// The label shown next to the search box while a mode is active.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Mode::All => None,
            Mode::System => Some("System"),
            Mode::Files => Some("Files"),
            Mode::Web => Some("Web"),
            Mode::Calc => Some("Calculator"),
            Mode::Windows => Some("Windows"),
            Mode::Clipboard => Some("Clipboard"),
            Mode::Emoji => Some("Emoji"),
        }
    }
}

/// An open window, as much as the launcher needs to know about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub address: String,
    pub class: String,
    pub title: String,
    pub workspace: String,
}

struct SystemCommand {
    title: &'static str,
    keywords: &'static str,
    icon: &'static str,
    action: fn() -> Action,
    confirm: bool,
    key: &'static str,
}

const SYSTEM_COMMANDS: &[SystemCommand] = &[
    SystemCommand {
        title: "Lock screen",
        keywords: "lock",
        icon: "system-lock-screen-symbolic",
        action: || Action::Shell("loginctl lock-session".into()),
        confirm: false,
        key: "sys:lock",
    },
    SystemCommand {
        title: "Suspend",
        keywords: "sleep standby",
        icon: "weather-clear-night-symbolic",
        action: || Action::Shell("systemctl suspend".into()),
        confirm: false,
        key: "sys:suspend",
    },
    SystemCommand {
        title: "Log out",
        keywords: "logout exit sign out session",
        icon: "system-log-out-symbolic",
        action: || Action::Internal(Internal::Logout),
        confirm: true,
        key: "sys:logout",
    },
    SystemCommand {
        title: "Restart",
        keywords: "reboot",
        icon: "system-reboot-symbolic",
        action: || Action::Shell("systemctl reboot".into()),
        confirm: true,
        key: "sys:reboot",
    },
    SystemCommand {
        title: "Shut down",
        keywords: "power off poweroff shutdown halt",
        icon: "system-shutdown-symbolic",
        action: || Action::Shell("systemctl poweroff".into()),
        confirm: true,
        key: "sys:poweroff",
    },
    SystemCommand {
        title: "Do not disturb",
        keywords: "dnd quiet silence mute notifications focus",
        icon: "notifications-disabled-symbolic",
        action: || Action::Internal(Internal::ToggleDnd),
        confirm: false,
        key: "sys:dnd",
    },
    SystemCommand {
        title: "Settings",
        keywords: "scuttle preferences config options theme appearance",
        icon: "preferences-system-symbolic",
        action: || Action::Internal(Internal::Settings),
        confirm: false,
        key: "sys:settings",
    },
    SystemCommand {
        title: "Assistant",
        keywords: "claude ai ask chat help",
        icon: "dialog-question-symbolic",
        action: || Action::Internal(Internal::Assistant),
        confirm: false,
        key: "sys:assistant",
    },
    SystemCommand {
        title: "Reload Scuttle",
        keywords: "refresh config restart shell",
        icon: "view-refresh-symbolic",
        action: || Action::Internal(Internal::Reload),
        confirm: false,
        key: "sys:reload",
    },
    SystemCommand {
        title: "Quit Scuttle",
        keywords: "exit stop shell",
        icon: "window-close-symbolic",
        action: || Action::Internal(Internal::Quit),
        confirm: true,
        key: "sys:quit",
    },
];

/// How many results each kind may contribute when searching everything.
const APP_CAP: usize = 5;
const WINDOW_CAP: usize = 4;
const SYSTEM_CAP: usize = 3;
const FILE_CAP: usize = 4;

/// Results scoring below this fraction of the best result of their kind
/// are dropped as noise.
const RELATIVE_CUTOFF: f32 = 0.35;

/// The launcher's search engine.
pub struct Engine {
    apps: Vec<DesktopEntry>,
    windows: Vec<WindowInfo>,
    files: Vec<FileEntry>,
    clipboard: Vec<ClipEntry>,
    history: History,
    web_template: String,
    max_results: usize,
    home: PathBuf,
    matcher: Matcher,
    path_matcher: Matcher,
}

impl Engine {
    pub fn new(web_template: &str, max_results: usize, home: &Path, history: History) -> Engine {
        Engine {
            apps: Vec::new(),
            windows: Vec::new(),
            files: Vec::new(),
            clipboard: Vec::new(),
            history,
            web_template: web_template.to_string(),
            max_results: max_results.max(1),
            home: home.to_path_buf(),
            matcher: Matcher::new(MatcherConfig::DEFAULT),
            path_matcher: Matcher::new(MatcherConfig::DEFAULT.match_paths()),
        }
    }

    pub fn set_apps(&mut self, apps: Vec<DesktopEntry>) {
        self.apps = apps;
    }

    pub fn set_windows(&mut self, windows: Vec<WindowInfo>) {
        self.windows = windows;
    }

    pub fn set_files(&mut self, files: Vec<FileEntry>) {
        self.files = files;
    }

    /// Replaces the clipboard history, newest first.
    pub fn set_clipboard(&mut self, entries: Vec<ClipEntry>) {
        self.clipboard = entries;
    }

    pub fn set_options(&mut self, web_template: &str, max_results: usize) {
        self.web_template = web_template.to_string();
        self.max_results = max_results.max(1);
    }

    /// Notes that a result was chosen, so it ranks higher next time.
    pub fn record_use(&mut self, item: &Item, now: u64) {
        // Windows and one-off results are not worth remembering.
        if matches!(
            item.kind,
            Kind::App | Kind::System | Kind::File | Kind::Emoji
        ) {
            self.history.record(&item.key, now);
            let _ = self.history.save(now);
        }
    }

    /// Searches for `query`. `now` is seconds since the Unix epoch and is
    /// only used to weigh how recently things were used.
    ///
    /// Results come back grouped by kind. The group containing the best
    /// match is first, a calculator result always leads, and the web
    /// search fallback always trails.
    pub fn search(&mut self, query: &str, now: u64) -> Vec<Item> {
        let (mode, text) = Mode::parse(query);
        let limit = self.max_results;

        if text.is_empty() {
            return match mode {
                Mode::All => self.frequent_apps(now, limit),
                Mode::Windows => self.all_windows(limit),
                Mode::System => self.all_system(limit),
                Mode::Clipboard => self.recent_clipboard(limit),
                Mode::Emoji => self.frequent_emoji(now, limit),
                Mode::Files | Mode::Web | Mode::Calc => Vec::new(),
            };
        }

        match mode {
            Mode::Calc => self.calc(text).into_iter().collect(),
            Mode::Web => vec![self.web(text)],
            Mode::System => settle_alone(self.match_system(text), limit),
            Mode::Windows => settle_alone(self.match_windows(text), limit),
            Mode::Files => settle_alone(self.match_files(text), limit),
            Mode::Clipboard => settle_alone(self.match_clipboard(text), limit),
            Mode::Emoji => settle_alone(self.match_emoji(text, now), limit),
            Mode::All => {
                let mut candidates = vec![
                    (self.match_apps(text, now), APP_CAP),
                    (self.match_windows(text), WINDOW_CAP),
                    (self.match_system(text), SYSTEM_CAP),
                ];
                if text.chars().count() >= 2 {
                    candidates.push((self.match_files(text), FILE_CAP));
                }
                // Loose matches are only worth showing when nothing
                // anywhere matched properly.
                let any_strict = candidates.iter().any(|(c, _)| c.iter().any(|s| s.strict));
                let mut groups: Vec<Vec<Item>> = candidates
                    .into_iter()
                    .map(|(c, cap)| settle(c, any_strict, cap))
                    .filter(|g| !g.is_empty())
                    .collect();
                // The group holding the strongest match goes first.
                groups.sort_by_key(|g| std::cmp::Reverse(g[0].score));

                let mut out = Vec::new();
                if calc::looks_like_math(text) {
                    out.extend(self.calc(text));
                }
                let room = limit.saturating_sub(out.len() + 1);
                out.extend(interleave_fairly(groups, room));
                out.push(self.web(text));
                out
            }
        }
    }

    fn app_item(entry: &DesktopEntry, score: u32) -> Item {
        let subtitle = if !entry.generic_name.is_empty() {
            entry.generic_name.clone()
        } else {
            entry.comment.clone()
        };
        Item {
            kind: Kind::App,
            title: entry.name.clone(),
            subtitle,
            icon: entry.icon.clone(),
            action: Action::Launch {
                exec: exec_command(&entry.exec),
                terminal: entry.terminal,
            },
            confirm: false,
            key: format!("app:{}", entry.id),
            score,
        }
    }

    fn frequent_apps(&self, now: u64, limit: usize) -> Vec<Item> {
        let mut items: Vec<Item> = self
            .apps
            .iter()
            .map(|e| Self::app_item(e, self.history.boost(&format!("app:{}", e.id), now)))
            .collect();
        // Apps arrive sorted by name; a stable sort keeps that order
        // among apps used equally often.
        items.sort_by_key(|i| std::cmp::Reverse(i.score));
        items.truncate(limit);
        items
    }

    fn match_apps(&mut self, text: &str, now: u64) -> Vec<Scored> {
        let query = Query::new(text);
        let lowered = text.to_lowercase();
        let mut items = Vec::new();
        for entry in &self.apps {
            let by_name = query.score(&entry.name, &mut self.matcher);
            let extra = format!(
                "{} {} {}",
                entry.generic_name,
                entry.keywords.join(" "),
                entry.id
            );
            let by_extra = query.score(&extra, &mut self.matcher).map(|m| scaled(m, 6));
            let Some(best) = better(by_name, by_extra) else {
                continue;
            };
            let mut score = best.score;
            // Typing the start of the name is the strongest signal.
            if entry.name.to_lowercase().starts_with(&lowered) {
                score += 60;
            }
            score += self.history.boost(&format!("app:{}", entry.id), now);
            items.push(Scored {
                item: Self::app_item(entry, score),
                strict: best.strict,
            });
        }
        items
    }

    fn window_item(window: &WindowInfo, score: u32) -> Item {
        Item {
            kind: Kind::Window,
            title: window.title.clone(),
            subtitle: format!(
                "{}, workspace {}",
                pretty_class(&window.class),
                window.workspace
            ),
            icon: Some(window.class.to_lowercase()),
            action: Action::FocusWindow {
                address: window.address.clone(),
            },
            confirm: false,
            key: format!("win:{}", window.address),
            score,
        }
    }

    fn all_windows(&self, limit: usize) -> Vec<Item> {
        self.windows
            .iter()
            .take(limit)
            .map(|w| Self::window_item(w, 0))
            .collect()
    }

    fn match_windows(&mut self, text: &str) -> Vec<Scored> {
        let query = Query::new(text);
        let mut items = Vec::new();
        for window in &self.windows {
            let haystack = format!("{} {}", window.title, window.class);
            if let Some(m) = query.score(&haystack, &mut self.matcher) {
                items.push(Scored {
                    item: Self::window_item(window, m.score * 9 / 10),
                    strict: m.strict,
                });
            }
        }
        items
    }

    fn system_item(command: &SystemCommand, score: u32) -> Item {
        Item {
            kind: Kind::System,
            title: command.title.to_string(),
            subtitle: String::new(),
            icon: Some(command.icon.to_string()),
            action: (command.action)(),
            confirm: command.confirm,
            key: command.key.to_string(),
            score,
        }
    }

    fn all_system(&self, limit: usize) -> Vec<Item> {
        SYSTEM_COMMANDS
            .iter()
            .take(limit)
            .map(|c| Self::system_item(c, 0))
            .collect()
    }

    fn match_system(&mut self, text: &str) -> Vec<Scored> {
        let query = Query::new(text);
        let mut items = Vec::new();
        for command in SYSTEM_COMMANDS {
            let by_title = query.score(command.title, &mut self.matcher);
            let by_keyword = query
                .score(command.keywords, &mut self.matcher)
                .map(|m| scaled(m, 8));
            if let Some(best) = better(by_title, by_keyword) {
                items.push(Scored {
                    item: Self::system_item(command, best.score * 95 / 100),
                    strict: best.strict,
                });
            }
        }
        items
    }

    fn match_files(&mut self, text: &str) -> Vec<Scored> {
        let query = Query::new(text);
        let mut items = Vec::new();
        for file in &self.files {
            let Some(m) = query.score(&file.name, &mut self.path_matcher) else {
                continue;
            };
            let parent = file.path.parent().unwrap_or(Path::new("/"));
            let item = Item {
                kind: Kind::File,
                title: file.name.clone(),
                subtitle: display_path(parent, &self.home),
                icon: Some(
                    if file.is_dir {
                        "folder-symbolic"
                    } else {
                        "text-x-generic-symbolic"
                    }
                    .into(),
                ),
                action: Action::OpenPath(file.path.clone()),
                confirm: false,
                key: format!("file:{}", file.path.display()),
                score: m.score * 8 / 10,
            };
            items.push(Scored {
                item,
                strict: m.strict,
            });
        }
        items
    }

    fn clip_item(entry: &ClipEntry, score: u32) -> Item {
        Item {
            kind: Kind::Clipboard,
            title: entry.preview.clone(),
            subtitle: String::new(),
            icon: Some(
                if entry.is_image {
                    "image-x-generic-symbolic"
                } else {
                    "edit-paste-symbolic"
                }
                .into(),
            ),
            action: Action::RestoreClip(entry.line.clone()),
            confirm: false,
            key: "clip".into(),
            score,
        }
    }

    fn recent_clipboard(&self, limit: usize) -> Vec<Item> {
        self.clipboard
            .iter()
            .take(limit)
            .map(|e| Self::clip_item(e, 0))
            .collect()
    }

    fn match_clipboard(&mut self, text: &str) -> Vec<Scored> {
        let query = Query::new(text);
        let mut items = Vec::new();
        // Entries are newest first. A small bonus for recency settles
        // ties between equally good matches.
        for (age, entry) in self.clipboard.iter().enumerate() {
            if let Some(m) = query.score(&entry.preview, &mut self.matcher) {
                let recency = 20u32.saturating_sub(age as u32);
                items.push(Scored {
                    item: Self::clip_item(entry, m.score + recency),
                    strict: m.strict,
                });
            }
        }
        items
    }

    fn emoji_item(emoji: crate::emoji::Emoji, score: u32) -> Item {
        Item {
            kind: Kind::Emoji,
            title: format!(
                "{}  {}",
                emoji.character,
                crate::emoji::display_name(emoji.name)
            ),
            subtitle: String::new(),
            icon: None,
            action: Action::Copy(emoji.character.to_string()),
            confirm: false,
            key: format!("emoji:{}", emoji.name),
            score,
        }
    }

    /// With nothing typed, the emoji used most and most recently come
    /// first, then the rest in Unicode's order.
    fn frequent_emoji(&self, now: u64, limit: usize) -> Vec<Item> {
        let mut items: Vec<Item> = crate::emoji::all()
            .map(|e| Self::emoji_item(e, self.history.boost(&format!("emoji:{}", e.name), now)))
            .collect();
        items.sort_by_key(|i| std::cmp::Reverse(i.score));
        items.truncate(limit);
        items
    }

    fn match_emoji(&mut self, text: &str, now: u64) -> Vec<Scored> {
        let query = Query::new(text);
        let lowered = text.to_lowercase();
        let mut items = Vec::new();
        for emoji in crate::emoji::all() {
            let Some(m) = query.score(emoji.name, &mut self.matcher) else {
                continue;
            };
            let mut score = m.score;
            // "fire" should find fire before firefighter.
            if emoji.name == lowered {
                score += 100;
            } else if emoji.name.starts_with(&lowered) {
                score += 40;
            }
            score += self.history.boost(&format!("emoji:{}", emoji.name), now);
            items.push(Scored {
                item: Self::emoji_item(emoji, score),
                strict: m.strict,
            });
        }
        items
    }

    fn calc(&self, text: &str) -> Option<Item> {
        let value = calc::evaluate(text).ok()?;
        let result = calc::format(value);
        Some(Item {
            kind: Kind::Calculator,
            title: result.clone(),
            subtitle: "Enter copies".into(),
            icon: Some("accessories-calculator-symbolic".into()),
            action: Action::Copy(result),
            confirm: false,
            key: "calc".into(),
            score: u32::MAX,
        })
    }

    fn web(&self, text: &str) -> Item {
        if let Some(url) = as_url(text) {
            return Item {
                kind: Kind::Web,
                title: format!("Open {text}"),
                subtitle: "Browser".into(),
                icon: Some("web-browser-symbolic".into()),
                action: Action::OpenUrl(url),
                confirm: false,
                key: "web:url".into(),
                score: 0,
            };
        }
        Item {
            kind: Kind::Web,
            title: format!("Search the web for \"{text}\""),
            subtitle: String::new(),
            icon: Some("system-search-symbolic".into()),
            action: Action::OpenUrl(self.web_template.replace("{query}", &percent_encode(text))),
            confirm: false,
            key: "web:search".into(),
            score: 0,
        }
    }
}

/// A result together with how convincing its match was.
struct Scored {
    item: Item,
    strict: bool,
}

/// Scales a match found in secondary text (keywords, generic names)
/// down to `tenths` of its score, so name matches win.
fn scaled(m: Match, tenths: u32) -> Match {
    Match {
        score: m.score * tenths / 10,
        ..m
    }
}

/// The better of two optional matches: strict beats loose, then score.
fn better(a: Option<Match>, b: Option<Match>) -> Option<Match> {
    [a, b]
        .into_iter()
        .flatten()
        .max_by_key(|m| (m.strict, m.score))
}

/// Turns one kind's candidates into its final rows. With `strict_only`
/// set, loose matches are discarded.
fn settle(candidates: Vec<Scored>, strict_only: bool, cap: usize) -> Vec<Item> {
    let items = candidates
        .into_iter()
        .filter(|c| c.strict || !strict_only)
        .map(|c| c.item)
        .collect();
    finish(items, cap)
}

/// [`settle`] for a search of a single kind: loose matches show only
/// when that kind has no strict ones.
fn settle_alone(candidates: Vec<Scored>, cap: usize) -> Vec<Item> {
    let any_strict = candidates.iter().any(|c| c.strict);
    settle(candidates, any_strict, cap)
}

/// Sorts a kind's matches, drops weak ones and applies the cap.
fn finish(mut items: Vec<Item>, cap: usize) -> Vec<Item> {
    items.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
    if let Some(best) = items.first().map(|i| i.score) {
        let floor = (best as f32 * RELATIVE_CUTOFF) as u32;
        items.retain(|i| i.score >= floor);
    }
    items.truncate(cap);
    items
}

/// Fits several groups into `room` rows without letting the first group
/// crowd out the rest: each group keeps at least its best result while
/// there is space, and remaining rows are handed out in group order.
fn interleave_fairly(groups: Vec<Vec<Item>>, room: usize) -> Vec<Item> {
    let mut quota = vec![0usize; groups.len()];
    let mut left = room;
    for (i, _) in groups.iter().enumerate() {
        if left == 0 {
            break;
        }
        quota[i] = 1;
        left -= 1;
    }
    for (i, group) in groups.iter().enumerate() {
        let extra = (group.len() - quota[i].min(group.len())).min(left);
        if quota[i] > 0 {
            quota[i] += extra;
            left -= extra;
        }
    }
    groups
        .into_iter()
        .zip(quota)
        .flat_map(|(group, take)| group.into_iter().take(take))
        .collect()
}

/// Turns a window class such as `org.gnome.Nautilus` into a readable
/// name such as `Nautilus`.
pub fn pretty_class(class: &str) -> String {
    let last = class.rsplit('.').next().unwrap_or(class);
    let mut chars = last.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Top-level domains common enough that "name.tld" is clearly a site
/// and not a file name.
const COMMON_TLDS: &[&str] = &[
    "com", "org", "net", "io", "dev", "app", "co", "uk", "de", "fr", "nl", "gg", "me", "sh", "rs",
    "edu", "gov", "wiki", "xyz", "tv", "land", "ai", "ca", "au", "eu",
];

/// If `text` is clearly a web address, returns it as a full URL.
fn as_url(text: &str) -> Option<String> {
    if text.is_empty() || text.contains(char::is_whitespace) {
        return None;
    }
    if text.starts_with("http://") || text.starts_with("https://") {
        return Some(text.to_string());
    }
    let host = text.split(['/', '?', '#']).next().unwrap_or(text);
    let host = host.split(':').next().unwrap_or(host);
    let (name, tld) = host.rsplit_once('.')?;
    let name_ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !name.starts_with('.');
    if name_ok && COMMON_TLDS.contains(&tld.to_ascii_lowercase().as_str()) {
        Some(format!("https://{text}"))
    } else {
        None
    }
}

/// Percent-encodes text for use in a URL query string.
pub fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn app(id: &str, name: &str, generic: &str, keywords: &[&str]) -> DesktopEntry {
        DesktopEntry {
            id: id.into(),
            name: name.into(),
            generic_name: generic.into(),
            comment: String::new(),
            exec: format!("{id} %U"),
            icon: Some(id.into()),
            terminal: false,
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn engine() -> Engine {
        let home = Path::new("/home/me");
        let mut e = Engine::new(
            "https://search.example/?q={query}",
            8,
            home,
            History::in_memory(),
        );
        e.set_apps(vec![
            app("blender", "Blender", "3D Modeler", &[]),
            app("firefox", "Firefox", "Web Browser", &["internet", "www"]),
            app(
                "org.gnome.Nautilus",
                "Files",
                "File Manager",
                &["folder", "explorer"],
            ),
            app("kitty", "kitty", "Terminal", &["shell", "console"]),
            app(
                "org.gnome.TextEditor",
                "Text Editor",
                "Text Editor",
                &["notepad"],
            ),
        ]);
        e.set_windows(vec![
            WindowInfo {
                address: "0xa1".into(),
                class: "kitty".into(),
                title: "~/projects/scuttle".into(),
                workspace: "1".into(),
            },
            WindowInfo {
                address: "0xa2".into(),
                class: "firefox".into(),
                title: "Hyprland Wiki".into(),
                workspace: "2".into(),
            },
        ]);
        e.set_files(vec![
            FileEntry {
                path: "/home/me/Documents/q3-report.pdf".into(),
                name: "q3-report.pdf".into(),
                is_dir: false,
            },
            FileEntry {
                path: "/home/me/notes/report-draft.md".into(),
                name: "report-draft.md".into(),
                is_dir: false,
            },
            FileEntry {
                path: "/home/me/Documents/blueprint.pdf".into(),
                name: "blueprint.pdf".into(),
                is_dir: false,
            },
        ]);
        e
    }

    fn titles(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.title.as_str()).collect()
    }

    #[test]
    fn modes_parse_from_prefixes() {
        assert_eq!(Mode::parse("firefox"), (Mode::All, "firefox"));
        assert_eq!(Mode::parse("  > lock "), (Mode::System, "lock"));
        assert_eq!(Mode::parse("/report"), (Mode::Files, "report"));
        assert_eq!(Mode::parse("? rust"), (Mode::Web, "rust"));
        assert_eq!(Mode::parse("=2+2"), (Mode::Calc, "2+2"));
        assert_eq!(Mode::parse("@wiki"), (Mode::Windows, "wiki"));
        assert_eq!(Mode::parse(";token"), (Mode::Clipboard, "token"));
        assert_eq!(Mode::parse(": fire"), (Mode::Emoji, "fire"));
        assert_eq!(Mode::parse(""), (Mode::All, ""));
    }

    #[test]
    fn typing_an_app_name_puts_it_first() {
        let items = engine().search("fire", NOW);
        assert_eq!(items[0].title, "Firefox");
        assert_eq!(items[0].kind, Kind::App);
        assert_eq!(
            items[0].action,
            Action::Launch {
                exec: "firefox".into(),
                terminal: false
            }
        );
    }

    #[test]
    fn apps_match_by_keyword_and_generic_name() {
        let mut e = engine();
        assert!(titles(&e.search("browser", NOW)).contains(&"Firefox"));
        assert!(titles(&e.search("terminal", NOW)).contains(&"kitty"));
        assert!(titles(&e.search("notepad", NOW)).contains(&"Text Editor"));
    }

    #[test]
    fn name_matches_outrank_keyword_matches() {
        let items = engine().search("files", NOW);
        assert_eq!(items[0].title, "Files");
    }

    #[test]
    fn web_search_is_always_the_last_fallback() {
        let items = engine().search("zzzzqqq", NOW);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, Kind::Web);
        assert_eq!(
            items[0].action,
            Action::OpenUrl("https://search.example/?q=zzzzqqq".into())
        );
        let items = engine().search("fire", NOW);
        assert_eq!(items.last().unwrap().kind, Kind::Web);
    }

    #[test]
    fn results_are_grouped_by_kind() {
        let items = engine().search("re", NOW);
        let mut kinds: Vec<Kind> = items.iter().map(|i| i.kind).collect();
        kinds.dedup();
        let mut unique = kinds.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            kinds.len(),
            unique.len(),
            "kinds must be contiguous: {kinds:?}"
        );
    }

    #[test]
    fn never_returns_more_than_the_limit() {
        let mut e = engine();
        e.set_options("https://s/?q={query}", 3);
        for q in ["e", "re", "fi", "t"] {
            assert!(e.search(q, NOW).len() <= 3, "query {q}");
        }
    }

    #[test]
    fn a_sum_shows_its_answer_first() {
        let items = engine().search("12 * (3 + 4)", NOW);
        assert_eq!(items[0].kind, Kind::Calculator);
        assert_eq!(items[0].title, "84");
        assert_eq!(items[0].action, Action::Copy("84".into()));
    }

    #[test]
    fn calc_mode_only_calculates() {
        let mut e = engine();
        assert_eq!(titles(&e.search("= 2^10", NOW)), vec!["1024"]);
        assert!(e.search("= nonsense", NOW).is_empty());
    }

    #[test]
    fn window_mode_finds_windows_by_title_or_class() {
        let mut e = engine();
        assert_eq!(titles(&e.search("@wiki", NOW)), vec!["Hyprland Wiki"]);
        assert_eq!(titles(&e.search("@kitty", NOW)), vec!["~/projects/scuttle"]);
        assert_eq!(e.search("@", NOW).len(), 2);
        let items = e.search("@wiki", NOW);
        assert_eq!(
            items[0].action,
            Action::FocusWindow {
                address: "0xa2".into()
            }
        );
        assert_eq!(items[0].subtitle, "Firefox, workspace 2");
    }

    #[test]
    fn file_mode_finds_files_and_shows_their_folder() {
        let mut e = engine();
        let items = e.search("/report", NOW);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.kind == Kind::File));
        let q3 = items.iter().find(|i| i.title == "q3-report.pdf").unwrap();
        assert_eq!(q3.subtitle, "~/Documents");
        assert_eq!(
            q3.action,
            Action::OpenPath("/home/me/Documents/q3-report.pdf".into())
        );
    }

    #[test]
    fn single_letters_do_not_search_files() {
        let items = engine().search("b", NOW);
        assert!(items.iter().all(|i| i.kind != Kind::File));
    }

    #[test]
    fn system_mode_lists_and_matches_commands() {
        let mut e = engine();
        assert!(e.search(">", NOW).len() >= 5);
        let items = e.search("> reboot", NOW);
        assert_eq!(items[0].title, "Restart");
        assert!(items[0].confirm, "restarting must ask first");
        let items = e.search("> lock", NOW);
        assert_eq!(items[0].title, "Lock screen");
        assert!(!items[0].confirm);
    }

    #[test]
    fn destructive_commands_all_require_confirmation() {
        for command in SYSTEM_COMMANDS {
            let destructive =
                ["Log out", "Restart", "Shut down", "Quit Scuttle"].contains(&command.title);
            assert_eq!(command.confirm, destructive, "{}", command.title);
        }
    }

    #[test]
    fn empty_query_lists_apps_most_used_first() {
        let mut e = engine();
        assert_eq!(e.search("", NOW)[0].title, "Blender");
        let kitty = e.search("kitty", NOW).remove(0);
        e.record_use(&kitty, NOW);
        e.record_use(&kitty, NOW);
        assert_eq!(e.search("", NOW)[0].title, "kitty");
    }

    #[test]
    fn frequently_used_apps_win_close_calls() {
        let mut e = engine();
        // "t" matches both Text Editor and kitty's "Terminal".
        let editor = e.search("text", NOW).remove(0);
        for _ in 0..10 {
            e.record_use(&editor, NOW);
        }
        assert_eq!(e.search("te", NOW)[0].title, "Text Editor");
    }

    #[test]
    fn scattered_letter_matches_are_hidden_when_a_real_match_exists() {
        let mut e = engine();
        e.set_apps(vec![
            app("firefox", "Firefox", "Web Browser", &[]),
            app(
                "libreoffice-impress",
                "LibreOffice Impress",
                "Presentation",
                &[],
            ),
        ]);
        assert_eq!(
            titles(&e.search("fire", NOW)),
            vec!["Firefox", "Hyprland Wiki", "Search the web for \"fire\""]
        );
    }

    #[test]
    fn loose_matches_still_show_when_nothing_better_exists() {
        let mut e = engine();
        e.set_windows(Vec::new());
        e.set_apps(vec![app("firefox", "Firefox", "Web Browser", &[])]);
        assert_eq!(e.search("frfx", NOW)[0].title, "Firefox");
    }

    #[test]
    fn emoji_mode_finds_emoji_by_name() {
        let mut e = engine();
        let items = e.search(":fire", NOW);
        assert_eq!(items[0].title, "🔥  Fire");
        assert_eq!(items[0].action, Action::Copy("🔥".into()));
        assert!(items.iter().all(|i| i.kind == Kind::Emoji));
        assert!(items.len() <= 8);
        assert_eq!(e.search(":crab", NOW)[0].title, "🦀  Crab");
        assert_eq!(e.search(": thumbs up", NOW)[0].title, "👍  Thumbs up");
    }

    #[test]
    fn empty_emoji_mode_shows_smileys_then_learns_favourites() {
        let mut e = engine();
        assert_eq!(e.search(":", NOW)[0].title, "😀  Grinning face");
        let crab = e.search(":crab", NOW).remove(0);
        e.record_use(&crab, NOW);
        assert_eq!(e.search(":", NOW)[0].title, "🦀  Crab");
    }

    #[test]
    fn emoji_only_appear_in_their_own_mode() {
        let items = engine().search("fire", NOW);
        assert!(items.iter().all(|i| i.kind != Kind::Emoji));
    }

    #[test]
    fn clipboard_mode_lists_and_searches_history() {
        let mut e = engine();
        e.set_clipboard(crate::clipboard::parse_list(
            "9\tcargo run -p scuttle\n8\t[[ binary data 12 KiB png 800x600 ]]\n7\thttps://wiki.hypr.land\n",
        ));
        let all = e.search(";", NOW);
        assert_eq!(
            titles(&all),
            vec![
                "cargo run -p scuttle",
                "Image, 800x600 png, 12 KiB",
                "https://wiki.hypr.land"
            ]
        );
        assert!(all.iter().all(|i| i.kind == Kind::Clipboard));

        let found = e.search(";hypr", NOW);
        assert_eq!(titles(&found), vec!["https://wiki.hypr.land"]);
        assert_eq!(
            found[0].action,
            Action::RestoreClip("7\thttps://wiki.hypr.land".into())
        );
        assert_eq!(
            titles(&e.search("; image", NOW)),
            vec!["Image, 800x600 png, 12 KiB"]
        );
    }

    #[test]
    fn clipboard_entries_stay_out_of_normal_search() {
        let mut e = engine();
        e.set_clipboard(crate::clipboard::parse_list("9\tfirefox secret token\n"));
        let items = e.search("fire", NOW);
        assert!(items.iter().all(|i| i.kind != Kind::Clipboard));
    }

    #[test]
    fn empty_clipboard_history_is_just_empty() {
        assert!(engine().search(";", NOW).is_empty());
        assert!(engine().search(";anything", NOW).is_empty());
    }

    #[test]
    fn recognises_web_addresses() {
        assert_eq!(
            as_url("archlinux.org"),
            Some("https://archlinux.org".into())
        );
        assert_eq!(
            as_url("wiki.hypr.land/Configuring"),
            Some("https://wiki.hypr.land/Configuring".into())
        );
        assert_eq!(as_url("https://example.com/a b"), None);
        assert_eq!(
            as_url("http://localhost:8080"),
            Some("http://localhost:8080".into())
        );
        for not_url in [
            "notes.txt",
            "config.fish",
            "report.pdf",
            "hello world.com",
            ".com",
            "plain",
        ] {
            assert_eq!(as_url(not_url), None, "{not_url}");
        }
        let items = engine().search("archlinux.org", NOW);
        assert_eq!(items.last().unwrap().title, "Open archlinux.org");
    }

    #[test]
    fn web_queries_are_percent_encoded() {
        assert_eq!(percent_encode("rust & gtk4"), "rust%20%26%20gtk4");
        assert_eq!(percent_encode("café"), "caf%C3%A9");
        assert_eq!(percent_encode("a-b_c.d~e"), "a-b_c.d~e");
        let items = engine().search("? what is 50% of 10", NOW);
        assert_eq!(
            items[0].action,
            Action::OpenUrl("https://search.example/?q=what%20is%2050%25%20of%2010".into())
        );
    }

    #[test]
    fn fair_interleaving_keeps_one_of_each_group() {
        let item = |kind, title: &str, score| Item {
            kind,
            title: title.into(),
            subtitle: String::new(),
            icon: None,
            action: Action::Copy(String::new()),
            confirm: false,
            key: title.into(),
            score,
        };
        let groups = vec![
            vec![
                item(Kind::App, "a1", 9),
                item(Kind::App, "a2", 8),
                item(Kind::App, "a3", 7),
            ],
            vec![item(Kind::File, "f1", 5), item(Kind::File, "f2", 4)],
            vec![item(Kind::System, "s1", 3)],
        ];
        let out = interleave_fairly(groups.clone(), 4);
        assert_eq!(titles(&out), vec!["a1", "a2", "f1", "s1"]);
        let out = interleave_fairly(groups.clone(), 2);
        assert_eq!(titles(&out), vec!["a1", "f1"]);
        let out = interleave_fairly(groups, 100);
        assert_eq!(out.len(), 6);
    }
}

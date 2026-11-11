//! Turns a query into a ranked, grouped list of results.

use std::path::{Path, PathBuf};

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher, Utf32Str};

use crate::calc;
use crate::desktop::{exec_command, DesktopEntry};
use crate::files::{display_path, FileEntry};
use crate::history::History;
use crate::item::{Action, Internal, Item, Kind};

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

    pub fn set_options(&mut self, web_template: &str, max_results: usize) {
        self.web_template = web_template.to_string();
        self.max_results = max_results.max(1);
    }

    /// Notes that a result was chosen, so it ranks higher next time.
    pub fn record_use(&mut self, item: &Item, now: u64) {
        // Windows and one-off results are not worth remembering.
        if matches!(item.kind, Kind::App | Kind::System | Kind::File) {
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
                Mode::Files | Mode::Web | Mode::Calc => Vec::new(),
            };
        }

        match mode {
            Mode::Calc => self.calc(text).into_iter().collect(),
            Mode::Web => vec![self.web(text)],
            Mode::System => self.match_system(text, limit),
            Mode::Windows => self.match_windows(text, limit),
            Mode::Files => self.match_files(text, limit),
            Mode::All => {
                let mut groups = vec![
                    self.match_apps(text, now, APP_CAP),
                    self.match_windows(text, WINDOW_CAP),
                    self.match_system(text, SYSTEM_CAP),
                ];
                if text.chars().count() >= 2 {
                    groups.push(self.match_files(text, FILE_CAP));
                }
                groups.retain(|g| !g.is_empty());
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

    fn score(matcher: &mut Matcher, pattern: &Pattern, haystack: &str) -> Option<u32> {
        let mut buf = Vec::new();
        pattern.score(Utf32Str::new(haystack, &mut buf), matcher)
    }

    fn pattern(text: &str) -> Pattern {
        Pattern::new(text, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy)
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
            action: Action::Launch { exec: exec_command(&entry.exec), terminal: entry.terminal },
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

    fn match_apps(&mut self, text: &str, now: u64, cap: usize) -> Vec<Item> {
        let pattern = Self::pattern(text);
        let lowered = text.to_lowercase();
        let mut items = Vec::new();
        for entry in &self.apps {
            let by_name = Self::score(&mut self.matcher, &pattern, &entry.name);
            let extra = format!("{} {} {}", entry.generic_name, entry.keywords.join(" "), entry.id);
            let by_extra = Self::score(&mut self.matcher, &pattern, &extra).map(|s| s * 6 / 10);
            let Some(mut score) = by_name.into_iter().chain(by_extra).max() else { continue };
            // Typing the start of the name is the strongest signal.
            if entry.name.to_lowercase().starts_with(&lowered) {
                score += 60;
            }
            score += self.history.boost(&format!("app:{}", entry.id), now);
            items.push(Self::app_item(entry, score));
        }
        finish(items, cap)
    }

    fn window_item(window: &WindowInfo, score: u32) -> Item {
        Item {
            kind: Kind::Window,
            title: window.title.clone(),
            subtitle: format!("Workspace {}", window.workspace),
            icon: Some(window.class.to_lowercase()),
            action: Action::FocusWindow { address: window.address.clone() },
            confirm: false,
            key: format!("win:{}", window.address),
            score,
        }
    }

    fn all_windows(&self, limit: usize) -> Vec<Item> {
        self.windows.iter().take(limit).map(|w| Self::window_item(w, 0)).collect()
    }

    fn match_windows(&mut self, text: &str, cap: usize) -> Vec<Item> {
        let pattern = Self::pattern(text);
        let mut items = Vec::new();
        for window in &self.windows {
            let haystack = format!("{} {}", window.title, window.class);
            if let Some(score) = Self::score(&mut self.matcher, &pattern, &haystack) {
                items.push(Self::window_item(window, score * 9 / 10));
            }
        }
        finish(items, cap)
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
        SYSTEM_COMMANDS.iter().take(limit).map(|c| Self::system_item(c, 0)).collect()
    }

    fn match_system(&mut self, text: &str, cap: usize) -> Vec<Item> {
        let pattern = Self::pattern(text);
        let mut items = Vec::new();
        for command in SYSTEM_COMMANDS {
            let by_title = Self::score(&mut self.matcher, &pattern, command.title);
            let by_keyword =
                Self::score(&mut self.matcher, &pattern, command.keywords).map(|s| s * 8 / 10);
            if let Some(score) = by_title.into_iter().chain(by_keyword).max() {
                items.push(Self::system_item(command, score * 95 / 100));
            }
        }
        finish(items, cap)
    }

    fn match_files(&mut self, text: &str, cap: usize) -> Vec<Item> {
        let pattern = Self::pattern(text);
        let mut items = Vec::new();
        for file in &self.files {
            let Some(score) = Self::score(&mut self.path_matcher, &pattern, &file.name) else {
                continue;
            };
            let parent = file.path.parent().unwrap_or(Path::new("/"));
            items.push(Item {
                kind: Kind::File,
                title: file.name.clone(),
                subtitle: display_path(parent, &self.home),
                icon: Some(if file.is_dir { "folder-symbolic" } else { "text-x-generic-symbolic" }.into()),
                action: Action::OpenPath(file.path.clone()),
                confirm: false,
                key: format!("file:{}", file.path.display()),
                score: score * 8 / 10,
            });
        }
        finish(items, cap)
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
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
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
        let mut e = Engine::new("https://search.example/?q={query}", 8, home, History::in_memory());
        e.set_apps(vec![
            app("blender", "Blender", "3D Modeler", &[]),
            app("firefox", "Firefox", "Web Browser", &["internet", "www"]),
            app("org.gnome.Nautilus", "Files", "File Manager", &["folder", "explorer"]),
            app("kitty", "kitty", "Terminal", &["shell", "console"]),
            app("org.gnome.TextEditor", "Text Editor", "Text Editor", &["notepad"]),
        ]);
        e.set_windows(vec![
            WindowInfo { address: "0xa1".into(), class: "kitty".into(), title: "~/projects/scuttle".into(), workspace: "1".into() },
            WindowInfo { address: "0xa2".into(), class: "firefox".into(), title: "Hyprland Wiki".into(), workspace: "2".into() },
        ]);
        e.set_files(vec![
            FileEntry { path: "/home/me/Documents/q3-report.pdf".into(), name: "q3-report.pdf".into(), is_dir: false },
            FileEntry { path: "/home/me/notes/report-draft.md".into(), name: "report-draft.md".into(), is_dir: false },
            FileEntry { path: "/home/me/Documents/blueprint.pdf".into(), name: "blueprint.pdf".into(), is_dir: false },
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
        assert_eq!(Mode::parse(""), (Mode::All, ""));
    }

    #[test]
    fn typing_an_app_name_puts_it_first() {
        let items = engine().search("fire", NOW);
        assert_eq!(items[0].title, "Firefox");
        assert_eq!(items[0].kind, Kind::App);
        assert_eq!(
            items[0].action,
            Action::Launch { exec: "firefox".into(), terminal: false }
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
        assert_eq!(kinds.len(), unique.len(), "kinds must be contiguous: {kinds:?}");
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
        assert_eq!(items[0].action, Action::FocusWindow { address: "0xa2".into() });
        assert_eq!(items[0].subtitle, "Workspace 2");
    }

    #[test]
    fn file_mode_finds_files_and_shows_their_folder() {
        let mut e = engine();
        let items = e.search("/report", NOW);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.kind == Kind::File));
        let q3 = items.iter().find(|i| i.title == "q3-report.pdf").unwrap();
        assert_eq!(q3.subtitle, "~/Documents");
        assert_eq!(q3.action, Action::OpenPath("/home/me/Documents/q3-report.pdf".into()));
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
            let destructive = ["Log out", "Restart", "Shut down", "Quit Scuttle"].contains(&command.title);
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
    fn recognises_web_addresses() {
        assert_eq!(as_url("archlinux.org"), Some("https://archlinux.org".into()));
        assert_eq!(as_url("wiki.hypr.land/Configuring"), Some("https://wiki.hypr.land/Configuring".into()));
        assert_eq!(as_url("https://example.com/a b"), None);
        assert_eq!(as_url("http://localhost:8080"), Some("http://localhost:8080".into()));
        for not_url in ["notes.txt", "config.fish", "report.pdf", "hello world.com", ".com", "plain"] {
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
            vec![item(Kind::App, "a1", 9), item(Kind::App, "a2", 8), item(Kind::App, "a3", 7)],
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

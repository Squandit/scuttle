//! Notifications as Scuttle keeps them, and the rules for showing,
//! replacing, expiring and remembering them. No D-Bus and no UI here.

use std::collections::VecDeque;

/// How important the sender says a notification is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

impl Urgency {
    /// From the `urgency` hint byte in the notification spec.
    pub fn from_hint(value: u8) -> Urgency {
        match value {
            0 => Urgency::Low,
            2 => Urgency::Critical,
            _ => Urgency::Normal,
        }
    }
}

/// How long a notification asked to stay on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeout {
    /// Let the server decide.
    Default,
    /// Stay until dismissed.
    Never,
    Millis(u32),
}

impl Timeout {
    /// From the `expire_timeout` argument: -1 is "you decide", 0 is
    /// "never", anything else is milliseconds.
    pub fn from_spec(value: i32) -> Timeout {
        match value {
            0 => Timeout::Never,
            v if v < 0 => Timeout::Default,
            v => Timeout::Millis(v as u32),
        }
    }
}

/// A button on a notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionButton {
    /// The identifier sent back to the app when the button is pressed.
    pub key: String,
    pub label: String,
}

/// Why a notification went away, as reported back to the sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Expired = 1,
    Dismissed = 2,
    /// The sending app asked for it to close.
    ClosedByApp = 3,
}

/// One notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    /// An icon name or a file path, possibly empty.
    pub icon: String,
    pub summary: String,
    /// Plain text. Any markup the sender included has been removed.
    pub body: String,
    pub actions: Vec<ActionButton>,
    pub urgency: Urgency,
    pub timeout: Timeout,
    /// The sender asked for this not to be kept in the history.
    pub transient: bool,
    /// A level from 0 to 100 to draw as a bar, as volume and brightness
    /// popups send.
    pub progress: Option<u8>,
    /// Notifications sharing a tag replace each other, so repeated
    /// volume steps update one toast instead of stacking up.
    pub sync_tag: Option<String>,
    /// When it arrived, in seconds since the Unix epoch.
    pub received_at: u64,
}

impl Notification {
    /// The action run by clicking the notification itself, if the app
    /// provided one.
    pub fn default_action(&self) -> Option<&ActionButton> {
        self.actions.iter().find(|a| a.key == "default")
    }

    /// The actions shown as buttons: everything except the default.
    pub fn buttons(&self) -> impl Iterator<Item = &ActionButton> {
        self.actions.iter().filter(|a| a.key != "default")
    }

    /// How long to keep this on screen, in milliseconds. `None` means
    /// until dismissed.
    pub fn display_millis(&self, default_millis: u32) -> Option<u32> {
        match (self.urgency, self.timeout) {
            // Critical notifications never time out by themselves,
            // whatever the sender asked for.
            (Urgency::Critical, _) | (_, Timeout::Never) => None,
            // A level popup is glanced at, not read.
            (_, Timeout::Default) if self.sync_tag.is_some() => Some(default_millis.min(1600)),
            (_, Timeout::Default) => Some(default_millis),
            // Keep even a very short request readable.
            (_, Timeout::Millis(ms)) => Some(ms.clamp(1500, 60_000)),
        }
    }
}

/// Pairs up the flat `[key, label, key, label, ...]` list the spec uses.
/// A trailing key with no label is dropped.
pub fn pair_actions(flat: &[String]) -> Vec<ActionButton> {
    flat.chunks_exact(2)
        .map(|pair| ActionButton {
            key: pair[0].clone(),
            label: pair[1].clone(),
        })
        .collect()
}

/// Turns a notification body into plain text: tags are removed, line
/// breaks kept, and the five XML entities decoded.
pub fn plain_text(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        match after.find('>') {
            Some(end) => {
                let tag = after[1..end].trim().to_ascii_lowercase();
                // Only treat it as a tag if it looks like one. "2 < 3"
                // and "<3" are ordinary text.
                let name = tag
                    .trim_start_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                let name = name.trim_end_matches('/');
                let is_tag = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic());
                if is_tag {
                    if name == "br" {
                        out.push('\n');
                    }
                    rest = &after[end + 1..];
                } else {
                    out.push('<');
                    rest = &after[1..];
                }
            }
            None => {
                out.push('<');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);

    let decoded = out
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        // Last, so "&amp;lt;" becomes "&lt;" and not "<".
        .replace("&amp;", "&");
    decoded.trim().to_string()
}

/// What the store wants the UI to do after a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Show a new toast.
    Show(Notification),
    /// Update the toast with the same id in place.
    Replace(Notification),
    /// Remove a toast.
    Hide(u32),
}

/// The notifications currently on screen and the history of past ones.
#[derive(Debug)]
pub struct Store {
    next_id: u32,
    active: Vec<Notification>,
    history: VecDeque<Notification>,
    history_limit: usize,
    do_not_disturb: bool,
    /// History entries that arrived since the history was last looked at.
    unseen: usize,
}

impl Default for Store {
    fn default() -> Self {
        Store::new(50)
    }
}

impl Store {
    pub fn new(history_limit: usize) -> Store {
        Store {
            next_id: 1,
            active: Vec::new(),
            history: VecDeque::new(),
            history_limit,
            do_not_disturb: false,
            unseen: 0,
        }
    }

    /// Takes in a notification from an app. `replaces_id` is the id of
    /// an earlier notification to update, or 0 for a new one.
    ///
    /// Returns the id the notification was given and what, if anything,
    /// the UI should do. With "do not disturb" on, only critical
    /// notifications are shown; the rest go straight to the history.
    pub fn add(&mut self, mut incoming: Notification, replaces_id: u32) -> (u32, Option<Change>) {
        // A tagged notification replaces the one on screen with the
        // same tag, whatever id the sender passed.
        let replaces_id = incoming
            .sync_tag
            .as_ref()
            .and_then(|tag| {
                self.active
                    .iter()
                    .find(|n| n.sync_tag.as_ref() == Some(tag))
            })
            .map(|n| n.id)
            .unwrap_or(replaces_id);
        let replacing = replaces_id != 0 && self.active.iter().any(|n| n.id == replaces_id);
        let id = if replaces_id != 0 {
            // Reuse the id even if the old one has gone, as the spec
            // asks, and make sure fresh ids never collide with it.
            if replaces_id >= self.next_id {
                self.next_id = replaces_id.checked_add(1).unwrap_or(1);
            }
            replaces_id
        } else {
            let id = self.next_id;
            // 0 means "new" on the wire, so never hand it out.
            self.next_id = id.checked_add(1).unwrap_or(1);
            id
        };
        incoming.id = id;

        let show = !self.do_not_disturb || incoming.urgency == Urgency::Critical;
        if !show {
            self.remember(incoming);
            return (id, None);
        }

        if replacing {
            if let Some(slot) = self.active.iter_mut().find(|n| n.id == id) {
                *slot = incoming.clone();
            }
            (id, Some(Change::Replace(incoming)))
        } else {
            self.active.push(incoming.clone());
            (id, Some(Change::Show(incoming)))
        }
    }

    /// Removes a notification from the screen. Returns it if it was
    /// there. Unless it was transient, it moves to the history.
    pub fn close(&mut self, id: u32) -> Option<Notification> {
        let index = self.active.iter().position(|n| n.id == id)?;
        let closed = self.active.remove(index);
        self.remember(closed.clone());
        Some(closed)
    }

    fn remember(&mut self, notification: Notification) {
        if notification.transient || self.history_limit == 0 {
            return;
        }
        self.history.retain(|n| n.id != notification.id);
        self.history.push_front(notification);
        self.history.truncate(self.history_limit);
        self.unseen = (self.unseen + 1).min(self.history.len());
    }

    /// Notifications currently on screen, oldest first.
    pub fn active(&self) -> &[Notification] {
        &self.active
    }

    /// Looks up an on-screen notification.
    pub fn get(&self, id: u32) -> Option<&Notification> {
        self.active.iter().find(|n| n.id == id)
    }

    /// Past notifications, newest first.
    pub fn history(&self) -> impl Iterator<Item = &Notification> {
        self.history.iter()
    }

    /// How many history entries have not been looked at yet.
    pub fn unseen(&self) -> usize {
        self.unseen
    }

    /// Marks the whole history as looked at.
    pub fn mark_seen(&mut self) {
        self.unseen = 0;
    }

    /// Forgets all past notifications.
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.unseen = 0;
    }

    pub fn do_not_disturb(&self) -> bool {
        self.do_not_disturb
    }

    pub fn set_do_not_disturb(&mut self, on: bool) {
        self.do_not_disturb = on;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(summary: &str) -> Notification {
        Notification {
            id: 0,
            app_name: "test".into(),
            icon: String::new(),
            summary: summary.into(),
            body: String::new(),
            actions: Vec::new(),
            urgency: Urgency::Normal,
            timeout: Timeout::Default,
            transient: false,
            progress: None,
            sync_tag: None,
            received_at: 0,
        }
    }

    #[test]
    fn new_notifications_get_increasing_ids_starting_at_one() {
        let mut store = Store::default();
        let (a, change) = store.add(note("a"), 0);
        let (b, _) = store.add(note("b"), 0);
        assert_eq!((a, b), (1, 2));
        assert!(matches!(change, Some(Change::Show(n)) if n.id == 1 && n.summary == "a"));
        assert_eq!(store.active().len(), 2);
    }

    #[test]
    fn replacing_updates_in_place_and_keeps_the_id() {
        let mut store = Store::default();
        let (id, _) = store.add(note("Downloading 10%"), 0);
        let (same, change) = store.add(note("Downloading 90%"), id);
        assert_eq!(same, id);
        assert!(matches!(change, Some(Change::Replace(n)) if n.summary == "Downloading 90%"));
        assert_eq!(store.active().len(), 1);
        assert_eq!(store.get(id).unwrap().summary, "Downloading 90%");
    }

    #[test]
    fn replacing_something_already_gone_shows_it_as_new_with_that_id() {
        let mut store = Store::default();
        let (id, change) = store.add(note("late"), 7);
        assert_eq!(id, 7);
        assert!(matches!(change, Some(Change::Show(_))));
        // The next fresh id must not collide with it.
        let (next, _) = store.add(note("after"), 0);
        assert_eq!(next, 8);
    }

    #[test]
    fn tagged_notifications_replace_each_other() {
        let mut store = Store::default();
        let level = |percent: u8| {
            let mut n = note("Volume");
            n.sync_tag = Some("volume".into());
            n.progress = Some(percent);
            n.transient = true;
            n
        };
        let (first, change) = store.add(level(40), 0);
        assert!(matches!(change, Some(Change::Show(_))));
        let (second, change) = store.add(level(45), 0);
        assert_eq!(second, first, "the same toast is reused");
        assert!(matches!(change, Some(Change::Replace(n)) if n.progress == Some(45)));
        assert_eq!(store.active().len(), 1);

        // A different tag is a different toast.
        let mut brightness = note("Brightness");
        brightness.sync_tag = Some("brightness".into());
        let (other, change) = store.add(brightness, 0);
        assert_ne!(other, first);
        assert!(matches!(change, Some(Change::Show(_))));

        // Level popups are glanced at: short display, no history.
        assert_eq!(store.get(first).unwrap().display_millis(5000), Some(1600));
        store.close(first);
        assert_eq!(store.history().count(), 0);
    }

    #[test]
    fn closing_moves_to_history_newest_first() {
        let mut store = Store::default();
        let (a, _) = store.add(note("a"), 0);
        let (b, _) = store.add(note("b"), 0);
        assert_eq!(store.close(a).unwrap().summary, "a");
        store.close(b);
        assert!(store.active().is_empty());
        let history: Vec<&str> = store.history().map(|n| n.summary.as_str()).collect();
        assert_eq!(history, vec!["b", "a"]);
        assert_eq!(store.unseen(), 2);
        store.mark_seen();
        assert_eq!(store.unseen(), 0);
    }

    #[test]
    fn closing_an_unknown_id_does_nothing() {
        let mut store = Store::default();
        assert_eq!(store.close(99), None);
    }

    #[test]
    fn transient_notifications_are_not_remembered() {
        let mut store = Store::default();
        let mut n = note("volume");
        n.transient = true;
        let (id, _) = store.add(n, 0);
        store.close(id);
        assert_eq!(store.history().count(), 0);
        assert_eq!(store.unseen(), 0);
    }

    #[test]
    fn history_is_capped() {
        let mut store = Store::new(3);
        for i in 0..10 {
            let (id, _) = store.add(note(&i.to_string()), 0);
            store.close(id);
        }
        let history: Vec<&str> = store.history().map(|n| n.summary.as_str()).collect();
        assert_eq!(history, vec!["9", "8", "7"]);
        assert_eq!(store.unseen(), 3);
    }

    #[test]
    fn do_not_disturb_hides_all_but_critical() {
        let mut store = Store::default();
        store.set_do_not_disturb(true);
        let (_, quiet) = store.add(note("chat message"), 0);
        assert_eq!(quiet, None);
        assert!(store.active().is_empty());
        assert_eq!(
            store.history().count(),
            1,
            "hidden notifications are still kept"
        );

        let mut urgent = note("battery critically low");
        urgent.urgency = Urgency::Critical;
        let (_, shown) = store.add(urgent, 0);
        assert!(matches!(shown, Some(Change::Show(_))));
    }

    #[test]
    fn display_time_follows_urgency_and_request() {
        let mut n = note("x");
        assert_eq!(n.display_millis(5000), Some(5000));
        n.timeout = Timeout::Millis(10_000);
        assert_eq!(n.display_millis(5000), Some(10_000));
        n.timeout = Timeout::Millis(1);
        assert_eq!(
            n.display_millis(5000),
            Some(1500),
            "too short to read is raised"
        );
        n.timeout = Timeout::Never;
        assert_eq!(n.display_millis(5000), None);
        n.timeout = Timeout::Millis(3000);
        n.urgency = Urgency::Critical;
        assert_eq!(
            n.display_millis(5000),
            None,
            "critical stays until dismissed"
        );
    }

    #[test]
    fn spec_values_convert() {
        assert_eq!(Timeout::from_spec(-1), Timeout::Default);
        assert_eq!(Timeout::from_spec(0), Timeout::Never);
        assert_eq!(Timeout::from_spec(2500), Timeout::Millis(2500));
        assert_eq!(Urgency::from_hint(0), Urgency::Low);
        assert_eq!(Urgency::from_hint(1), Urgency::Normal);
        assert_eq!(Urgency::from_hint(2), Urgency::Critical);
        assert_eq!(Urgency::from_hint(200), Urgency::Normal);
    }

    #[test]
    fn actions_pair_up_and_split_into_default_and_buttons() {
        let flat: Vec<String> = ["default", "Open", "reply", "Reply", "dangling"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut n = note("x");
        n.actions = pair_actions(&flat);
        assert_eq!(n.actions.len(), 2);
        assert_eq!(n.default_action().unwrap().label, "Open");
        let buttons: Vec<&str> = n.buttons().map(|a| a.label.as_str()).collect();
        assert_eq!(buttons, vec!["Reply"]);
    }

    #[test]
    fn bodies_become_plain_text() {
        assert_eq!(
            plain_text("<b>Bold</b> and <i>italic</i>"),
            "Bold and italic"
        );
        assert_eq!(
            plain_text("line one<br>line two<br/>three"),
            "line one\nline two\nthree"
        );
        assert_eq!(plain_text("<a href=\"https://x.y\">link</a>"), "link");
        assert_eq!(plain_text("Tom &amp; Jerry &lt;3"), "Tom & Jerry <3");
        assert_eq!(
            plain_text("&amp;lt; stays escaped once"),
            "&lt; stays escaped once"
        );
    }

    #[test]
    fn text_that_only_looks_like_markup_is_kept() {
        assert_eq!(plain_text("2 < 3 and 5 > 4"), "2 < 3 and 5 > 4");
        assert_eq!(plain_text("I <3 Arch"), "I <3 Arch");
        assert_eq!(plain_text("unclosed <b"), "unclosed <b");
        assert_eq!(plain_text("a <= b"), "a <= b");
    }

    #[test]
    fn ids_never_wrap_to_zero() {
        let mut store = Store {
            next_id: u32::MAX,
            ..Default::default()
        };
        let (a, _) = store.add(note("a"), 0);
        let (b, _) = store.add(note("b"), 0);
        assert_eq!(a, u32::MAX);
        assert_eq!(b, 1);
    }
}

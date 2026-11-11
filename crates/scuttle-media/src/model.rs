//! What a player reports, and which player to show when several run.

/// What one media player is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// The player's D-Bus name, e.g. `org.mpris.MediaPlayer2.spotify`.
    pub player: String,
    /// The player's name for people, e.g. `Spotify`.
    pub identity: String,
    pub title: String,
    /// Artists joined with commas. Empty when unknown.
    pub artist: String,
    pub playing: bool,
}

impl Track {
    /// One line for a narrow space: the title, or the player's name
    /// when the track has none.
    pub fn headline(&self) -> &str {
        if self.title.is_empty() {
            &self.identity
        } else {
            &self.title
        }
    }
}

/// Something to tell the player to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    PlayPause,
    Next,
    Previous,
}

impl Control {
    /// The MPRIS method name.
    pub fn method(self) -> &'static str {
        match self {
            Control::PlayPause => "PlayPause",
            Control::Next => "Next",
            Control::Previous => "Previous",
        }
    }
}

/// Chooses which player to show.
///
/// A playing player beats a paused one. Between equals, the one that
/// was showing before stays, so the widget does not flick between two
/// players. Players with nothing loaded are ignored unless they are
/// all there is.
pub fn pick<'a>(previous: Option<&str>, tracks: &'a [Track]) -> Option<&'a Track> {
    let rank = |t: &Track| {
        (
            t.playing,
            Some(t.player.as_str()) == previous,
            !t.title.is_empty(),
        )
    };
    tracks.iter().max_by_key(|t| rank(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(player: &str, title: &str, playing: bool) -> Track {
        Track {
            player: player.into(),
            identity: player.to_uppercase(),
            title: title.into(),
            artist: String::new(),
            playing,
        }
    }

    #[test]
    fn nothing_running_shows_nothing() {
        assert_eq!(pick(None, &[]), None);
    }

    #[test]
    fn the_playing_player_wins() {
        let tracks = [
            track("browser", "A video", false),
            track("music", "A song", true),
        ];
        assert_eq!(pick(None, &tracks).unwrap().player, "music");
        assert_eq!(pick(Some("browser"), &tracks).unwrap().player, "music");
    }

    #[test]
    fn between_two_playing_the_current_one_stays() {
        let tracks = [track("a", "One", true), track("b", "Two", true)];
        assert_eq!(pick(Some("a"), &tracks).unwrap().player, "a");
        assert_eq!(pick(Some("b"), &tracks).unwrap().player, "b");
    }

    #[test]
    fn a_paused_player_still_shows_and_stays_put() {
        let tracks = [track("a", "One", false), track("b", "Two", false)];
        assert_eq!(pick(Some("b"), &tracks).unwrap().player, "b");
    }

    #[test]
    fn players_with_nothing_loaded_come_last() {
        let tracks = [track("idle", "", false), track("music", "A song", false)];
        assert_eq!(pick(None, &tracks).unwrap().player, "music");
        let only_idle = [track("idle", "", false)];
        assert_eq!(pick(None, &only_idle).unwrap().player, "idle");
    }

    #[test]
    fn headline_falls_back_to_the_player_name() {
        assert_eq!(track("music", "A song", true).headline(), "A song");
        assert_eq!(track("music", "", true).headline(), "MUSIC");
    }

    #[test]
    fn controls_map_to_mpris_methods() {
        assert_eq!(Control::PlayPause.method(), "PlayPause");
        assert_eq!(Control::Next.method(), "Next");
        assert_eq!(Control::Previous.method(), "Previous");
    }
}

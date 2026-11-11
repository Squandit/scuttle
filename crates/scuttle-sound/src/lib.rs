//! Interface sounds for Scuttle: short synthesised tones played when
//! things happen, by default only while headphones are in use.

pub mod headphones;
pub mod player;
pub mod synth;

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub use player::Player;
pub use synth::Sound;

/// When interface sounds are allowed to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum When {
    Never,
    /// Only while the audio output is headphones or a headset.
    Headphones,
    Always,
}

/// How long a headphone check is trusted before asking again.
const RECHECK_AFTER: Duration = Duration::from_secs(5);

const UNKNOWN: u8 = 0;
const SPEAKERS: u8 = 1;
const HEADPHONES: u8 = 2;

/// Plays interface sounds according to the user's settings.
///
/// `play` is safe to call on every key press: it never blocks, and
/// checking for headphones happens on another thread.
pub struct Feedback {
    player: Player,
    when: Cell<When>,
    volume: Cell<f32>,
    /// The last headphone check's answer, shared with the checker thread.
    output: Arc<AtomicU8>,
    checking: Arc<AtomicBool>,
    checked_at: Cell<Option<Instant>>,
}

impl Feedback {
    pub fn new(when: When, volume: f32) -> Feedback {
        let feedback = Feedback {
            player: Player::start(),
            when: Cell::new(when),
            volume: Cell::new(volume.clamp(0.0, 1.0)),
            output: Arc::new(AtomicU8::new(UNKNOWN)),
            checking: Arc::new(AtomicBool::new(false)),
            checked_at: Cell::new(None),
        };
        feedback.refresh_if_stale();
        feedback
    }

    /// Applies new settings.
    pub fn configure(&self, when: When, volume: f32) {
        self.when.set(when);
        self.volume.set(volume.clamp(0.0, 1.0));
        self.refresh_if_stale();
    }

    /// Starts a headphone check in the background if the last one is
    /// old. Only one check runs at a time.
    fn refresh_if_stale(&self) {
        if self.when.get() != When::Headphones {
            return;
        }
        let fresh = self
            .checked_at
            .get()
            .is_some_and(|at| at.elapsed() < RECHECK_AFTER);
        if fresh || self.checking.swap(true, Ordering::AcqRel) {
            return;
        }
        self.checked_at.set(Some(Instant::now()));
        let output = Arc::clone(&self.output);
        let checking = Arc::clone(&self.checking);
        thread::spawn(move || {
            let answer = match headphones::detect() {
                Some(true) => HEADPHONES,
                // No answer means it cannot be confirmed, so stay quiet.
                Some(false) | None => SPEAKERS,
            };
            output.store(answer, Ordering::Release);
            checking.store(false, Ordering::Release);
        });
    }

    /// Whether a sound would play right now.
    pub fn enabled(&self) -> bool {
        match self.when.get() {
            When::Never => false,
            When::Always => true,
            When::Headphones => self.output.load(Ordering::Acquire) == HEADPHONES,
        }
    }

    /// Plays `sound` if the settings allow it.
    pub fn play(&self, sound: Sound) {
        self.refresh_if_stale();
        if self.enabled() && self.volume.get() > 0.0 {
            self.player.play(sound, self.volume.get());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_means_never() {
        let feedback = Feedback::new(When::Never, 1.0);
        assert!(!feedback.enabled());
        feedback.play(Sound::Tick);
    }

    #[test]
    fn always_is_enabled_without_any_check() {
        let feedback = Feedback::new(When::Always, 0.3);
        assert!(feedback.enabled());
        assert_eq!(feedback.output.load(Ordering::Acquire), UNKNOWN);
    }

    #[test]
    fn headphones_mode_is_quiet_until_headphones_are_confirmed() {
        let feedback = Feedback::new(When::Headphones, 0.3);
        // Whatever the background check finds on this machine, an
        // unconfirmed output must not play.
        feedback.output.store(UNKNOWN, Ordering::Release);
        assert!(!feedback.enabled());
        feedback.output.store(SPEAKERS, Ordering::Release);
        assert!(!feedback.enabled());
        feedback.output.store(HEADPHONES, Ordering::Release);
        assert!(feedback.enabled());
    }

    #[test]
    fn settings_can_change_while_running() {
        let feedback = Feedback::new(When::Never, 0.3);
        feedback.configure(When::Always, 2.0);
        assert!(feedback.enabled());
        assert_eq!(feedback.volume.get(), 1.0);
        feedback.configure(When::Never, 0.5);
        assert!(!feedback.enabled());
    }

    #[test]
    fn play_never_blocks() {
        let feedback = Feedback::new(When::Always, 0.2);
        let started = Instant::now();
        for _ in 0..100 {
            feedback.play(Sound::Tick);
        }
        assert!(started.elapsed() < Duration::from_millis(300));
    }
}

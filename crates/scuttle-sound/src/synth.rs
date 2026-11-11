//! The interface sounds, generated from a few sine waves.
//!
//! Nothing here is a recording, so there are no audio files to ship or
//! license. Every sound is short, soft and fades in and out, which keeps
//! it from clicking or startling anyone wearing headphones.

use std::f32::consts::TAU;

/// The interface events that make a sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    /// Moving the selection in a list.
    Tick,
    /// The launcher opening.
    Open,
    /// The launcher closing with nothing chosen.
    Close,
    /// Running the selected result.
    Activate,
    /// A destructive action waiting for a second Enter.
    Ask,
    /// Something could not be done.
    Error,
    /// A notification arriving.
    Notify,
}

impl Sound {
    pub const ALL: [Sound; 7] = [
        Sound::Tick,
        Sound::Open,
        Sound::Close,
        Sound::Activate,
        Sound::Ask,
        Sound::Error,
        Sound::Notify,
    ];
}

/// One enveloped sine partial.
struct Tone {
    /// Frequency at the start, in hertz.
    from: f32,
    /// Frequency at the end. Different from `from` for a glide.
    to: f32,
    /// When the tone starts, in seconds from the start of the sound.
    start: f32,
    /// How long it lasts, in seconds.
    length: f32,
    /// Peak level, 0 to 1, before the master volume.
    level: f32,
}

fn tones(sound: Sound) -> &'static [Tone] {
    match sound {
        // A tiny high blip. Short enough to repeat 30 times a second
        // while a key is held without smearing into a tone.
        Sound::Tick => &[Tone {
            from: 1900.0,
            to: 1700.0,
            start: 0.0,
            length: 0.022,
            level: 0.45,
        }],
        // A soft rising pair.
        Sound::Open => &[
            Tone {
                from: 520.0,
                to: 520.0,
                start: 0.0,
                length: 0.07,
                level: 0.40,
            },
            Tone {
                from: 780.0,
                to: 780.0,
                start: 0.035,
                length: 0.09,
                level: 0.40,
            },
        ],
        // The same pair falling.
        Sound::Close => &[
            Tone {
                from: 700.0,
                to: 700.0,
                start: 0.0,
                length: 0.06,
                level: 0.32,
            },
            Tone {
                from: 470.0,
                to: 470.0,
                start: 0.03,
                length: 0.08,
                level: 0.32,
            },
        ],
        // A fuller, lower note with an octave on top.
        Sound::Activate => &[
            Tone {
                from: 660.0,
                to: 660.0,
                start: 0.0,
                length: 0.10,
                level: 0.42,
            },
            Tone {
                from: 1320.0,
                to: 1320.0,
                start: 0.0,
                length: 0.06,
                level: 0.18,
            },
        ],
        // Two equal notes: a question.
        Sound::Ask => &[
            Tone {
                from: 587.0,
                to: 587.0,
                start: 0.0,
                length: 0.06,
                level: 0.40,
            },
            Tone {
                from: 587.0,
                to: 587.0,
                start: 0.09,
                length: 0.06,
                level: 0.40,
            },
        ],
        // A low downward glide.
        Sound::Error => &[Tone {
            from: 300.0,
            to: 210.0,
            start: 0.0,
            length: 0.14,
            level: 0.50,
        }],
        // Two soft notes a fifth apart, a little longer than the rest
        // so it reads as "something arrived".
        Sound::Notify => &[
            Tone {
                from: 660.0,
                to: 660.0,
                start: 0.0,
                length: 0.09,
                level: 0.34,
            },
            Tone {
                from: 990.0,
                to: 990.0,
                start: 0.07,
                length: 0.12,
                level: 0.30,
            },
        ],
    }
}

/// How long the sound lasts, in seconds.
pub fn duration(sound: Sound) -> f32 {
    tones(sound)
        .iter()
        .map(|t| t.start + t.length)
        .fold(0.0, f32::max)
}

/// Renders a sound as mono samples between -1 and 1 at `sample_rate`.
pub fn render(sound: Sound, sample_rate: u32) -> Vec<f32> {
    let rate = sample_rate.max(8000) as f32;
    let total = (duration(sound) * rate).ceil() as usize;
    let mut samples = vec![0.0f32; total];

    for tone in tones(sound) {
        let first = (tone.start * rate) as usize;
        let count = (tone.length * rate) as usize;
        // A few milliseconds of fade at each end removes the click a
        // hard start or stop would make.
        let attack = (0.004 * rate).min(count as f32 * 0.3);
        let mut phase = 0.0f32;
        for i in 0..count {
            let progress = i as f32 / count as f32;
            let frequency = tone.from + (tone.to - tone.from) * progress;
            phase += TAU * frequency / rate;
            let fade_in = (i as f32 / attack).min(1.0);
            // Squared so the tail falls away quickly, like a struck note.
            let fade_out = (1.0 - progress).powi(2);
            if let Some(slot) = samples.get_mut(first + i) {
                *slot += phase.sin() * tone.level * fade_in * fade_out;
            }
        }
    }

    // Overlapping tones could sum past full scale.
    for sample in &mut samples {
        *sample = sample.clamp(-1.0, 1.0);
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_is_short() {
        for sound in Sound::ALL {
            let d = duration(sound);
            assert!(d > 0.01 && d <= 0.2, "{sound:?} lasts {d}s");
        }
    }

    #[test]
    fn the_tick_is_short_enough_to_repeat_while_a_key_is_held() {
        // Key repeat is around 25 to 30 per second.
        assert!(duration(Sound::Tick) < 1.0 / 30.0);
    }

    #[test]
    fn length_follows_the_sample_rate() {
        for rate in [44_100u32, 48_000, 96_000] {
            let samples = render(Sound::Open, rate);
            let expected = (duration(Sound::Open) * rate as f32).ceil() as usize;
            assert_eq!(samples.len(), expected);
        }
    }

    #[test]
    fn samples_stay_in_range_and_are_finite() {
        for sound in Sound::ALL {
            for sample in render(sound, 48_000) {
                assert!(sample.is_finite());
                assert!((-1.0..=1.0).contains(&sample));
            }
        }
    }

    #[test]
    fn sounds_start_and_end_silent_so_they_do_not_click() {
        for sound in Sound::ALL {
            let samples = render(sound, 48_000);
            assert!(
                samples[0].abs() < 0.02,
                "{sound:?} starts at {}",
                samples[0]
            );
            let last = *samples.last().unwrap();
            assert!(last.abs() < 0.02, "{sound:?} ends at {last}");
        }
    }

    #[test]
    fn sounds_are_audible_but_never_loud() {
        for sound in Sound::ALL {
            let peak = render(sound, 48_000)
                .iter()
                .fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak > 0.1, "{sound:?} is nearly silent ({peak})");
            assert!(peak < 0.8, "{sound:?} is too loud ({peak})");
        }
    }

    #[test]
    fn an_absurd_sample_rate_does_not_panic() {
        assert!(!render(Sound::Tick, 0).is_empty());
        assert!(!render(Sound::Tick, 384_000).is_empty());
    }

    #[test]
    fn sounds_differ_from_each_other() {
        let tick = render(Sound::Tick, 48_000);
        let error = render(Sound::Error, 48_000);
        assert_ne!(tick.len(), error.len());
    }
}

//! Plays rendered sounds through the default audio output.
//!
//! The audio device is opened when the first sound is asked for and
//! closed again after a few seconds of silence. An idle shell therefore
//! holds no audio stream open, which lets the sound card sleep.

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::synth::{self, Sound};

/// Close the device after this long without a sound.
const IDLE_CLOSE: Duration = Duration::from_secs(4);
/// After a failure to open the device, wait this long before retrying.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Sounds playing at once. Holding a key cannot pile up more than this.
const MAX_VOICES: usize = 6;

/// A sound that is currently playing.
struct Voice {
    samples: Arc<[f32]>,
    position: usize,
    gain: f32,
}

type Voices = Arc<Mutex<Vec<Voice>>>;

/// Adds every playing voice into `out`, one mono value per frame
/// copied to all channels, and drops voices that have finished.
fn mix<T: SizedSample + FromSample<f32>>(out: &mut [T], channels: usize, voices: &mut Vec<Voice>) {
    for frame in out.chunks_mut(channels.max(1)) {
        let mut value = 0.0f32;
        for voice in voices.iter_mut() {
            if let Some(sample) = voice.samples.get(voice.position) {
                value += sample * voice.gain;
                voice.position += 1;
            }
        }
        let sample = T::from_sample(value.clamp(-1.0, 1.0));
        for slot in frame {
            *slot = sample;
        }
    }
    voices.retain(|v| v.position < v.samples.len());
}

/// An open audio device.
struct Output {
    // Dropping the stream closes the device.
    _stream: cpal::Stream,
    voices: Voices,
    sample_rate: u32,
    rendered: HashMap<Sound, Arc<[f32]>>,
}

impl Output {
    fn open() -> Result<Output, String> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| "no audio output device".to_string())?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let channels = config.channels as usize;
        let sample_rate = config.sample_rate;
        let voices: Voices = Arc::new(Mutex::new(Vec::new()));

        fn build<T: SizedSample + FromSample<f32>>(
            device: &cpal::Device,
            config: &cpal::StreamConfig,
            channels: usize,
            voices: Voices,
        ) -> Result<cpal::Stream, cpal::Error> {
            device.build_output_stream(
                *config,
                move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
                    // Never block the audio thread. If the lock is busy
                    // for this buffer, play silence and catch up next time.
                    match voices.try_lock() {
                        Ok(mut voices) => mix(out, channels, &mut voices),
                        Err(_) => out.fill(T::from_sample(0.0f32)),
                    }
                },
                |_error| {},
                None,
            )
        }

        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, &config, channels, Arc::clone(&voices)),
            SampleFormat::I16 => build::<i16>(&device, &config, channels, Arc::clone(&voices)),
            SampleFormat::U16 => build::<u16>(&device, &config, channels, Arc::clone(&voices)),
            SampleFormat::I32 => build::<i32>(&device, &config, channels, Arc::clone(&voices)),
            other => return Err(format!("unsupported sample format {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;

        Ok(Output {
            _stream: stream,
            voices,
            sample_rate,
            rendered: HashMap::new(),
        })
    }

    fn play(&mut self, sound: Sound, gain: f32) {
        let rate = self.sample_rate;
        let samples = self
            .rendered
            .entry(sound)
            .or_insert_with(|| synth::render(sound, rate).into())
            .clone();
        if let Ok(mut voices) = self.voices.lock() {
            if voices.len() >= MAX_VOICES {
                voices.remove(0);
            }
            voices.push(Voice {
                samples,
                position: 0,
                gain,
            });
        }
    }

    fn is_silent(&self) -> bool {
        self.voices.lock().map(|v| v.is_empty()).unwrap_or(true)
    }
}

/// A handle to the playback thread. Sending a sound never blocks.
#[derive(Clone)]
pub struct Player {
    tx: Sender<(Sound, f32)>,
}

impl Player {
    /// Starts the playback thread. No audio device is touched until the
    /// first sound is played.
    pub fn start() -> Player {
        let (tx, rx) = mpsc::channel::<(Sound, f32)>();
        thread::spawn(move || {
            let mut output: Option<Output> = None;
            let mut failed_at: Option<Instant> = None;
            loop {
                match rx.recv_timeout(IDLE_CLOSE) {
                    Ok((sound, gain)) => {
                        if output.is_none() {
                            // A machine with no sound card should not
                            // retry on every key press.
                            if failed_at.is_some_and(|at| at.elapsed() < RETRY_AFTER) {
                                continue;
                            }
                            match Output::open() {
                                Ok(opened) => {
                                    output = Some(opened);
                                    failed_at = None;
                                }
                                Err(_) => {
                                    failed_at = Some(Instant::now());
                                    continue;
                                }
                            }
                        }
                        if let Some(output) = output.as_mut() {
                            output.play(sound, gain);
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if output.as_ref().is_some_and(Output::is_silent) {
                            output = None;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        Player { tx }
    }

    /// Plays `sound` at `gain` (0 to 1). Does nothing if there is no
    /// working audio output.
    pub fn play(&self, sound: Sound, gain: f32) {
        let _ = self.tx.send((sound, gain.clamp(0.0, 1.0)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(samples: &[f32], gain: f32) -> Voice {
        Voice {
            samples: samples.into(),
            position: 0,
            gain,
        }
    }

    #[test]
    fn mixing_copies_each_value_to_every_channel() {
        let mut voices = vec![voice(&[0.5, -0.5], 1.0)];
        let mut out = [0.0f32; 6];
        mix(&mut out, 2, &mut voices);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5, 0.0, 0.0]);
    }

    #[test]
    fn finished_voices_are_removed() {
        let mut voices = vec![voice(&[0.1], 1.0), voice(&[0.1, 0.1, 0.1], 1.0)];
        let mut out = [0.0f32; 2];
        mix(&mut out, 1, &mut voices);
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].position, 2);
    }

    #[test]
    fn overlapping_voices_add_and_clip_safely() {
        let mut voices = vec![voice(&[0.8], 1.0), voice(&[0.8], 1.0)];
        let mut out = [0.0f32; 1];
        mix(&mut out, 1, &mut voices);
        assert_eq!(out, [1.0]);
    }

    #[test]
    fn gain_scales_the_voice() {
        let mut voices = vec![voice(&[1.0], 0.25)];
        let mut out = [0.0f32; 1];
        mix(&mut out, 1, &mut voices);
        assert_eq!(out, [0.25]);
    }

    #[test]
    fn integer_formats_are_converted() {
        let mut voices = vec![voice(&[1.0, -1.0, 0.0], 1.0)];
        let mut out = [0i16; 3];
        mix(&mut out, 1, &mut voices);
        assert_eq!(out[0], i16::MAX);
        assert!(out[1] <= -i16::MAX);
        assert_eq!(out[2], 0);
    }

    #[test]
    fn playing_with_no_audio_device_does_not_panic_or_block() {
        // The test machine may or may not have a sound card. Either way
        // sending must return at once.
        let player = Player::start();
        let started = Instant::now();
        for _ in 0..50 {
            player.play(Sound::Tick, 0.2);
        }
        assert!(started.elapsed() < Duration::from_millis(200));
    }
}

//! Works out whether sound is currently going to headphones.
//!
//! Interface sounds are pleasant in headphones and annoying from laptop
//! speakers in a quiet room, so by default Scuttle only plays them when
//! the audio output is something worn on the head.
//!
//! The answer comes from `pactl`, which works with both PipeWire (via
//! pipewire-pulse) and PulseAudio.

use std::process::{Command, Stdio};

use serde_json::Value;

/// Decides from `pactl --format=json list sinks` output and the name of
/// the default sink whether that sink is headphones or a headset.
///
/// Returns `None` when the text cannot be understood or the default
/// sink is not in it.
pub fn is_headphones(sinks_json: &str, default_sink: &str) -> Option<bool> {
    let sinks: Value = serde_json::from_str(sinks_json).ok()?;
    let sink = sinks
        .as_array()?
        .iter()
        .find(|s| s.get("name").and_then(Value::as_str) == Some(default_sink.trim()))?;

    let text = |value: Option<&Value>| value.and_then(Value::as_str).unwrap_or("").to_lowercase();
    let properties = sink.get("properties");
    let property = |key: &str| text(properties.and_then(|p| p.get(key)));

    // 1. The port in use says what is plugged in. This is how a laptop's
    //    headphone jack is told apart from its speakers.
    let active_port = text(sink.get("active_port"));
    if !active_port.is_empty() {
        let port = sink
            .get("ports")
            .and_then(Value::as_array)
            .and_then(|ports| ports.iter().find(|p| text(p.get("name")) == active_port));
        let port_type = text(port.and_then(|p| p.get("type")));
        if matches!(port_type.as_str(), "headphones" | "headset") {
            return Some(true);
        }
        if matches!(
            port_type.as_str(),
            "speaker" | "hdmi" | "tv" | "spdif" | "line"
        ) {
            return Some(false);
        }
        if active_port.contains("headphone") || active_port.contains("headset") {
            return Some(true);
        }
        if active_port.contains("speaker") || active_port.contains("hdmi") {
            return Some(false);
        }
    }

    // 2. The device itself may say what it is. Bluetooth and USB
    //    headsets report this way.
    let form_factor = property("device.form_factor");
    if matches!(form_factor.as_str(), "headphone" | "headset" | "hands-free") {
        return Some(true);
    }
    if matches!(
        form_factor.as_str(),
        "speaker" | "tv" | "hifi" | "car" | "portable"
    ) {
        return Some(false);
    }

    // 3. A Bluetooth output with no stated form factor is far more often
    //    earbuds than a speaker, and its icon name usually settles it.
    let icon = property("device.icon_name");
    if icon.contains("headphone") || icon.contains("headset") {
        return Some(true);
    }
    if icon.contains("speaker") {
        return Some(false);
    }

    Some(false)
}

fn run(args: &[&str]) -> Option<String> {
    let output = Command::new("pactl")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

/// Asks the running sound server. `None` means it could not be asked,
/// usually because `pactl` is not installed.
pub fn detect() -> Option<bool> {
    let default_sink = run(&["get-default-sink"])?;
    let sinks = run(&["--format=json", "list", "sinks"])?;
    is_headphones(&sinks, &default_sink)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A laptop's built-in sound card with speakers and a headphone
    /// jack, as PipeWire reports it.
    fn laptop(active_port: &str) -> String {
        format!(
            r#"[{{
              "index": 55, "state": "SUSPENDED",
              "name": "alsa_output.pci-0000_00_1f.3.analog-stereo",
              "description": "Built-in Audio Analog Stereo",
              "mute": false,
              "properties": {{
                "device.api": "alsa", "device.bus": "pci",
                "device.form_factor": "internal",
                "device.icon_name": "audio-card-analog-pci"
              }},
              "ports": [
                {{"name": "analog-output-speaker", "description": "Speakers", "type": "Speaker", "priority": 10000, "availability": "availability unknown"}},
                {{"name": "analog-output-headphones", "description": "Headphones", "type": "Headphones", "priority": 15000, "availability": "available"}}
              ],
              "active_port": "{active_port}"
            }}]"#
        )
    }

    const LAPTOP: &str = "alsa_output.pci-0000_00_1f.3.analog-stereo";

    fn single(name: &str, properties: &str) -> String {
        format!(
            r#"[{{"name": "{name}", "properties": {{{properties}}}, "ports": [], "active_port": null}}]"#
        )
    }

    #[test]
    fn laptop_speakers_are_not_headphones() {
        assert_eq!(
            is_headphones(&laptop("analog-output-speaker"), LAPTOP),
            Some(false)
        );
    }

    #[test]
    fn plugging_into_the_jack_counts() {
        assert_eq!(
            is_headphones(&laptop("analog-output-headphones"), LAPTOP),
            Some(true)
        );
    }

    #[test]
    fn bluetooth_earbuds_count() {
        let sinks = single(
            "bluez_output.AA_BB_CC_DD_EE_FF.1",
            r#""device.api": "bluez5", "device.bus": "bluetooth", "device.form_factor": "headset""#,
        );
        assert_eq!(
            is_headphones(&sinks, "bluez_output.AA_BB_CC_DD_EE_FF.1"),
            Some(true)
        );
    }

    #[test]
    fn bluetooth_speaker_does_not_count() {
        let sinks = single(
            "bluez_output.11_22_33_44_55_66.1",
            r#""device.bus": "bluetooth", "device.form_factor": "speaker""#,
        );
        assert_eq!(
            is_headphones(&sinks, "bluez_output.11_22_33_44_55_66.1"),
            Some(false)
        );
    }

    #[test]
    fn usb_headset_is_recognised_by_its_icon() {
        let sinks = single(
            "alsa_output.usb-Headset.analog-stereo",
            r#""device.bus": "usb", "device.icon_name": "audio-headset-usb""#,
        );
        assert_eq!(
            is_headphones(&sinks, "alsa_output.usb-Headset.analog-stereo"),
            Some(true)
        );
    }

    #[test]
    fn hdmi_monitor_does_not_count() {
        let sinks = r#"[{"name": "alsa_output.hdmi", "properties": {},
            "ports": [{"name": "hdmi-output-0", "type": "HDMI"}], "active_port": "hdmi-output-0"}]"#;
        assert_eq!(is_headphones(sinks, "alsa_output.hdmi"), Some(false));
    }

    #[test]
    fn an_unknown_device_is_assumed_to_be_speakers() {
        let sinks = single("mystery", r#""device.bus": "usb""#);
        assert_eq!(is_headphones(&sinks, "mystery"), Some(false));
    }

    #[test]
    fn only_the_default_sink_matters() {
        let sinks = r#"[
            {"name": "speakers", "properties": {"device.form_factor": "speaker"}, "ports": [], "active_port": null},
            {"name": "buds", "properties": {"device.form_factor": "headphone"}, "ports": [], "active_port": null}
        ]"#;
        assert_eq!(is_headphones(sinks, "speakers"), Some(false));
        assert_eq!(is_headphones(sinks, "buds\n"), Some(true));
    }

    #[test]
    fn unreadable_input_gives_no_answer() {
        assert_eq!(is_headphones("not json", "x"), None);
        assert_eq!(is_headphones("{}", "x"), None);
        assert_eq!(is_headphones("[]", "x"), None);
        assert_eq!(
            is_headphones(&laptop("analog-output-speaker"), "some-other-sink"),
            None
        );
    }
}

//! Turns theme tokens and config into the GTK stylesheet every Scuttle
//! surface is drawn with.
//!
//! The stylesheet is plain text with `$token` placeholders. Keeping it
//! as one readable template makes the look easy to audit, and the tests
//! below make sure no placeholder is ever left unfilled.

use crate::config::Config;
use crate::theme::{Mode, Theme};

/// Builds the stylesheet for the given theme and config.
///
/// `on_battery` applies the power rules: blur off and motion capped,
/// when the config asks for that.
pub fn stylesheet(theme: &Theme, config: &Config, on_battery: bool) -> String {
    let motion = config.effective_motion(on_battery);
    let blur = config.effective_blur(on_battery);
    let radius = config.appearance.radius;

    // Translucent panels need the compositor's blur behind them to stay
    // readable. Without blur they go nearly opaque.
    let panel_alpha = if blur { 0.82 } else { 0.97 };
    let scrim_alpha = if blur { 0.84 } else { 0.95 };
    let shadow = match theme.mode {
        Mode::Dark => "rgba(0, 0, 0, 0.40)",
        Mode::Light => "rgba(60, 45, 20, 0.22)",
    };

    let tokens: Vec<(&str, String)> = vec![
        (
            "font_display",
            font_stack(&config.appearance.font_display, "sans-serif"),
        ),
        (
            "font_mono",
            font_stack(&config.appearance.font_mono, "monospace"),
        ),
        ("font", font_stack(&config.appearance.font, "sans-serif")),
        ("ground", theme.ground.css()),
        ("surface", theme.surface.css()),
        ("panel", theme.surface.with_alpha(panel_alpha).css()),
        ("scrim", theme.ground.with_alpha(scrim_alpha).css()),
        ("line_strong", theme.text.with_alpha(0.20).css()),
        ("line", theme.line.css()),
        ("text_mid", theme.text_mid.css()),
        ("text_dim", theme.text_dim.css()),
        ("text", theme.text.css()),
        ("accent_soft", theme.accent_soft().css()),
        ("accent_edge", theme.accent_edge().css()),
        ("accent_text", theme.accent_text.css()),
        ("accent", theme.accent.css()),
        ("on_accent", theme.on_accent.css()),
        ("wash_strong", theme.wash_strong().css()),
        ("wash", theme.wash().css()),
        ("shadow", shadow.to_string()),
        ("radius_lg", format!("{}px", radius + 2)),
        ("radius_sm", format!("{}px", (radius * 2 / 3).max(2))),
        ("radius_xs", format!("{}px", (radius / 2).max(2))),
        ("radius", format!("{radius}px")),
        ("bar_height", format!("{}px", config.bar.height)),
        (
            "cell",
            format!("{}px", config.bar.height.saturating_sub(10)),
        ),
        ("dur", format!("{}ms", motion.duration_ms())),
        ("exit", format!("{}ms", motion.exit_ms())),
        (
            "toast_travel",
            format!("{}px", (motion.travel_px() * 3).min(40)),
        ),
        ("travel", format!("{}px", motion.travel_px())),
        (
            "stagger_rules",
            (1..=7)
                .map(|i| {
                    format!(
                        ".results > row.d{i} {{ animation-delay: {}ms; }}",
                        motion.stagger_ms() * i
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        (
            "pulse",
            // The "thinking" line breathes while waiting. With motion
            // off it just sits there.
            if motion.duration_ms() == 0 {
                "none".to_string()
            } else {
                "pulse 1400ms ease-in-out infinite".to_string()
            },
        ),
        ("ease", motion.css_ease()),
    ];
    scale_px(&fill(TEMPLATE, tokens), config.appearance.scale)
}

/// Rounds a base size in pixels to the size it has at `scale`.
///
/// Hairlines stay one pixel so borders remain crisp, and nothing that
/// was visible is ever scaled away to zero.
pub fn scaled(base: f32, scale: f32) -> i32 {
    if base.abs() <= 1.0 {
        return base.round() as i32;
    }
    let value = (base * scale).round() as i32;
    if value == 0 {
        base.signum() as i32
    } else {
        value
    }
}

/// Multiplies every `px` length in a stylesheet by `scale`.
fn scale_px(css: &str, scale: f32) -> String {
    if (scale - 1.0).abs() < 0.001 {
        return css.to_string();
    }
    let bytes = css.as_bytes();
    let mut out = String::with_capacity(css.len() + 64);
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let starts_number = c.is_ascii_digit()
            && (i == 0
                || !(bytes[i - 1].is_ascii_alphanumeric()
                    || bytes[i - 1] == b'.'
                    || bytes[i - 1] == b'#'
                    || bytes[i - 1] == b'_'));
        if !starts_number {
            // Copy one whole character, which may be several bytes.
            let end = (i + 1..=bytes.len())
                .find(|&j| css.is_char_boundary(j))
                .unwrap_or(bytes.len());
            out.push_str(&css[i..end]);
            i = end;
            continue;
        }
        let mut end = i;
        while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'.') {
            end += 1;
        }
        let number = &css[i..end];
        if css[end..].starts_with("px") {
            match number.parse::<f32>() {
                Ok(value) => out.push_str(&scaled(value, scale).to_string()),
                Err(_) => out.push_str(number),
            }
        } else {
            out.push_str(number);
        }
        i = end;
    }
    out
}

/// Replaces every `$name` in `template`. Longer names are substituted
/// first so `$text_dim` is never mistaken for `$text`.
fn fill(template: &str, mut tokens: Vec<(&str, String)>) -> String {
    tokens.sort_by_key(|(name, _)| std::cmp::Reverse(name.len()));
    let mut out = template.to_string();
    for (name, value) in tokens {
        out = out.replace(&format!("${name}"), &value);
    }
    out
}

/// A CSS font-family list with sensible fallbacks. The name comes from
/// the user's config, so anything that could break out of the quoted
/// string is removed.
fn font_stack(name: &str, generic: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let fallbacks = match generic {
        "monospace" => "\"JetBrains Mono\", \"DejaVu Sans Mono\", monospace",
        _ => "\"Inter\", \"Cantarell\", \"Noto Sans\", sans-serif",
    };
    if clean.is_empty() {
        fallbacks.to_string()
    } else {
        format!("\"{clean}\", {fallbacks}")
    }
}

const TEMPLATE: &str = r#"
/* Generated by Scuttle. Edit config.toml, not this. */

window.scuttle {
  background: none;
  color: $text;
  font-family: $font;
  font-size: 13px;
}

/* Neutralise whatever GTK theme is installed so every surface looks the
   same on every machine. */
.scuttle button {
  background: none;
  border: none;
  box-shadow: none;
  outline: none;
  min-height: 0;
  min-width: 0;
  padding: 0;
  margin: 0;
  color: inherit;
  font-weight: inherit;
}

.scuttle list, .scuttle row { background: none; outline: none; color: inherit; }

/* ---- Bar ------------------------------------------------------------ */

.bar {
  background-color: $panel;
  border: 1px solid $line;
  border-radius: $radius_sm;
  min-height: $bar_height;
  padding: 0 12px 0 5px;
}

.bar.notch {
  background-color: $ground;
  border: none;
  border-radius: 0 0 $radius_lg $radius_lg;
  padding: 0 20px;
}

.bar.notch.bottom { border-radius: $radius_lg $radius_lg 0 0; }

.scuttle .ws {
  min-width: $cell;
  min-height: $cell;
  padding: 0;
  border-radius: $radius_xs;
  font-family: $font_mono;
  font-weight: 500;
  color: $text_dim;
  transition: background-color $dur $ease, color $dur $ease;
}

.scuttle .ws.occupied { background-color: $wash; color: $text; }
.scuttle .ws:hover { background-color: $wash_strong; color: $text; }
.scuttle .ws.active, .scuttle .ws.active:hover { background-color: $accent; color: $on_accent; }

.bar-title { color: $text_mid; margin-left: 12px; }
.bar-subtitle { color: $text_dim; }
.clock { font-weight: 500; }

.module { color: $text_mid; }
.module .key { color: $text_dim; font-family: $font_mono; font-size: 12px; }
.module .value { font-family: $font_mono; font-size: 12px; }
.module.warn .value { color: $accent_text; }
.module.media image { color: $text_dim; }
.module.media.playing image { color: $accent_text; }

.scuttle .badge {
  padding: 0 6px;
  background-color: $accent;
  color: $on_accent;
  border-radius: 11px;
  min-width: 22px;
  min-height: 22px;
  font-size: 12px;
  font-weight: 600;
}

.scuttle .badge.muted {
  background-color: $wash_strong;
  color: $text_mid;
  font-family: $font_mono;
  font-weight: 500;
}

/* ---- Notifications -------------------------------------------------- */

.toast {
  background-color: $panel;
  border: 1px solid $line;
  border-radius: $radius;
  padding: 14px;
  opacity: 0;
  transform: translateX($toast_travel);
  transition: opacity $exit ease-in, transform $exit ease-in;
}

.from-left .toast { transform: translateX(-$toast_travel); }

.toast.shown {
  opacity: 1;
  transform: none;
  transition: opacity $dur ease-out, transform $dur $ease;
}

.toast.critical { border-color: $accent; }
.toast-title { font-size: 15px; font-weight: 600; color: $text; }
.toast-body { font-size: 13px; color: $text_dim; }

/* Level bars in toasts and the volume and brightness popup. */
.scuttle progressbar trough {
  min-height: 6px;
  border-radius: 3px;
  background-color: $wash_strong;
  border: none;
}

.scuttle progressbar progress {
  /* Some themes pull the fill outwards with negative margins. */
  margin: 0;
  min-height: 6px;
  border-radius: 3px;
  background-color: $accent;
  border: none;
}

.osd {
  background-color: $panel;
  border: 1px solid $line;
  border-radius: 30px;
  padding: 18px 24px;
  opacity: 0;
  transform: translateY($travel);
  transition: opacity $exit ease-in, transform $exit ease-in;
}

.osd.shown {
  opacity: 1;
  transform: none;
  transition: opacity $dur ease-out, transform $dur $ease;
}

.osd-value { font-family: $font_display; font-size: 18px; font-weight: 300; }

/* ---- Launcher ------------------------------------------------------- */

.scrim {
  background-color: $scrim;
  opacity: 0;
  transition: opacity $exit ease-in;
}

.scrim.open {
  opacity: 1;
  transition: opacity $dur ease-out;
}

.launcher {
  transform: translateY($travel) scale(0.985);
  transition: transform $exit ease-in;
}

.scrim.open .launcher {
  transform: none;
  transition: transform $dur $ease;
}

.query {
  font-family: $font_display;
  font-size: 60px;
  font-weight: 200;
  letter-spacing: -1px;
  color: $text;
  caret-color: $accent;
  background: none;
  border: none;
  border-radius: 0;
  box-shadow: none;
  outline: none;
  min-height: 78px;
  padding: 0;
}

.search-row {
  border-bottom: 2px solid $accent;
  padding: 0 4px 10px 4px;
}

.query:focus-within { outline: none; box-shadow: none; }
.query placeholder { color: $text_dim; }
.query selection { background-color: $accent; color: $on_accent; }

.results { background: none; }

.results > row {
  padding: 0;
  margin: 0 0 2px 0;
  border-radius: $radius_sm;
  background: none;
  outline: none;
  color: $text;
  transition: background-color $exit $ease;
}

@keyframes rise {
  from { opacity: 0; transform: translateY($travel); }
  to { opacity: 1; transform: none; }
}

.results > row.enter { animation: rise $dur $ease both; }
$stagger_rules

.results > row:hover { background-color: $wash; }
.results > row:selected { background-color: $accent; color: $on_accent; }

.result { min-height: 48px; padding: 0 14px; font-size: 16px; }
.result image { color: $text_mid; }
.result-meta { font-size: 13px; color: $text_dim; }
.results > row:selected .result-title { font-weight: 600; }
.results > row:selected .result-meta,
.results > row:selected .result image { color: $on_accent; }

.section {
  font-size: 11px;
  letter-spacing: 1px;
  text-transform: uppercase;
  color: $text_dim;
  padding: 14px 14px 4px 14px;
}

.detail {
  background-color: $wash;
  border: 1px solid $line;
  border-radius: $radius_lg;
  padding: 22px;
}

.detail-title { font-family: $font_display; font-size: 24px; font-weight: 300; }
.detail-sub { font-size: 13px; color: $text_dim; }
.detail-key { font-family: $font_mono; font-size: 12px; color: $text_dim; }

.detail-action { font-size: 14px; color: $text; }

.mode-chip {
  background-color: $accent;
  color: $on_accent;
  border-radius: $radius_xs;
  padding: 6px 12px;
  font-size: 14px;
  font-weight: 600;
}

.hint { font-size: 12px; color: $text_dim; }
.hint.mono { font-family: $font_mono; }
.confirm { font-size: 13px; color: $accent_text; }

.glance { font-size: 14px; color: $text; }
.glance .dim { color: $text_dim; }
.glance .active { color: $accent_text; }
.glance-clock { font-family: $font_display; font-size: 48px; font-weight: 200; }

/* ---- Assistant ------------------------------------------------------ */

.assistant {
  background-color: $panel;
  border: 1px solid $line;
  border-radius: $radius_lg;
  opacity: 0;
  transform: translateX($toast_travel);
  transition: opacity $exit ease-in, transform $exit ease-in;
}

.assistant.from-left { transform: translateX(-$toast_travel); }

.assistant.open {
  opacity: 1;
  transform: none;
  transition: opacity $dur ease-out, transform $dur $ease;
}

.assistant-head { padding: 16px 14px 10px 20px; }
.assistant-title { font-family: $font_display; font-size: 20px; font-weight: 300; }

.chip {
  background-color: $wash;
  color: $text_mid;
  border-radius: 12px;
  padding: 3px 10px;
  font-size: 12px;
}

.chip.fallback { background-color: $accent_soft; color: $accent_text; }

.scuttle button.icon {
  min-width: 32px;
  min-height: 32px;
  border-radius: $radius_xs;
  color: $text_dim;
}

.scuttle button.icon:hover { background-color: $wash; color: $text; }

.msgs { padding: 6px 18px 14px 18px; }

.msg { font-size: 15px; }
.msg.enter { animation: rise $dur $ease both; }

.msg.user {
  background-color: $wash_strong;
  border-radius: $radius;
  padding: 10px 14px;
  margin-left: 44px;
}

.msg selection { background-color: $accent; color: $on_accent; }
.msg link { color: $accent_text; }
.msg-marker { color: $text_dim; min-width: 18px; }

.msg-code {
  background-color: $wash;
  border-radius: $radius_sm;
  padding: 10px 12px;
  font-family: $font_mono;
  font-size: 13px;
}

.msg-error { font-size: 13px; color: $accent_text; }
.notice { font-size: 12px; color: $text_dim; }

@keyframes pulse {
  0% { opacity: 0.25; }
  50% { opacity: 1; }
  100% { opacity: 0.25; }
}

.thinking { font-size: 13px; color: $text_dim; animation: $pulse; }

.action-card {
  border: 1px solid $accent_edge;
  border-radius: $radius;
  padding: 12px;
}

.action-card.settled { border-color: $line; }
.action-why { font-size: 14px; color: $text; }
.action-status { font-size: 12px; color: $text_dim; }
.action-status.bad { color: $accent_text; }
.action-done { font-size: 13px; color: $text_mid; }
.action-done image { color: $accent_text; }

.scuttle button.suggestion {
  border: 1px solid $line_strong;
  border-radius: $radius_sm;
  padding: 10px 14px;
  font-size: 14px;
  color: $text_mid;
}

.scuttle button.suggestion:hover { background-color: $wash; color: $text; }

.assistant-empty-title { font-family: $font_display; font-size: 24px; font-weight: 300; }

.composer {
  border-top: 1px solid $line;
  padding: 12px 12px 12px 18px;
}

.composer textview, .composer textview text {
  background: none;
  color: $text;
  font-size: 15px;
  caret-color: $accent;
}

.composer textview selection { background-color: $accent; color: $on_accent; }
.composer-placeholder { font-size: 15px; color: $text_dim; }

.scuttle button.send {
  background-color: $accent;
  color: $on_accent;
  border-radius: $radius_xs;
  min-width: 38px;
  min-height: 38px;
}

.scuttle button.send:disabled { background-color: $wash_strong; color: $text_dim; }
.scuttle button.send.stop { background-color: $wash_strong; color: $text; }

/* ---- Settings ------------------------------------------------------- */

window.scuttle.settings { background-color: $ground; }

.settings-side {
  background-color: $surface;
  border-right: 1px solid $line;
  padding: 22px 12px;
}

.settings-heading {
  font-family: $font_display;
  font-size: 22px;
  font-weight: 300;
  padding: 0 10px 4px 10px;
}

.scuttle .settings-nav button {
  border-radius: $radius_xs;
  padding: 9px 10px;
  font-size: 14px;
  color: $text_mid;
}

.scuttle .settings-nav button:hover { background-color: $wash; color: $text; }

.scuttle .settings-nav button:checked {
  background-color: $accent;
  color: $on_accent;
  font-weight: 600;
}

.settings-top { padding: 24px 22px 14px 28px; }
.settings-title { font-family: $font_display; font-size: 28px; font-weight: 300; }
.settings-page { padding: 0 28px 20px 28px; }

.settings-row { padding: 14px 0; border-bottom: 1px solid $line; }
.settings-label { font-size: 15px; color: $text; }
.settings-help { font-size: 13px; color: $text_dim; }

/* The config line a row writes. */
.settings-line { font-family: $font_mono; font-size: 12px; color: $text_dim; }
.settings-readout { font-family: $font_mono; font-size: 13px; color: $text_mid; }

.scuttle button.settings-reset {
  font-size: 12px;
  color: $text_dim;
  padding: 2px 8px;
  border-radius: $radius_xs;
}

.scuttle button.settings-reset:hover { background-color: $wash; color: $accent_text; }

.settings-footer {
  border-top: 1px solid $line;
  padding: 12px 28px;
  font-size: 12px;
  color: $text_dim;
}

.settings-footer.bad { color: $accent_text; }

.segmented { background-color: $wash; border-radius: $radius_xs; padding: 3px; }

.scuttle .segmented button {
  padding: 6px 13px;
  border-radius: $radius_xs;
  font-size: 13px;
  color: $text_mid;
}

.scuttle .segmented button:hover { color: $text; }
.scuttle .segmented button:checked { background-color: $accent; color: $on_accent; font-weight: 600; }

.scuttle switch {
  background: none;
  background-color: $wash_strong;
  border: none;
  box-shadow: none;
  border-radius: 14px;
  min-width: 46px;
  min-height: 26px;
}

.scuttle switch:checked { background-color: $accent; }

.scuttle switch slider {
  background: none;
  background-color: $text;
  border: none;
  box-shadow: none;
  border-radius: 11px;
  min-width: 22px;
  min-height: 22px;
  margin: 2px;
}

.scuttle switch:checked slider { background-color: $on_accent; }
.scuttle switch image { opacity: 0; }

.scuttle scale { padding: 8px 0; }

.scuttle scale trough {
  background-color: $wash_strong;
  border: none;
  border-radius: 2px;
  min-height: 4px;
}

.scuttle scale highlight {
  /* Some themes pull the fill outwards with negative margins. */
  margin: 0;
  min-height: 4px;
  min-width: 4px;
  background-color: $accent;
  border: none;
  border-radius: 2px;
}

.scuttle scale slider {
  background: none;
  background-color: $text;
  border: none;
  box-shadow: none;
  border-radius: 9px;
  min-width: 18px;
  min-height: 18px;
  margin: -7px;
}

.scuttle entry.settings-entry {
  background: none;
  background-color: $wash;
  color: $text;
  caret-color: $accent;
  border: 1px solid $line;
  box-shadow: none;
  outline: none;
  border-radius: $radius_xs;
  min-height: 36px;
  padding: 0 10px;
  font-size: 13px;
}

.scuttle entry.settings-entry:focus-within { border-color: $accent_edge; }
.scuttle entry.settings-entry selection { background-color: $accent; color: $on_accent; }
.scuttle entry.settings-entry placeholder { color: $text_dim; }

.settings button:focus-visible,
.settings switch:focus-visible,
.settings scale:focus-visible { outline: 2px solid $accent_edge; outline-offset: 2px; }

/* ---- Shared pieces -------------------------------------------------- */

.panel {
  background-color: $panel;
  border: 1px solid $line;
  border-radius: $radius;
  box-shadow: 0 20px 50px $shadow;
}

.scuttle button.primary {
  background: none;
  background-color: $accent;
  color: $on_accent;
  border: none;
  box-shadow: none;
  border-radius: $radius_xs;
  min-height: 44px;
  padding: 0 16px;
  font-weight: 600;
}

.scuttle button.quiet {
  background: none;
  color: $text;
  border: 1px solid $line_strong;
  box-shadow: none;
  border-radius: $radius_xs;
  min-height: 44px;
  padding: 0 16px;
}

.scuttle button.quiet:hover { background-color: $wash; }

/* After the two above, so it can make either of them smaller. */
.scuttle button.small { min-height: 32px; padding: 0 14px; font-size: 13px; }

/* The assistant panel can be driven from the keyboard, so it shows
   where focus is. Last, so no button style above can hide it. */
.scuttle .assistant button:focus-visible { outline: 2px solid $accent_edge; outline-offset: 2px; }
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::Motion;

    fn sheet(config: &Config, on_battery: bool) -> String {
        stylesheet(&config.theme(), config, on_battery)
    }

    #[test]
    fn no_placeholder_is_left_unfilled() {
        let css = sheet(&Config::default(), false);
        assert!(!css.contains('$'), "unfilled token in:\n{css}");
    }

    #[test]
    fn every_theme_and_mode_fills_completely() {
        for name in crate::theme::BUILTIN {
            for mode in [Mode::Dark, Mode::Light] {
                let mut c = Config::default();
                c.appearance.theme = name.to_string();
                c.appearance.mode = mode;
                assert!(!sheet(&c, false).contains('$'), "{name} {mode:?}");
            }
        }
    }

    #[test]
    fn accent_reaches_the_stylesheet() {
        let mut c = Config::default();
        c.appearance.accent = Some("#3355ff".parse().unwrap());
        assert!(sheet(&c, false).contains("#3355ff"));
        assert!(!sheet(&c, false).contains("#f74c00"));
    }

    #[test]
    fn motion_off_removes_all_durations() {
        let mut c = Config::default();
        c.appearance.motion = Motion::Off;
        let css = sheet(&c, false);
        assert!(css.contains("0ms"));
        assert!(!css.contains("170ms") && !css.contains("280ms"));
    }

    #[test]
    fn battery_makes_panels_opaque_and_motion_subtle() {
        let mut c = Config::default();
        c.appearance.motion = Motion::Full;
        let plugged = sheet(&c, false);
        let battery = sheet(&c, true);
        assert!(plugged.contains("280ms"));
        assert!(!battery.contains("280ms") && battery.contains("170ms"));
        assert!(plugged.contains("0.820") && battery.contains("0.970"));
    }

    #[test]
    fn font_names_cannot_break_out_of_the_stylesheet() {
        let mut c = Config::default();
        c.appearance.font = "Evil\"; } * { color: red".into();
        let css = sheet(&c, false);
        assert!(!css.contains("color: red\""));
        assert!(css.contains("\"Evil color red\""));
    }

    #[test]
    fn scale_multiplies_pixel_lengths_only() {
        let css = "a { font-size: 13px; padding: 0 12px 4px; opacity: 0.5; transition: x 170ms; color: #f74c00; }";
        assert_eq!(
            scale_px(css, 2.0),
            "a { font-size: 26px; padding: 0 24px 8px; opacity: 0.5; transition: x 340ms; color: #f74c00; }"
                .replace("340ms", "170ms")
        );
    }

    #[test]
    fn hairlines_stay_one_pixel() {
        assert_eq!(
            scale_px("border: 1px solid; margin: -1px;", 2.0),
            "border: 1px solid; margin: -1px;"
        );
        assert_eq!(scaled(1.0, 2.5), 1);
        assert_eq!(scaled(2.0, 1.5), 3);
        assert_eq!(scaled(0.0, 2.0), 0);
        assert_eq!(scaled(3.0, 0.1), 1, "never scaled away to nothing");
    }

    #[test]
    fn scale_of_one_changes_nothing() {
        let mut c = Config::default();
        c.appearance.scale = 1.0;
        let plain = sheet(&c, false);
        assert!(plain.contains("font-size: 13px"));
        c.appearance.scale = 1.5;
        let bigger = sheet(&c, false);
        assert!(bigger.contains("font-size: 20px"), "13px text becomes 20px");
        assert!(!bigger.contains("font-size: 13px"));
        // Colours and timings must come through untouched.
        assert!(bigger.contains("#f74c00") && bigger.contains("170ms"));
    }

    #[test]
    fn longer_tokens_win_over_their_prefixes() {
        let out = fill(
            "$text_dim $text",
            vec![("text", "A".into()), ("text_dim", "B".into())],
        );
        assert_eq!(out, "B A");
    }
}

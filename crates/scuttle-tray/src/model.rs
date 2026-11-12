//! What a tray icon is, independent of how it arrived.

/// How much an item wants to be seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    /// Nothing to report. Still shown, since hiding it would make the
    /// app unreachable.
    Passive,
    #[default]
    Active,
    /// The app wants the user's attention.
    NeedsAttention,
}

impl Status {
    pub fn parse(text: &str) -> Status {
        match text {
            "Passive" => Status::Passive,
            "NeedsAttention" => Status::NeedsAttention,
            _ => Status::Active,
        }
    }
}

/// An image sent as raw pixels: 8-bit RGBA, row by row.
#[derive(Clone, PartialEq, Eq)]
pub struct Pixels {
    pub width: i32,
    pub height: i32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The pixel data would drown out everything else.
        write!(f, "Pixels({}x{})", self.width, self.height)
    }
}

/// How an item's icon is given.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Icon {
    /// The name of an icon in the icon theme, or the path of an image
    /// file. `theme_path` is an extra folder to look for it in.
    Named {
        name: String,
        theme_path: Option<String>,
    },
    Pixels(Pixels),
    /// The app sent nothing usable.
    #[default]
    Missing,
}

/// One icon in the tray.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Item {
    /// Identifies the item on the bus: its service name followed by
    /// its object path.
    pub key: String,
    /// The app's own name for the item, e.g. "discord".
    pub id: String,
    pub title: String,
    pub status: Status,
    pub icon: Icon,
    /// What to show on hover.
    pub tooltip: String,
    /// The object path of the item's menu, when it has one.
    pub menu: Option<String>,
    /// The item has nothing to activate, only a menu, so a plain click
    /// should open the menu.
    pub is_menu: bool,
}

impl Item {
    /// The words that best name this item: the tooltip if it has one,
    /// then the title, then the app's id.
    pub fn label(&self) -> &str {
        [&self.tooltip, &self.title, &self.id]
            .into_iter()
            .find(|text| !text.trim().is_empty())
            .map(|text| text.as_str())
            .unwrap_or("")
    }
}

/// Splits an item key back into its service name and object path.
pub fn split_key(key: &str) -> Option<(&str, &str)> {
    let slash = key.find('/')?;
    let (service, path) = key.split_at(slash);
    (!service.is_empty()).then_some((service, path))
}

/// Works out which item an app is registering.
///
/// Apps do this two ways. Most pass their bus name and keep the item
/// at the standard path. Some pass the item's object path instead, and
/// the bus name is whoever sent the call.
pub fn registration(sender: &str, argument: &str) -> Option<(String, String)> {
    const STANDARD_PATH: &str = "/StatusNotifierItem";
    let argument = argument.trim();
    if sender.is_empty() && !argument.starts_with(':') && !argument.contains('.') {
        return None;
    }
    if argument.starts_with('/') {
        return (!sender.is_empty()).then(|| (sender.to_string(), argument.to_string()));
    }
    if argument.is_empty() {
        return None;
    }
    // A name followed by a path, as this crate writes its own keys.
    if let Some((service, path)) = split_key(argument) {
        return Some((service.to_string(), path.to_string()));
    }
    Some((argument.to_string(), STANDARD_PATH.to_string()))
}

/// Converts pixels from the order the protocol sends them in (alpha,
/// red, green, blue) to RGBA.
pub fn argb_to_rgba(argb: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(argb.len());
    for pixel in argb.chunks_exact(4) {
        rgba.extend_from_slice(&[pixel[1], pixel[2], pixel[3], pixel[0]]);
    }
    rgba
}

/// Picks the image to use from the sizes an app offers: the smallest
/// one that is at least `wanted` pixels, so it is scaled down rather
/// than up, or the largest when all are smaller. Images whose data
/// does not match their stated size are ignored.
pub fn best_pixmap(offered: &[(i32, i32, Vec<u8>)], wanted: i32) -> Option<Pixels> {
    let usable = offered.iter().filter(|(width, height, data)| {
        *width > 0
            && *height > 0
            && *width <= 1024
            && *height <= 1024
            && data.len() == (*width as usize) * (*height as usize) * 4
    });
    let (big_enough, too_small): (Vec<_>, Vec<_>) =
        usable.partition(|(width, height, _)| (*width).min(*height) >= wanted);
    let chosen = big_enough
        .into_iter()
        .min_by_key(|(width, _, _)| *width)
        .or_else(|| too_small.into_iter().max_by_key(|(width, _, _)| *width))?;
    Some(Pixels {
        width: chosen.0,
        height: chosen.1,
        rgba: argb_to_rgba(&chosen.2),
    })
}

/// Chooses an item's icon from what it sent. A named icon wins over
/// pixels, because it follows the icon theme and stays sharp.
pub fn choose_icon(
    name: &str,
    theme_path: &str,
    pixmaps: &[(i32, i32, Vec<u8>)],
    wanted: i32,
) -> Icon {
    let name = name.trim();
    if !name.is_empty() {
        return Icon::Named {
            name: name.to_string(),
            theme_path: Some(theme_path.trim())
                .filter(|path| !path.is_empty())
                .map(str::to_string),
        };
    }
    match best_pixmap(pixmaps, wanted) {
        Some(pixels) => Icon::Pixels(pixels),
        None => Icon::Missing,
    }
}

/// Removes markup from a tooltip. Apps are allowed a little HTML
/// there, and a bar tooltip shows plain text.
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_words_are_read_and_unknown_ones_count_as_active() {
        assert_eq!(Status::parse("Passive"), Status::Passive);
        assert_eq!(Status::parse("Active"), Status::Active);
        assert_eq!(Status::parse("NeedsAttention"), Status::NeedsAttention);
        assert_eq!(Status::parse("something new"), Status::Active);
    }

    #[test]
    fn an_app_can_register_by_name() {
        assert_eq!(
            registration(":1.42", "org.kde.StatusNotifierItem-4077-1"),
            Some((
                "org.kde.StatusNotifierItem-4077-1".to_string(),
                "/StatusNotifierItem".to_string()
            ))
        );
        assert_eq!(
            registration(":1.42", ":1.42"),
            Some((":1.42".to_string(), "/StatusNotifierItem".to_string()))
        );
    }

    #[test]
    fn an_app_can_register_by_path() {
        assert_eq!(
            registration(":1.77", "/org/ayatana/NotificationItem/nm_applet"),
            Some((
                ":1.77".to_string(),
                "/org/ayatana/NotificationItem/nm_applet".to_string()
            ))
        );
    }

    #[test]
    fn nonsense_registrations_are_refused() {
        assert_eq!(registration(":1.5", ""), None);
        assert_eq!(registration(":1.5", "   "), None);
        assert_eq!(registration("", "/some/path"), None);
        assert_eq!(registration("", "noname"), None);
    }

    #[test]
    fn keys_split_back_into_service_and_path() {
        assert_eq!(
            split_key(":1.42/StatusNotifierItem"),
            Some((":1.42", "/StatusNotifierItem"))
        );
        assert_eq!(
            split_key("org.app.Tray/org/ayatana/Item"),
            Some(("org.app.Tray", "/org/ayatana/Item"))
        );
        assert_eq!(split_key("/no/service"), None);
        assert_eq!(split_key("nopath"), None);
        // A key round-trips through registration.
        assert_eq!(
            registration(":1.9", ":1.42/StatusNotifierItem"),
            Some((":1.42".to_string(), "/StatusNotifierItem".to_string()))
        );
    }

    #[test]
    fn pixels_are_reordered_from_argb() {
        // One opaque orange pixel and one half-transparent blue one.
        let argb = [0xff, 0xf7, 0x4c, 0x00, 0x80, 0x00, 0x00, 0xff];
        assert_eq!(
            argb_to_rgba(&argb),
            [0xf7, 0x4c, 0x00, 0xff, 0x00, 0x00, 0xff, 0x80]
        );
        // A trailing partial pixel is dropped, not read out of bounds.
        assert_eq!(argb_to_rgba(&[1, 2, 3, 4, 5]).len(), 4);
    }

    fn image(size: i32) -> (i32, i32, Vec<u8>) {
        (size, size, vec![0xff; (size * size * 4) as usize])
    }

    #[test]
    fn the_icon_closest_above_the_wanted_size_is_chosen() {
        let offered = [image(16), image(22), image(32), image(48)];
        assert_eq!(best_pixmap(&offered, 22).unwrap().width, 22);
        assert_eq!(best_pixmap(&offered, 24).unwrap().width, 32);
        assert_eq!(best_pixmap(&offered, 16).unwrap().width, 16);
        // Nothing big enough: take the biggest there is.
        assert_eq!(best_pixmap(&offered, 96).unwrap().width, 48);
        assert_eq!(best_pixmap(&[], 22), None);
    }

    #[test]
    fn broken_images_are_ignored() {
        let lying = (32, 32, vec![0u8; 10]);
        let empty = (0, 0, Vec::new());
        let negative = (-4, 4, vec![0u8; 64]);
        let huge = (4096, 4096, Vec::new());
        assert_eq!(
            best_pixmap(&[lying.clone(), empty, negative, huge], 22),
            None
        );
        assert_eq!(best_pixmap(&[lying, image(16)], 22).unwrap().width, 16);
    }

    #[test]
    fn a_named_icon_wins_over_pixels() {
        let pixmaps = [image(22)];
        assert_eq!(
            choose_icon("discord-tray", "", &pixmaps, 22),
            Icon::Named {
                name: "discord-tray".into(),
                theme_path: None
            }
        );
        assert_eq!(
            choose_icon("steam", "/opt/steam/icons", &[], 22),
            Icon::Named {
                name: "steam".into(),
                theme_path: Some("/opt/steam/icons".into())
            }
        );
        assert!(matches!(
            choose_icon("  ", "", &pixmaps, 22),
            Icon::Pixels(_)
        ));
        assert_eq!(choose_icon("", "", &[], 22), Icon::Missing);
    }

    #[test]
    fn the_label_falls_back_from_tooltip_to_title_to_id() {
        let mut item = Item {
            id: "nm-applet".into(),
            title: "Network".into(),
            tooltip: "Wired connection 1".into(),
            ..Item::default()
        };
        assert_eq!(item.label(), "Wired connection 1");
        item.tooltip = "  ".into();
        assert_eq!(item.label(), "Network");
        item.title.clear();
        assert_eq!(item.label(), "nm-applet");
        item.id.clear();
        assert_eq!(item.label(), "");
    }

    #[test]
    fn tooltips_lose_their_markup() {
        assert_eq!(
            plain("<b>Steam</b>  3 friends &amp; 1 update"),
            "Steam 3 friends & 1 update"
        );
        assert_eq!(plain("plain"), "plain");
        assert_eq!(plain("a > b"), "a > b");
        assert_eq!(plain("line one\nline two"), "line one line two");
    }

    #[test]
    fn pixel_data_stays_out_of_debug_output() {
        let pixels = Pixels {
            width: 22,
            height: 22,
            rgba: vec![0; 22 * 22 * 4],
        };
        assert_eq!(format!("{pixels:?}"), "Pixels(22x22)");
    }
}

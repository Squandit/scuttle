//! Clipboard history, read from `cliphist`.
//!
//! cliphist is the small daemon most Hyprland setups already run to
//! remember what was copied. Scuttle shows its history and restores an
//! entry on request, and does not keep a second copy of anyone's
//! clipboard.

/// One remembered clipboard entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipEntry {
    /// The exact line cliphist printed. Handing it back is how an
    /// entry is restored.
    pub line: String,
    /// What to show: the text on one line, or a description of an image.
    pub preview: String,
    pub is_image: bool,
}

/// Parses the output of `cliphist list`: one entry per line, an id,
/// a tab, then a preview. Newest first.
pub fn parse_list(output: &str) -> Vec<ClipEntry> {
    output
        .lines()
        .filter_map(|line| {
            let (id, preview) = line.split_once('\t')?;
            if id.trim().is_empty() || id.parse::<u64>().is_err() {
                return None;
            }
            let preview = preview.trim();
            if preview.is_empty() {
                return None;
            }
            match describe_binary(preview) {
                Some(description) => Some(ClipEntry {
                    line: line.to_string(),
                    preview: description,
                    is_image: true,
                }),
                None => Some(ClipEntry {
                    line: line.to_string(),
                    // Collapse runs of whitespace so a copied paragraph
                    // reads as one tidy line.
                    preview: preview.split_whitespace().collect::<Vec<_>>().join(" "),
                    is_image: false,
                }),
            }
        })
        .collect()
}

/// Turns cliphist's `[[ binary data 12 KiB png 800x600 ]]` marker into
/// something readable such as `Image, 800x600 png, 12 KiB`.
fn describe_binary(preview: &str) -> Option<String> {
    let inner = preview.strip_prefix("[[")?.strip_suffix("]]")?.trim();
    let rest = inner.strip_prefix("binary data")?.trim();
    let parts: Vec<&str> = rest.split_whitespace().collect();
    match parts.as_slice() {
        [size, unit, format, dimensions] => {
            Some(format!("Image, {dimensions} {format}, {size} {unit}"))
        }
        [size, unit, format] => Some(format!("Image, {format}, {size} {unit}")),
        _ => Some("Image".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_entries_newest_first() {
        let entries = parse_list("42\tcargo run -p scuttle\n41\thttps://wiki.hypr.land\n");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].preview, "cargo run -p scuttle");
        assert_eq!(entries[0].line, "42\tcargo run -p scuttle");
        assert!(!entries[0].is_image);
    }

    #[test]
    fn describes_images() {
        let entries = parse_list("7\t[[ binary data 12 KiB png 800x600 ]]\n");
        assert_eq!(entries[0].preview, "Image, 800x600 png, 12 KiB");
        assert!(entries[0].is_image);
        // The original line is kept untouched for restoring.
        assert_eq!(entries[0].line, "7\t[[ binary data 12 KiB png 800x600 ]]");
        assert_eq!(
            parse_list("8\t[[ binary data 3 MiB jpeg ]]\n")[0].preview,
            "Image, jpeg, 3 MiB"
        );
        assert_eq!(parse_list("9\t[[ binary data ]]\n")[0].preview, "Image");
    }

    #[test]
    fn text_that_merely_has_brackets_is_still_text() {
        let entries = parse_list("5\t[[ wiki link ]]\n6\tarr[[0]]\n");
        assert!(entries.iter().all(|e| !e.is_image));
    }

    #[test]
    fn multi_space_and_tabbed_text_is_tidied_for_display() {
        let entries = parse_list("3\tfn main()   {\tprintln!(); }\n");
        assert_eq!(entries[0].preview, "fn main() { println!(); }");
        assert_eq!(entries[0].line, "3\tfn main()   {\tprintln!(); }");
    }

    #[test]
    fn junk_lines_are_skipped() {
        assert!(parse_list("").is_empty());
        assert!(parse_list("no tab here\n\tno id\nabc\tnot a number\n12\t   \n").is_empty());
    }
}

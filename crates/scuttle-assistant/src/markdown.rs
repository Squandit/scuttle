//! Just enough Markdown to show an answer well in a narrow panel:
//! paragraphs, lists, code blocks, and inline code, bold, italics and
//! links.
//!
//! Inline text comes out as Pango markup. Whatever goes in, the markup
//! that comes out is well formed, including for half an answer that is
//! still streaming in.

/// A piece of an answer, top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// A paragraph, as Pango markup.
    Para(String),
    /// A list item: its marker ("•" or "2.") and its text as markup.
    Item { marker: String, markup: String },
    /// A code block, as plain text.
    Code { language: String, text: String },
}

/// Escapes text for use inside Pango markup.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Finds `needle` in `chars` at or after `from`.
fn find(chars: &[char], from: usize, needle: &[char]) -> Option<usize> {
    if needle.is_empty() || chars.len() < needle.len() {
        return None;
    }
    (from..=chars.len() - needle.len()).find(|&i| chars[i..i + needle.len()] == *needle)
}

/// Turns one run of inline Markdown into Pango markup. `mono` is the
/// font family for inline code.
pub fn inline(text: &str, mono: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    inline_chars(&chars, &escape(mono))
}

fn inline_chars(chars: &[char], mono: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    let plain = |slice: &[char]| escape(&slice.iter().collect::<String>());
    while i < chars.len() {
        match chars[i] {
            '`' => {
                if let Some(end) = find(chars, i + 1, &['`']) {
                    if end > i + 1 {
                        out.push_str(&format!(
                            "<span font_family=\"{mono}\">{}</span>",
                            plain(&chars[i + 1..end])
                        ));
                        i = end + 1;
                        continue;
                    }
                }
                out.push('`');
                i += 1;
            }
            '*' if chars.get(i + 1) == Some(&'*') => match find(chars, i + 2, &['*', '*']) {
                Some(end) if end > i + 2 => {
                    out.push_str(&format!(
                        "<b>{}</b>",
                        inline_chars(&chars[i + 2..end], mono)
                    ));
                    i = end + 2;
                }
                _ => {
                    out.push_str("**");
                    i += 2;
                }
            },
            '*' => {
                // Italics only when the stars hug the words, so "2 * 3"
                // and a stray star are left alone.
                let opens = chars.get(i + 1).is_some_and(|c| !c.is_whitespace());
                let close = find(chars, i + 1, &['*'])
                    .filter(|&end| end > i + 1 && !chars[end - 1].is_whitespace());
                match close {
                    Some(end) if opens => {
                        out.push_str(&format!(
                            "<i>{}</i>",
                            inline_chars(&chars[i + 1..end], mono)
                        ));
                        i = end + 1;
                    }
                    _ => {
                        out.push('*');
                        i += 1;
                    }
                }
            }
            '[' => {
                let link = find(chars, i + 1, &[']', '(']).and_then(|mid| {
                    let end = find(chars, mid + 2, &[')'])?;
                    let url: String = chars[mid + 2..end].iter().collect();
                    let safe = url.starts_with("https://") || url.starts_with("http://");
                    (safe && mid > i + 1 && !url.contains(char::is_whitespace))
                        .then_some((mid, end, url))
                });
                match link {
                    Some((mid, end, url)) => {
                        out.push_str(&format!(
                            "<a href=\"{}\">{}</a>",
                            escape(&url),
                            inline_chars(&chars[i + 1..mid], mono)
                        ));
                        i = end + 1;
                    }
                    None => {
                        out.push('[');
                        i += 1;
                    }
                }
            }
            c => {
                out.push_str(&escape(c.encode_utf8(&mut [0; 4])));
                i += 1;
            }
        }
    }
    out
}

/// The marker and text of a list item, if `line` is one.
fn list_item(line: &str) -> Option<(String, &str)> {
    let trimmed = line.trim_start();
    for bullet in ["- ", "* ", "• "] {
        if let Some(rest) = trimmed.strip_prefix(bullet) {
            return Some(("•".to_string(), rest));
        }
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if (1..=3).contains(&digits) {
        if let Some(rest) = trimmed[digits..].strip_prefix(". ") {
            return Some((format!("{}.", &trimmed[..digits]), rest));
        }
    }
    None
}

/// Splits an answer into blocks.
pub fn blocks(text: &str, mono: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut code: Option<(String, Vec<&str>)> = None;

    fn flush(para: &mut Vec<&str>, out: &mut Vec<Block>, mono: &str) {
        if !para.is_empty() {
            out.push(Block::Para(inline(&para.join("\n"), mono)));
            para.clear();
        }
    }

    for line in text.lines() {
        if let Some((language, lines)) = code.as_mut() {
            if line.trim_start().starts_with("```") {
                out.push(Block::Code {
                    language: std::mem::take(language),
                    text: lines.join("\n"),
                });
                code = None;
            } else {
                lines.push(line);
            }
            continue;
        }
        let trimmed = line.trim_start();
        if let Some(language) = trimmed.strip_prefix("```") {
            flush(&mut para, &mut out, mono);
            code = Some((language.trim().to_string(), Vec::new()));
        } else if trimmed.is_empty() {
            flush(&mut para, &mut out, mono);
        } else if let Some((marker, rest)) = list_item(line) {
            flush(&mut para, &mut out, mono);
            out.push(Block::Item {
                marker,
                markup: inline(rest, mono),
            });
        } else if trimmed.starts_with('#') {
            // Headings are too loud for a side panel. Show them as a
            // bold line.
            flush(&mut para, &mut out, mono);
            let title = trimmed.trim_start_matches('#').trim();
            if !title.is_empty() {
                out.push(Block::Para(format!("<b>{}</b>", inline(title, mono))));
            }
        } else {
            para.push(line.trim_end());
        }
    }
    flush(&mut para, &mut out, mono);
    // A code block still being streamed is shown as far as it has got.
    if let Some((language, lines)) = code {
        out.push(Block::Code {
            language,
            text: lines.join("\n"),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONO: &str = "Geist Mono";

    fn para(markup: &str) -> Block {
        Block::Para(markup.to_string())
    }

    /// A small well-formedness check: every tag opened is closed in
    /// order, and no raw `<` or `&` slips through.
    fn well_formed(markup: &str) -> bool {
        let mut stack = Vec::new();
        let mut rest = markup;
        while let Some(at) = rest.find(['<', '&']) {
            if rest[at..].starts_with('&') {
                let known = ["&amp;", "&lt;", "&gt;", "&quot;", "&apos;"];
                if !known.iter().any(|k| rest[at..].starts_with(k)) {
                    return false;
                }
                rest = &rest[at + 1..];
                continue;
            }
            let Some(end) = rest[at..].find('>') else {
                return false;
            };
            let tag = &rest[at + 1..at + end];
            if let Some(name) = tag.strip_prefix('/') {
                if stack.pop() != Some(name.to_string()) {
                    return false;
                }
            } else {
                stack.push(tag.split(' ').next().unwrap_or("").to_string());
            }
            rest = &rest[at + end + 1..];
        }
        stack.is_empty()
    }

    #[test]
    fn plain_text_is_escaped() {
        assert_eq!(
            inline("a < b && c > \"d\"", MONO),
            "a &lt; b &amp;&amp; c &gt; &quot;d&quot;"
        );
    }

    #[test]
    fn inline_code_bold_and_italics() {
        assert_eq!(
            inline("Run `ls -la` now", MONO),
            "Run <span font_family=\"Geist Mono\">ls -la</span> now"
        );
        assert_eq!(inline("**Stop** it", MONO), "<b>Stop</b> it");
        assert_eq!(
            inline("that is *very* loud", MONO),
            "that is <i>very</i> loud"
        );
        assert_eq!(
            inline("**use `pkill`**", MONO),
            "<b>use <span font_family=\"Geist Mono\">pkill</span></b>"
        );
    }

    #[test]
    fn code_is_never_parsed_further() {
        assert_eq!(
            inline("`a **b** <c>`", MONO),
            "<span font_family=\"Geist Mono\">a **b** &lt;c&gt;</span>"
        );
    }

    #[test]
    fn stray_markers_stay_as_typed() {
        assert_eq!(inline("2 * 3 * 4", MONO), "2 * 3 * 4");
        assert_eq!(inline("a ** b", MONO), "a ** b");
        assert_eq!(inline("snake_case_name", MONO), "snake_case_name");
        assert_eq!(inline("an unclosed `tick", MONO), "an unclosed `tick");
        assert_eq!(inline("an unclosed **bold", MONO), "an unclosed **bold");
        assert_eq!(inline("[not a link]", MONO), "[not a link]");
    }

    #[test]
    fn links_are_kept_only_for_the_web() {
        assert_eq!(
            inline(
                "see [the wiki](https://wiki.archlinux.org/title/Fan?a=1&b=2)",
                MONO
            ),
            "see <a href=\"https://wiki.archlinux.org/title/Fan?a=1&amp;b=2\">the wiki</a>"
        );
        assert_eq!(
            inline("[x](javascript:alert(1))", MONO),
            "[x](javascript:alert(1))"
        );
        assert_eq!(
            inline("[x](file:///etc/passwd)", MONO),
            "[x](file:///etc/passwd)"
        );
    }

    #[test]
    fn the_font_name_cannot_break_out_of_its_attribute() {
        let markup = inline("`x`", "Evil\" size=\"99");
        assert!(well_formed(&markup), "{markup}");
        assert!(markup.contains("Evil&quot; size=&quot;99"));
    }

    #[test]
    fn markup_is_well_formed_for_awkward_and_partial_input() {
        for text in [
            "**bold with `code` and *italic***",
            "*a **b* c**",
            "half a **bold",
            "half a `code",
            "<script>alert('x')</script> & more",
            "[a](https://x.y) [b](https://",
            "***",
            "`",
            "**`**`**",
            "",
        ] {
            let markup = inline(text, MONO);
            assert!(well_formed(&markup), "{text:?} gave {markup:?}");
        }
    }

    #[test]
    fn paragraphs_lists_and_code_blocks() {
        let text = "Your fan is loud because of two things.\n\n- `rust-analyzer` is busy\n- Firefox has **40 tabs**\n\n1. Close tabs\n2. Wait\n\n```sh\npkill rust-analyzer\n  echo done\n```\nThat is all.";
        assert_eq!(
            blocks(text, MONO),
            vec![
                para("Your fan is loud because of two things."),
                Block::Item {
                    marker: "•".into(),
                    markup: "<span font_family=\"Geist Mono\">rust-analyzer</span> is busy".into()
                },
                Block::Item {
                    marker: "•".into(),
                    markup: "Firefox has <b>40 tabs</b>".into()
                },
                Block::Item {
                    marker: "1.".into(),
                    markup: "Close tabs".into()
                },
                Block::Item {
                    marker: "2.".into(),
                    markup: "Wait".into()
                },
                Block::Code {
                    language: "sh".into(),
                    text: "pkill rust-analyzer\n  echo done".into()
                },
                para("That is all."),
            ]
        );
    }

    #[test]
    fn lines_in_a_paragraph_keep_their_breaks() {
        assert_eq!(
            blocks("one\ntwo\n\nthree", MONO),
            vec![para("one\ntwo"), para("three")]
        );
    }

    #[test]
    fn headings_become_bold_lines() {
        assert_eq!(
            blocks("## Why\nBecause.", MONO),
            vec![para("<b>Why</b>"), para("Because.")]
        );
    }

    #[test]
    fn code_is_not_escaped_or_parsed() {
        let text = "```\nif a < b && **c** { `x` }\n```";
        assert_eq!(
            blocks(text, MONO),
            vec![Block::Code {
                language: String::new(),
                text: "if a < b && **c** { `x` }".into()
            }]
        );
    }

    #[test]
    fn a_code_block_still_streaming_shows_what_there_is() {
        assert_eq!(
            blocks("Try:\n```sh\nls -la", MONO),
            vec![
                para("Try:"),
                Block::Code {
                    language: "sh".into(),
                    text: "ls -la".into()
                }
            ]
        );
    }

    #[test]
    fn numbers_that_are_not_lists_stay_in_the_paragraph() {
        assert_eq!(
            blocks("2024. That was the year.", MONO),
            vec![para("2024. That was the year.")]
        );
        assert_eq!(blocks("3.5 GB free", MONO), vec![para("3.5 GB free")]);
    }
}

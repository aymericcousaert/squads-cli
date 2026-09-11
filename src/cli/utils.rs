use markdown;

pub fn truncate(s: &str, max_len: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > max_len {
        let truncated: String = chars[..max_len.saturating_sub(3)].iter().collect();
        format!("{}...", truncated)
    } else {
        s.to_string()
    }
}

pub fn strip_html(s: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;

    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            '\n' | '\r' => {
                if !in_tag {
                    result.push(' ');
                }
            }
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }

    result
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("\\!", "!")
        .replace("\\?", "?")
        .replace("\\.", ".")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub fn format_size(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = KB * 1024;
    const GB: i64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub fn markdown_to_html(content: &str) -> String {
    markdown::to_html_with_options(
        content,
        &markdown::Options {
            parse: markdown::ParseOptions {
                constructs: markdown::Constructs {
                    gfm_table: true,
                    ..markdown::Constructs::gfm()
                },
                ..markdown::ParseOptions::gfm()
            },
            ..markdown::Options::gfm()
        },
    )
    .unwrap_or_else(|_| content.to_string())
}

/// Turning Teams message HTML into readable text.
///
/// Only the TUI uses this today; the CLI still calls `strip_html` directly, so
/// the group is allowed to be dead in a build without the TUI.
#[cfg_attr(not(feature = "tui"), allow(dead_code))]
pub mod message {
    use super::strip_html;

    /// Message text with the inline images Teams sends made readable.
    ///
    /// Emoji and pictures both arrive as `<img>` tags, which `strip_html` drops
    /// silently: "Thanks 😊" came out as "Thanks", and a GIF-only message came
    /// out empty. Emoji become their character, pictures become their alt text.
    /// Pictures are removed rather than labelled: the caller draws them, and
    /// labelling them here too showed every one twice, once as text and once
    /// as the picture itself.
    pub fn text(html: &str) -> String {
        strip_html(&rewrite_images(html, |key| {
            crate::api::emoji::get_emoji_by_key(key).map(str::to_string)
        }))
    }

    /// Pictures a message carries, as `(url, alt)`, in the order they appear.
    ///
    /// Emoji are skipped: they are `<img>` tags too, but `text` turns them into
    /// characters, so treating them as pictures would try to download a 20px
    /// PNG for every smiley.
    pub fn pictures(html: &str) -> Vec<(String, String)> {
        let mut found = Vec::new();
        let mut rest = html;

        while let Some(start) = find_img_tag(rest) {
            let tail = &rest[start..];
            let Some(end) = tail.find('>') else { break };
            let tag = &tail[..end];

            let is_emoji = attribute(tag, "itemtype").is_some_and(|t| t.ends_with("/Emoji"));
            if !is_emoji {
                if let Some(url) = attribute(tag, "src") {
                    let alt = attribute(tag, "alt").unwrap_or_default();
                    found.push((url, alt));
                }
            }
            rest = &tail[end + 1..];
        }

        found
    }

    /// Alt text worth putting on screen.
    ///
    /// Teams and Giphy often send the literal word "image", or repeat the file
    /// type, which tells the reader nothing. Better to say what the app is
    /// doing than to echo a useless label.
    pub fn useful_alt(alt: &str) -> Option<&str> {
        const USELESS: [&str; 6] = ["image", "picture", "photo", "gif", "img", "graphic"];
        let trimmed = alt.trim().trim_matches(|c: char| c == '.' || c == ':');
        if trimmed.is_empty() || USELESS.iter().any(|w| trimmed.eq_ignore_ascii_case(w)) {
            return None;
        }
        Some(trimmed)
    }

    /// Swap every `<img>` for plain text, asking `emoji` to resolve Teams emoji ids.
    fn rewrite_images(html: &str, emoji: impl Fn(&str) -> Option<String>) -> String {
        let mut out = String::with_capacity(html.len());
        let mut rest = html;

        while let Some(start) = find_img_tag(rest) {
            out.push_str(&rest[..start]);
            let tail = &rest[start..];
            let Some(end) = tail.find('>') else {
                // Unclosed tag: leave it to strip_html rather than guess.
                out.push_str(tail);
                return out;
            };
            out.push_str(&image_text(&tail[..end], &emoji));
            rest = &tail[end + 1..];
        }

        out.push_str(rest);
        out
    }

    fn find_img_tag(html: &str) -> Option<usize> {
        // Compare bytes, never `&str` slices: `<` can be followed by any text,
        // and slicing to a fixed byte length splits a multi-byte character.
        let bytes = html.as_bytes();
        let name_ends = |c: u8| c.is_ascii_whitespace() || c == b'>' || c == b'/';
        let mut from = 0;
        while let Some(offset) = html[from..].find('<') {
            let at = from + offset;
            let named_img = bytes
                .get(at + 1..at + 4)
                .is_some_and(|name| name.eq_ignore_ascii_case(b"img"));
            if named_img && bytes.get(at + 4).copied().is_none_or(name_ends) {
                return Some(at);
            }
            from = at + 1;
        }
        None
    }

    /// An emoji becomes its character; any other picture becomes nothing.
    fn image_text(tag: &str, emoji: &impl Fn(&str) -> Option<String>) -> String {
        let is_emoji = attribute(tag, "itemtype").is_some_and(|t| t.ends_with("/Emoji"));
        if is_emoji {
            if let Some(id) = attribute(tag, "itemid") {
                // An id the mapping does not know still reads better than nothing.
                return emoji(&id.to_lowercase()).unwrap_or_else(|| format!(":{}:", id));
            }
        }
        String::new()
    }

    /// The value of one double-quoted attribute, matched only at a name boundary so
    /// `alt` does not also match `data-alt`.
    fn attribute(tag: &str, name: &str) -> Option<String> {
        let needle = format!("{}=\"", name);
        let mut from = 0;
        while let Some(offset) = tag[from..].find(&needle) {
            let at = from + offset;
            let preceded_by_space = tag[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
            if preceded_by_space {
                let value = &tag[at + needle.len()..];
                return value.find('"').map(|end| value[..end].to_string());
            }
            from = at + needle.len();
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn stub(key: &str) -> Option<String> {
            (key == "smileeyes").then(|| "😊".to_string())
        }

        #[test]
        fn emoji_image_becomes_its_character() {
            let html = r#"<p>Thanks <span title="Smiling eyes"><img itemscope="" itemtype="http://schema.skype.com/Emoji" itemid="smileeyes" src="https://statics.teams.cdn.office.net/x.png"></span></p>"#;
            assert_eq!(strip_html(&rewrite_images(html, stub)), "Thanks 😊");
        }

        #[test]
        fn unknown_emoji_falls_back_to_its_name() {
            let html = r#"<img itemtype="http://schema.skype.com/Emoji" itemid="nosuchthing">"#;
            assert_eq!(rewrite_images(html, stub), ":nosuchthing:");
        }

        #[test]
        fn attribute_needs_a_name_boundary() {
            let tag = r#"img data-alt="wrong" alt="right""#;
            assert_eq!(attribute(tag, "alt").as_deref(), Some("right"));
        }

        /// The panic this replaced: reading the tag name by slicing `&str` to
        /// byte 3 split whatever multi-byte character sat across it. Seen in
        /// the wild with 'ç' (2 bytes) and '•' (3 bytes), so this walks every
        /// offset for widths 2, 3 and 4 rather than pinning those two.
        #[test]
        fn multibyte_after_a_bare_angle_bracket_never_panics() {
            for wide in ["ç", "•", "😊", "é", "→"] {
                for pad in ["", "a", "ab", "abc", "3 ", " "] {
                    for tail in ["", ">", " x", "img>"] {
                        let html = format!("before <{}{}{} after", pad, wide, tail);
                        let out = rewrite_images(&html, stub);
                        assert!(out.contains(wide), "lost {} in {:?}", wide, html);
                    }
                }
            }
        }

        #[test]
        fn truncated_tags_never_panic() {
            for html in ["<", "<i", "<im", "trailing <", "<img", "a <imgç"] {
                let _ = rewrite_images(html, stub);
            }
        }

        #[test]
        fn a_bare_angle_bracket_is_left_alone() {
            assert_eq!(rewrite_images("si < ça marche", stub), "si < ça marche");
        }

        #[test]
        fn a_tag_starting_with_img_is_not_an_image() {
            assert_eq!(rewrite_images("<imgx>", stub), "<imgx>");
        }

        #[test]
        fn pictures_are_listed_with_their_alt() {
            let html = r#"<p>voici</p><img src="https://x/a.gif" alt="un chat"><img src="https://x/b.png">"#;
            assert_eq!(
                pictures(html),
                vec![
                    ("https://x/a.gif".to_string(), "un chat".to_string()),
                    ("https://x/b.png".to_string(), String::new()),
                ]
            );
        }

        #[test]
        fn emoji_are_not_pictures() {
            let html = r#"<img itemtype="http://schema.skype.com/Emoji" itemid="smileeyes" src="https://statics/20_f.png">"#;
            assert!(pictures(html).is_empty());
        }

        #[test]
        fn pictures_survive_multibyte_text() {
            let html = r#"2 <3 ça <img src="https://x/a.gif" alt="ça va">"#;
            assert_eq!(
                pictures(html),
                vec![("https://x/a.gif".to_string(), "ça va".to_string())]
            );
        }

        /// The bug this fixes: the body said "[image: image]" and the picture
        /// was drawn underneath it, so every picture appeared twice.
        #[test]
        fn the_drawing_caller_gets_no_label() {
            let html = r#"<p>voila</p><img src="https://x/a.gif" alt="image">"#;
            assert_eq!(rewrite_images(html, stub).trim(), "<p>voila</p>");
            assert_eq!(strip_html(&rewrite_images(html, stub)).trim(), "voila");
        }

        #[test]
        fn emoji_survive_even_when_pictures_are_dropped() {
            let html = r#"<p>hi <img itemtype="http://schema.skype.com/Emoji" itemid="smileeyes"><img src="x.gif" alt="cat"></p>"#;
            assert_eq!(strip_html(&rewrite_images(html, stub)).trim(), "hi 😊");
        }

        #[test]
        fn useless_alt_text_is_dropped() {
            for alt in ["", "  ", "image", "Image", "IMAGE.", "gif", "photo", "img"] {
                assert_eq!(useful_alt(alt), None, "should have dropped {:?}", alt);
            }
        }

        #[test]
        fn real_alt_text_is_kept() {
            assert_eq!(
                useful_alt("  Fun Wow GIF by CANAL+  "),
                Some("Fun Wow GIF by CANAL+")
            );
            assert_eq!(useful_alt("image de la sonde"), Some("image de la sonde"));
        }

        #[test]
        fn text_without_images_is_untouched() {
            let html = "<p>bonjour</p>";
            assert_eq!(rewrite_images(html, stub), html);
        }
    }
}

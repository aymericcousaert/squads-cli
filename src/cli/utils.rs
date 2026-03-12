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

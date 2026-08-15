//! Terminal-safe Markdown presentation for provider-authored messages.
//! Command output and diagnostics never pass through this module.

pub fn render(input: &str) -> String {
    let mut output = String::new();
    let mut fenced = false;
    for raw in input.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            output.push_str("  ");
            output.push_str(line);
        } else if let Some(heading) = trimmed
            .strip_prefix("### ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("# "))
        {
            output.push_str(&inline(heading.trim()));
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            output.push_str("• ");
            output.push_str(&inline(item));
        } else if let Some(quote) = trimmed.strip_prefix("> ") {
            output.push_str("│ ");
            output.push_str(&inline(quote));
        } else {
            output.push_str(&inline(line));
        }
        output.push('\n');
    }
    output
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect()
}

fn inline(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;
    while let Some(open) = remaining.find('[') {
        output.push_str(&remaining[..open]);
        let Some(close) = remaining[open + 1..].find(']') else {
            output.push_str(&remaining[open..]);
            remaining = "";
            break;
        };
        let close = open + 1 + close;
        if remaining[close + 1..].starts_with('(') {
            if let Some(end) = remaining[close + 2..].find(')') {
                output.push_str(&remaining[open + 1..close]);
                remaining = &remaining[close + 3 + end..];
                continue;
            }
        }
        output.push_str(&remaining[open..=close]);
        remaining = &remaining[close + 1..];
    }
    output.push_str(remaining);
    output.replace("**", "").replace("__", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::render;

    #[test]
    fn renders_commonmark_without_provider_markup_or_controls() {
        let output = render(
            "## Heading\n\n- **item**\n\n```text\ncode\n```\n\n[link](https://example.com)\n\u{1b}[31m",
        );
        assert!(output.contains("Heading"));
        assert!(output.contains("• item"));
        assert!(output.contains("  code"));
        assert!(output.contains("link"));
        assert!(!output.contains("##"));
        assert!(!output.contains("**"));
        assert!(!output.contains('\u{1b}'));
    }
}

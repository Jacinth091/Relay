//! Control sequence and ANSI escape sequence sanitizer for provider output.
//!
//! Provider output rendered by Relay must sanitize control bytes to prevent
//! raw terminal takeover, screen switching, cursor manipulation, or OSC attacks.

/// Strips ANSI escape sequences, OSC sequences, alternate screen switches,
/// cursor controls, and raw control bytes from a string.
pub fn sanitize_terminal_output(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Escape sequence handling
            if let Some(&next_ch) = chars.peek() {
                match next_ch {
                    '[' => {
                        // Control Sequence Introducer (CSI)
                        chars.next(); // consume '['
                        while let Some(&seq_ch) = chars.peek() {
                            chars.next();
                            // CSI parameters are bytes in range 0x20..=0x3F, final byte is 0x40..=0x7E
                            if (0x40..=0x7E).contains(&(seq_ch as u8)) {
                                break;
                            }
                        }
                    }
                    ']' => {
                        // Operating System Command (OSC)
                        chars.next(); // consume ']'
                        while let Some(&seq_ch) = chars.peek() {
                            chars.next();
                            if seq_ch == '\x07' || seq_ch == '\x1b' {
                                break;
                            }
                        }
                    }
                    '(' | ')' | '*' | '+' => {
                        // Character set designation
                        chars.next();
                        chars.next();
                    }
                    _ => {
                        chars.next();
                    }
                }
            }
        } else if ch == '\r' {
            // Preserve newline / carriage return safely if followed by \n
            if chars.peek() == Some(&'\n') {
                result.push('\n');
                chars.next();
            } else {
                result.push('\n');
            }
        } else if ch == '\n' || ch == '\t' {
            result.push(ch);
        } else if ch.is_control() {
            // Strip other ASCII control characters (0x00-0x1F, 0x7F)
            continue;
        } else {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_color_and_style_codes() {
        let input = "\x1b[31mRed Text\x1b[0m \x1b[1mBold\x1b[0m";
        assert_eq!(sanitize_terminal_output(input), "Red Text Bold");
    }

    #[test]
    fn strips_alternate_screen_and_cursor_commands() {
        let input = "\x1b[?1049hHello\x1b[?1049l World\x1b[2J\x1b[H";
        assert_eq!(sanitize_terminal_output(input), "Hello World");
    }

    #[test]
    fn strips_osc_title_commands() {
        let input = "\x1b]0;Title\x07Body text";
        assert_eq!(sanitize_terminal_output(input), "Body text");
    }

    #[test]
    fn preserves_newlines_and_tabs() {
        let input = "Line 1\r\nLine 2\tTabbed";
        assert_eq!(sanitize_terminal_output(input), "Line 1\nLine 2\tTabbed");
    }
}

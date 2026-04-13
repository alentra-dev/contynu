/// Truncate `s` to at most `max_bytes` bytes, snapping back to the nearest
/// preceding char boundary if `max_bytes` falls inside a multi-byte UTF-8
/// scalar. Always safe — never panics on byte indexing.
pub fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_under_limit_returns_full() {
        assert_eq!(truncate_at_char_boundary("hello", 10), "hello");
    }

    #[test]
    fn ascii_over_limit_truncates_to_exact_bytes() {
        assert_eq!(truncate_at_char_boundary("hello world", 5), "hello");
    }

    #[test]
    fn does_not_panic_on_multibyte_boundary() {
        // '─' (U+2500) is 3 bytes in UTF-8. Cutting at byte 1 of 3 must not panic.
        let s = "abc─def";
        // bytes: a(1) b(1) c(1) ─(3) d(1) e(1) f(1) = 9 bytes
        // Asking for 4 bytes lands inside '─' (which occupies bytes 3..6).
        let out = truncate_at_char_boundary(s, 4);
        assert_eq!(out, "abc");
    }

    #[test]
    fn snaps_back_to_char_boundary() {
        let s = "a─b";
        // Bytes: a(0) ─(1..4) b(4)
        assert_eq!(truncate_at_char_boundary(s, 2), "a");
        assert_eq!(truncate_at_char_boundary(s, 3), "a");
        assert_eq!(truncate_at_char_boundary(s, 4), "a─");
    }

    #[test]
    fn empty_string_is_safe() {
        assert_eq!(truncate_at_char_boundary("", 10), "");
        assert_eq!(truncate_at_char_boundary("", 0), "");
    }

    #[test]
    fn zero_max_bytes_returns_empty() {
        assert_eq!(truncate_at_char_boundary("hello", 0), "");
    }

    #[test]
    fn box_drawing_at_byte_200_does_not_panic() {
        // Reproduces the exact MCP server crash: a string where byte 200 lands
        // inside a U+2500 box-drawing char, the same shape Codex emits.
        let s = "─ Worked for 1m 38s ".to_string()
            + &"─".repeat(100); // many em-dashes
        assert!(s.len() > 200);
        let out = truncate_at_char_boundary(&s, 200);
        assert!(out.len() <= 200);
        assert!(s.starts_with(out));
    }
}

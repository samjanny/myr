pub fn parse_id(input: &str) -> Option<u32> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    trimmed.parse().ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_plain_identifiers() {
        assert_eq!(super::parse_id("42"), Some(42));
        assert_eq!(super::parse_id("x"), None);
    }
}

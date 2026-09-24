pub fn parse_id(input: &str) -> Option<u32> {
    let mut value: u32 = 0;
    let mut digits = 0usize;
    for byte in input.trim().bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u32::from(byte - b'0'))?;
        digits += 1;
    }
    (digits > 0).then_some(value)
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_plain_identifiers() {
        assert_eq!(super::parse_id("42"), Some(42));
        assert_eq!(super::parse_id("x"), None);
    }
}

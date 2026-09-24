// Only ASCII letters are case-folded.
pub fn normalize(name: &str) -> String {
    name.chars().map(|c| c.to_ascii_lowercase()).collect()
}

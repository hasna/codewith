pub(crate) fn truncate_to_byte_limit(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

#[cfg(test)]
mod tests {
    use super::truncate_to_byte_limit;
    use pretty_assertions::assert_eq;

    #[test]
    fn truncates_at_a_utf8_character_boundary() {
        assert_eq!(truncate_to_byte_limit("aaaaéz", /*max_bytes*/ 5), "aaaa");
    }
}

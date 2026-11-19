pub fn initial(username: &str) -> String {
    username
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

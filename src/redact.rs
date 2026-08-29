pub fn redact_phone_number(phone: &str) -> String {
    let visible_start = phone.chars().take(2).collect::<String>();
    let visible_end = phone
        .chars()
        .rev()
        .take(2)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{}****{}", visible_start, visible_end)
}

pub fn redact_path(path: &str) -> String {
    if path.is_empty() {
        return "<empty>".to_string();
    }
    let file_name = std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("<redacted>");
    format!("<redacted>/{file_name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_redaction_keeps_edges() {
        assert_eq!(redact_phone_number("+15551234567"), "+1****67");
    }

    #[test]
    fn path_redaction_keeps_file_name() {
        assert_eq!(
            redact_path("/var/lib/telegram-s3/session"),
            "<redacted>/session"
        );
    }
}

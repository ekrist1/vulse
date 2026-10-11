pub fn check_email(email: &str) -> Result<(), String> {
    if email.len() < 3 || email.len() > 254 {
        return Err("email must be between 3 and 254 characters".into());
    }
    let Some((local, domain)) = email.split_once('@') else {
        return Err("email must contain @".into());
    };
    if local.is_empty() || domain.is_empty() || domain.contains('@') || !domain.contains('.') {
        return Err("email is invalid".into());
    }
    if !email
        .chars()
        .all(|c| c.is_ascii() && !c.is_whitespace() && c != '<' && c != '>')
    {
        return Err("email is invalid".into());
    }
    Ok(())
}

pub fn check_handle(handle: &str) -> Result<(), String> {
    if is_slug(handle) && (2..=32).contains(&handle.len()) {
        Ok(())
    } else {
        Err("handle must be 2-32 characters of lowercase letters, numbers, or hyphens".into())
    }
}

pub fn check_collection_name(name: &str) -> Result<(), String> {
    if is_slug(name) && (1..=64).contains(&name.len()) {
        Ok(())
    } else {
        Err(
            "collection name must be 1-64 characters of lowercase letters, numbers, or hyphens"
                .into(),
        )
    }
}

pub fn check_password(password: &str) -> Result<(), String> {
    if (8..=128).contains(&password.len()) && password.chars().any(|c| !c.is_whitespace()) {
        Ok(())
    } else {
        Err("password must be 8-128 characters".into())
    }
}

pub fn check_key_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if (1..=80).contains(&name.len()) && name.chars().all(|c| !c.is_control()) {
        Ok(())
    } else {
        Err("key name must be 1-80 characters".into())
    }
}

pub fn page_bounds(limit: Option<u32>, offset: Option<u32>) -> Result<(u32, u32), String> {
    let limit = limit.unwrap_or(50);
    let offset = offset.unwrap_or(0);
    if !(1..=100).contains(&limit) {
        return Err("limit must be between 1 and 100".into());
    }
    Ok((limit, offset))
}

/// A real calendar date written exactly as `YYYY-MM-DD`, years 0001–9999.
pub fn is_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return false;
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok() && &value[..4] != "0000"
}

pub fn is_uuid(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            if *byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

pub fn is_field_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    (1..=64).contains(&name.len())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn is_slug(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::is_date;

    #[test]
    fn dates_must_be_real_iso_calendar_days() {
        for good in ["2026-10-11", "2024-02-29", "0001-01-01", "9999-12-31"] {
            assert!(is_date(good), "{good}");
        }
        for bad in [
            "2023-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-1-5",
            "26-10-11",
            "2026/10/11",
            "2026-10-11T00:00",
            "0000-01-01",
            "+2026-10-1",
            "",
        ] {
            assert!(!is_date(bad), "{bad}");
        }
    }

    use super::*;

    #[test]
    fn rejects_short_passwords_and_bad_handles() {
        assert!(check_password("short").is_err());
        assert!(check_password("long-enough").is_ok());
        assert!(check_handle("A").is_err());
        assert!(check_handle("ada").is_ok());
        assert!(check_email("ada@example.com").is_ok());
        assert!(check_email("ada").is_err());
    }

    #[test]
    fn uuid_shape() {
        assert!(is_uuid("550e8400-e29b-41d4-a716-446655440000"));
        assert!(!is_uuid("../etc/passwd"));
        assert!(!is_uuid("550e8400e29b41d4a716446655440000"));
    }
}

//! `Set-Cookie` parsing (RFC 6265bis-15 §5.6) and the cookie-date
//! algorithm (§5.1.1).

/// Longest name plus value (§5.6 step 5, §6.1).
pub const MAX_NAME_VALUE_BYTES: usize = 4096;
/// Longest attribute value (§5.6.1 step 7).
pub const MAX_ATTRIBUTE_VALUE_BYTES: usize = 1024;
/// Cap on any expiry, in seconds after the time the cookie was received
/// (§5.6.1 and §5.6.2: 400 days).
pub const MAX_AGE_SECONDS: i64 = 400 * 24 * 60 * 60;

/// The `SameSite` attribute (§5.6.7); `Default` when absent or unknown.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum SameSite {
    /// Never sent on cross-site requests.
    Strict,
    /// Sent on cross-site top-level navigations with a safe method.
    Lax,
    /// Sent on every request (requires `Secure`).
    None,
    /// No valid attribute; Rustic enforces it as `Lax` (Lax-by-default).
    #[default]
    Default,
}

/// One parsed `Set-Cookie` header value before the storage model runs.
///
/// Attribute values are already reduced to the last occurrence of each
/// attribute, which is what the storage model reads (§5.7 steps 6–17).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedCookie {
    /// The cookie name (may be empty).
    pub name: String,
    /// The cookie value.
    pub value: String,
    /// The last valid `Expires`, as Unix seconds.
    pub expires: Option<i64>,
    /// The last valid `Max-Age`, in seconds (may be zero or negative).
    pub max_age: Option<i64>,
    /// The last non-empty `Domain`, lowercase, leading dot removed.
    pub domain: Option<String>,
    /// The last `Path` that starts with `/`; `None` means default-path.
    pub path: Option<String>,
    /// `Secure` was present.
    pub secure: bool,
    /// `HttpOnly` was present.
    pub http_only: bool,
    /// The last `SameSite`.
    pub same_site: SameSite,
    /// `Partitioned` was present (CHIPS).
    pub partitioned: bool,
}

/// %x00-08 / %x0A-1F / %x7F: control characters other than tab.
fn has_forbidden_control(text: &str) -> bool {
    text.bytes()
        .any(|byte| matches!(byte, 0x00..=0x08 | 0x0A..=0x1F | 0x7F))
}

fn trim_wsp(text: &str) -> &str {
    text.trim_matches([' ', '\t'])
}

/// Parse one `Set-Cookie` field value (isomorphic-decoded). `None` means
/// the user agent ignores the header (§5.6 steps 1–6).
pub fn parse_set_cookie(input: &str) -> Option<ParsedCookie> {
    if has_forbidden_control(input) {
        return None;
    }
    let (pair, mut attributes) = match input.find(';') {
        Some(index) => (&input[..index], &input[index..]),
        None => (input, ""),
    };
    let (name, value) = match pair.find('=') {
        Some(index) => (trim_wsp(&pair[..index]), trim_wsp(&pair[index + 1..])),
        None => ("", trim_wsp(pair)),
    };
    if name.len() + value.len() > MAX_NAME_VALUE_BYTES || (name.is_empty() && value.is_empty()) {
        return None;
    }
    let mut cookie = ParsedCookie {
        name: name.to_owned(),
        value: value.to_owned(),
        ..ParsedCookie::default()
    };
    while let Some(rest) = attributes.strip_prefix(';') {
        let end = rest.find(';').unwrap_or(rest.len());
        let attribute = &rest[..end];
        attributes = &rest[end..];
        let (name, value) = match attribute.find('=') {
            Some(index) => (
                trim_wsp(&attribute[..index]),
                trim_wsp(&attribute[index + 1..]),
            ),
            None => (trim_wsp(attribute), ""),
        };
        if value.len() > MAX_ATTRIBUTE_VALUE_BYTES {
            continue;
        }
        apply_attribute(&mut cookie, name, value);
    }
    Some(cookie)
}

fn apply_attribute(cookie: &mut ParsedCookie, name: &str, value: &str) {
    let is = |expected: &str| name.eq_ignore_ascii_case(expected);
    if is("expires") {
        if let Some(time) = parse_cookie_date(value) {
            cookie.expires = Some(time);
        }
    } else if is("max-age") {
        let digits = value.strip_prefix('-').unwrap_or(value);
        if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
            // Saturate: any huge value is capped at 400 days later anyway.
            let magnitude = digits.parse::<i64>().unwrap_or(i64::MAX);
            cookie.max_age = Some(if value.starts_with('-') {
                -magnitude
            } else {
                magnitude
            });
        }
    } else if is("domain") {
        if !value.is_empty() {
            let domain = value.strip_prefix('.').unwrap_or(value);
            cookie.domain = Some(domain.to_ascii_lowercase());
        }
    } else if is("path") {
        cookie.path = value.starts_with('/').then(|| value.to_owned());
    } else if is("secure") {
        cookie.secure = true;
    } else if is("httponly") {
        cookie.http_only = true;
    } else if is("samesite") {
        cookie.same_site = if value.eq_ignore_ascii_case("strict") {
            SameSite::Strict
        } else if value.eq_ignore_ascii_case("lax") {
            SameSite::Lax
        } else if value.eq_ignore_ascii_case("none") {
            SameSite::None
        } else {
            SameSite::Default
        };
    } else if is("partitioned") {
        cookie.partitioned = true;
    }
}

fn is_delimiter(byte: u8) -> bool {
    matches!(byte, 0x09 | 0x20..=0x2F | 0x3B..=0x40 | 0x5B..=0x60 | 0x7B..=0x7E)
}

/// Leading 1–`max` digits followed by nothing or a non-digit; the value
/// and the digit count.
fn leading_digits(token: &[u8], min: usize, max: usize) -> Option<(u32, usize)> {
    let count = token
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if count < min || count > max {
        return None;
    }
    let mut value = 0u32;
    for &digit in &token[..count] {
        value = value * 10 + u32::from(digit - b'0');
    }
    Some((value, count))
}

fn parse_time(token: &[u8]) -> Option<(u32, u32, u32)> {
    let (hour, used) = leading_digits(token, 1, 2)?;
    let rest = token[used..].strip_prefix(b":")?;
    let (minute, used) = leading_digits(rest, 1, 2)?;
    let rest = rest[used..].strip_prefix(b":")?;
    let (second, _) = leading_digits(rest, 1, 2)?;
    Some((hour, minute, second))
}

const MONTHS: [&[u8; 3]; 12] = [
    b"jan", b"feb", b"mar", b"apr", b"may", b"jun", b"jul", b"aug", b"sep", b"oct", b"nov", b"dec",
];

/// RFC 6265bis §5.1.1: parse a cookie date into Unix seconds.
pub fn parse_cookie_date(input: &str) -> Option<i64> {
    let bytes = input.as_bytes();
    let mut time = None;
    let mut day = None;
    let mut month = None;
    let mut year = None;
    for token in bytes.split(|&byte| is_delimiter(byte)) {
        if token.is_empty() {
            continue;
        }
        if time.is_none() {
            if let Some(found) = parse_time(token) {
                time = Some(found);
                continue;
            }
        }
        if day.is_none() {
            if let Some((value, _)) = leading_digits(token, 1, 2) {
                day = Some(value);
                continue;
            }
        }
        if month.is_none() && token.len() >= 3 {
            let prefix = token[..3].to_ascii_lowercase();
            if let Some(index) = MONTHS.iter().position(|name| name[..] == prefix[..]) {
                month = Some(u32::try_from(index).expect("12 months") + 1);
                continue;
            }
        }
        if year.is_none() {
            if let Some((value, _)) = leading_digits(token, 2, 4) {
                year = Some(value);
            }
        }
    }
    let (hour, minute, second) = time?;
    let (day, month, mut year) = (day?, month?, year?);
    if (70..=99).contains(&year) {
        year += 1900;
    } else if year <= 69 {
        year += 2000;
    }
    if !(1..=31).contains(&day) || year < 1601 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    if day > days_in_month(year, month) {
        return None;
    }
    let days = days_from_civil(i64::from(year), month, day);
    Some(days * 86_400 + i64::from(hour * 3600 + minute * 60 + second))
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar (Howard
/// Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_value_pairs() {
        let cookie = parse_set_cookie(" a = b c ; ").unwrap();
        assert_eq!((cookie.name.as_str(), cookie.value.as_str()), ("a", "b c"));
        let nameless = parse_set_cookie("token").unwrap();
        assert_eq!(
            (nameless.name.as_str(), nameless.value.as_str()),
            ("", "token")
        );
        let empty_value = parse_set_cookie("a=").unwrap();
        assert_eq!(empty_value.value, "");
        assert_eq!(parse_set_cookie("=b").unwrap().name, "");
        assert_eq!(parse_set_cookie("a=b=c").unwrap().value, "b=c");
        assert!(parse_set_cookie("=").is_none());
        assert!(parse_set_cookie("; Secure").is_none());
        assert!(parse_set_cookie("a=b\x01").is_none());
        assert!(parse_set_cookie("a=b\x7f").is_none());
        assert!(parse_set_cookie("a=\tb").is_some());
        let long = "v".repeat(MAX_NAME_VALUE_BYTES - 1);
        assert!(parse_set_cookie(&format!("a={long}")).is_some());
        assert!(parse_set_cookie(&format!("ab={long}")).is_none());
    }

    #[test]
    fn attributes_last_wins_and_invalid_ones_are_ignored() {
        let cookie = parse_set_cookie(
            "a=b; Path=/x; path=relative; DOMAIN=.Example.COM; domain=; secure; HttpOnly=yes; \
             SameSite=lax; samesite=bogus; Max-Age=10; max-age=1x; Partitioned; unknown=1",
        )
        .unwrap();
        assert_eq!(cookie.path, None);
        assert_eq!(cookie.domain.as_deref(), Some("example.com"));
        assert!(cookie.secure && cookie.http_only && cookie.partitioned);
        assert_eq!(cookie.same_site, SameSite::Default);
        assert_eq!(cookie.max_age, Some(10));
        let cookie = parse_set_cookie("a=b; Max-Age=-5; SameSite=None; path=/p").unwrap();
        assert_eq!(cookie.max_age, Some(-5));
        assert_eq!(cookie.same_site, SameSite::None);
        assert_eq!(cookie.path.as_deref(), Some("/p"));
        let long = format!("a=b; Path=/{}", "p".repeat(MAX_ATTRIBUTE_VALUE_BYTES));
        assert_eq!(parse_set_cookie(&long).unwrap().path, None);
        let huge = parse_set_cookie("a=b; max-age=99999999999999999999999").unwrap();
        assert_eq!(huge.max_age, Some(i64::MAX));
        assert_eq!(
            parse_set_cookie("a=b; expires=Wed, 21 Oct 2015 07:28:00 GMT")
                .unwrap()
                .expires,
            Some(1_445_412_480)
        );
        assert_eq!(
            parse_set_cookie("a=b; expires=never").unwrap().expires,
            None
        );
    }

    #[test]
    fn cookie_dates() {
        for (input, expected) in [
            ("Thu, 01 Jan 1970 00:00:00 GMT", Some(0)),
            ("Wed, 21 Oct 2015 07:28:00 GMT", Some(1_445_412_480)),
            ("Wednesday, 21-Oct-15 07:28:00 GMT", Some(1_445_412_480)),
            ("21 oct 2015 7:28:0", Some(1_445_412_480)),
            ("Oct 21 07:28:00 2015", Some(1_445_412_480)),
            ("Thu, 10 Apr 1980 16:33:12 GMT", Some(324_232_392)),
            ("Sat, 29 Feb 2020 00:00:00 GMT", Some(1_582_934_400)),
            ("Fri, 29 Feb 2019 00:00:00 GMT", None),
            ("Mon, 31 Apr 2023 00:00:00 GMT", None),
            ("Mon, 01 Jan 1600 00:00:00 GMT", None),
            ("Mon, 01 Jan 1601 00:00:00 GMT", Some(-11_644_473_600)),
            ("Mon, 01 Jan 69 00:00:00 GMT", Some(3_124_224_000)),
            ("Mon, 32 Jan 2000 00:00:00 GMT", None),
            ("Mon, 01 Jan 2000 24:00:00 GMT", None),
            ("Mon, 01 Jan 2000 00:60:00 GMT", None),
            ("Mon, 01 Jan 2000 00:00:60 GMT", None),
            ("Mon, 01 Jan 2000 GMT", None),
            ("01 Jan 2000 00:00:00", Some(946_684_800)),
            ("1 Janx 2000 00:00:00", Some(946_684_800)),
            ("001 Jan 2000 00:00:00", None),
            ("", None),
        ] {
            assert_eq!(parse_cookie_date(input), expected, "{input}");
        }
    }
}

use super::*;

const NOW: i64 = 1_700_000_000;

fn url(input: &str) -> Url {
    Url::parse(input, None).unwrap()
}

fn suffixes() -> PublicSuffixList {
    PublicSuffixList::parse("com\nco.uk\nuk\ntest\n").unwrap()
}

fn jar() -> CookieJar {
    CookieJar::new(suffixes(), CookieLimits::default())
}

fn first_party(url: &Url) -> CookieContext<'_> {
    CookieContext {
        url,
        same_site: true,
        top_level_navigation: false,
        safe_method: true,
        http_api: true,
        top_level_site: None,
    }
}

fn set(jar: &mut CookieJar, at: &str, header: &str) -> Result<(), CookieRejection> {
    let target = url(at);
    jar.set_cookie(header, &first_party(&target), NOW)
}

fn header(jar: &mut CookieJar, at: &str) -> Option<String> {
    let target = url(at);
    jar.cookie_header(&first_party(&target), NOW)
}

#[test]
fn host_only_and_domain_cookies() {
    let mut jar = jar();
    set(&mut jar, "http://www.example.com/", "host=1").unwrap();
    set(
        &mut jar,
        "http://www.example.com/",
        "dom=2; Domain=.EXAMPLE.com",
    )
    .unwrap();
    assert_eq!(
        header(&mut jar, "http://www.example.com/").as_deref(),
        Some("host=1; dom=2")
    );
    assert_eq!(
        header(&mut jar, "http://a.example.com/").as_deref(),
        Some("dom=2")
    );
    assert_eq!(
        header(&mut jar, "http://example.com/").as_deref(),
        Some("dom=2")
    );
    assert_eq!(header(&mut jar, "http://badexample.com/"), None);
    assert!(jar.cookies()[0].host_only() && !jar.cookies()[1].host_only());
    assert_eq!(
        set(&mut jar, "http://www.example.com/", "x=1; Domain=other.com"),
        Err(CookieRejection::DomainMismatch)
    );
    assert_eq!(
        set(
            &mut jar,
            "http://www.example.com/",
            "x=1; Domain=a.www.example.com"
        ),
        Err(CookieRejection::DomainMismatch)
    );
    // A public suffix is refused unless it is the host itself, which
    // makes the cookie host-only.
    assert_eq!(
        set(&mut jar, "http://www.example.co.uk/", "x=1; Domain=co.uk"),
        Err(CookieRejection::DomainMismatch)
    );
    set(&mut jar, "http://co.uk/", "x=1; Domain=co.uk").unwrap();
    assert!(jar.cookies().last().unwrap().host_only());
    // IP hosts only domain-match themselves.
    set(&mut jar, "http://127.0.0.1:8080/", "ip=1; Domain=127.0.0.1").unwrap();
    assert_eq!(
        set(&mut jar, "http://127.0.0.1/", "ip=2; Domain=0.0.1"),
        Err(CookieRejection::DomainMismatch)
    );
    assert_eq!(
        header(&mut jar, "http://127.0.0.1:9/").as_deref(),
        Some("ip=1")
    );
}

#[test]
fn paths() {
    assert_eq!(default_path(&url("http://a.test")), "/");
    assert_eq!(default_path(&url("http://a.test/x")), "/");
    assert_eq!(default_path(&url("http://a.test/x/y/z?q")), "/x/y");
    assert_eq!(default_path(&url("http://a.test/x/")), "/x");
    assert!(path_match("/x", "/x"));
    assert!(path_match("/x/y", "/x"));
    assert!(path_match("/x/y", "/x/"));
    assert!(!path_match("/xy", "/x"));
    assert!(!path_match("/", "/x"));
    let mut jar = jar();
    set(&mut jar, "http://a.example.com/docs/page", "d=1").unwrap();
    set(&mut jar, "http://a.example.com/", "root=1; Path=/").unwrap();
    set(&mut jar, "http://a.example.com/", "deep=1; Path=/docs/a").unwrap();
    assert_eq!(jar.cookies()[0].path(), "/docs");
    assert_eq!(
        header(&mut jar, "http://a.example.com/docs/a/b").as_deref(),
        Some("deep=1; d=1; root=1")
    );
    assert_eq!(
        header(&mut jar, "http://a.example.com/doc").as_deref(),
        Some("root=1")
    );
}

#[test]
fn expiry_max_age_and_deletion() {
    let mut jar = jar();
    set(&mut jar, "http://a.example.com/", "s=1").unwrap();
    set(
        &mut jar,
        "http://a.example.com/",
        "e=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT",
    )
    .unwrap();
    assert_eq!(jar.len(), 1, "an expired cookie is not stored");
    set(
        &mut jar,
        "http://a.example.com/",
        "m=1; Max-Age=60; Expires=Wed, 21 Oct 2015 07:28:00 GMT",
    )
    .unwrap();
    set(
        &mut jar,
        "http://a.example.com/",
        "far=1; Max-Age=999999999999",
    )
    .unwrap();
    let expiry = |jar: &CookieJar, name: &str| {
        jar.cookies()
            .iter()
            .find(|cookie| cookie.name() == name)
            .unwrap()
            .expiry()
    };
    assert_eq!(expiry(&jar, "s"), None);
    assert_eq!(expiry(&jar, "m"), Some(NOW + 60));
    assert_eq!(expiry(&jar, "far"), Some(NOW + MAX_AGE_SECONDS));
    let target = url("http://a.example.com/");
    assert_eq!(
        jar.cookie_header(&first_party(&target), NOW + 60)
            .as_deref(),
        Some("s=1; far=1")
    );
    set(&mut jar, "http://a.example.com/", "s=gone; Max-Age=0").unwrap();
    assert!(jar.cookies().iter().all(|cookie| cookie.name() != "s"));
    jar.purge_expired(NOW + 61);
    assert_eq!(jar.len(), 1);
}

#[test]
fn replacement_keeps_creation_order() {
    let mut jar = jar();
    set(&mut jar, "http://a.example.com/", "a=1").unwrap();
    set(&mut jar, "http://a.example.com/", "b=1").unwrap();
    set(&mut jar, "http://a.example.com/", "a=2").unwrap();
    assert_eq!(
        header(&mut jar, "http://a.example.com/").as_deref(),
        Some("a=2; b=1")
    );
    // A different path is a different cookie.
    set(&mut jar, "http://a.example.com/", "a=3; Path=/x").unwrap();
    assert_eq!(jar.len(), 3);
    set(&mut jar, "http://a.example.com/", "novalue").unwrap();
    assert_eq!(
        header(&mut jar, "http://a.example.com/x").as_deref(),
        Some("a=3; a=2; b=1; novalue")
    );
}

#[test]
fn secure_http_only_and_shadowing() {
    let mut jar = jar();
    assert_eq!(
        set(&mut jar, "http://a.example.com/", "s=1; Secure"),
        Err(CookieRejection::InsecureSecure)
    );
    set(&mut jar, "https://a.example.com/", "s=1; Secure").unwrap();
    set(&mut jar, "http://localhost/", "local=1; Secure").unwrap();
    assert_eq!(header(&mut jar, "http://a.example.com/"), None);
    assert_eq!(
        header(&mut jar, "https://a.example.com/").as_deref(),
        Some("s=1")
    );
    assert_eq!(
        header(&mut jar, "http://localhost/").as_deref(),
        Some("local=1")
    );
    assert_eq!(
        set(&mut jar, "http://a.example.com/", "s=2"),
        Err(CookieRejection::ShadowsSecure)
    );
    assert_eq!(
        set(
            &mut jar,
            "http://a.example.com/sub",
            "s=2; Domain=example.com; Path=/sub"
        ),
        Err(CookieRejection::ShadowsSecure)
    );
    set(&mut jar, "https://a.example.com/", "h=1; HttpOnly").unwrap();
    let target = url("https://a.example.com/");
    let script = CookieContext {
        http_api: false,
        ..first_party(&target)
    };
    assert_eq!(jar.cookie_header(&script, NOW).as_deref(), Some("s=1"));
    assert_eq!(
        jar.set_cookie("h=2", &script, NOW),
        Err(CookieRejection::HttpOnly)
    );
    assert_eq!(
        jar.set_cookie("x=1; HttpOnly", &script, NOW),
        Err(CookieRejection::HttpOnly)
    );
    jar.set_cookie("x=1", &script, NOW).unwrap();
    assert_eq!(
        jar.cookie_header(&first_party(&target), NOW).as_deref(),
        Some("s=1; h=1; x=1")
    );
}

#[test]
fn prefixes() {
    let mut jar = jar();
    let secure = "https://a.example.com/x/";
    assert_eq!(
        set(&mut jar, secure, "__Secure-a=1"),
        Err(CookieRejection::Prefix)
    );
    assert_eq!(
        set(&mut jar, secure, "__secure-a=1"),
        Err(CookieRejection::Prefix)
    );
    set(&mut jar, secure, "__Secure-a=1; Secure; Domain=example.com").unwrap();
    assert_eq!(
        set(&mut jar, secure, "__Host-a=1; Secure"),
        Err(CookieRejection::Prefix)
    );
    assert_eq!(
        set(
            &mut jar,
            secure,
            "__Host-a=1; Secure; Path=/; Domain=a.example.com"
        ),
        Err(CookieRejection::Prefix)
    );
    set(&mut jar, secure, "__Host-a=1; Secure; Path=/").unwrap();
    assert_eq!(
        set(&mut jar, secure, "__Host-x"),
        Err(CookieRejection::NamelessPrefix)
    );
    assert_eq!(
        set(&mut jar, secure, "=__SECURE-x"),
        Err(CookieRejection::NamelessPrefix)
    );
    assert_eq!(jar.len(), 2);
}

fn cross_site(target: &Url) -> CookieContext<'_> {
    CookieContext {
        same_site: false,
        ..first_party(target)
    }
}

#[test]
fn same_site_rules() {
    let mut jar = jar();
    jar.set_third_party(ThirdPartyCookies::Allow);
    let target = url("https://a.example.com/");
    for header in [
        "strict=1; SameSite=Strict",
        "lax=1; SameSite=Lax",
        "default=1",
        "none=1; SameSite=None; Secure",
    ] {
        jar.set_cookie(header, &first_party(&target), NOW).unwrap();
    }
    assert_eq!(
        jar.set_cookie("n=1; SameSite=None", &first_party(&target), NOW),
        Err(CookieRejection::NoneWithoutSecure)
    );
    let subresource = cross_site(&target);
    assert_eq!(
        jar.cookie_header(&subresource, NOW).as_deref(),
        Some("none=1")
    );
    let navigation = CookieContext {
        top_level_navigation: true,
        ..cross_site(&target)
    };
    assert_eq!(
        jar.cookie_header(&navigation, NOW).as_deref(),
        Some("lax=1; default=1; none=1")
    );
    let unsafe_navigation = CookieContext {
        safe_method: false,
        ..navigation.clone()
    };
    // Lax-allowing-unsafe: a cookie without SameSite goes with a
    // top-level cross-site POST for two minutes after it was first set,
    // and replacing it does not restart the window.
    assert_eq!(
        jar.cookie_header(&unsafe_navigation, NOW).as_deref(),
        Some("default=1; none=1")
    );
    let late = NOW + super::LAX_ALLOWING_UNSAFE_SECONDS;
    jar.set_cookie("default=2", &first_party(&target), late)
        .unwrap();
    assert_eq!(
        jar.cookie_header(&unsafe_navigation, late).as_deref(),
        Some("default=2; none=1")
    );
    assert_eq!(
        jar.cookie_header(&unsafe_navigation, late + 1).as_deref(),
        Some("none=1")
    );
    // Storing from a cross-site subresource needs SameSite=None; a
    // top-level navigation may store any.
    assert_eq!(
        jar.set_cookie("x=1; SameSite=Lax", &subresource, NOW),
        Err(CookieRejection::CrossSite)
    );
    jar.set_cookie("y=1; SameSite=None; Secure", &subresource, NOW)
        .unwrap();
    jar.set_cookie("z=1; SameSite=Strict", &navigation, NOW)
        .unwrap();
}

#[test]
fn third_party_cookies_are_blocked_unless_partitioned() {
    let mut jar = jar();
    let embed = url("https://widget.example.com/");
    let top = Some(suffixes().site(&url("https://news.co.uk/").origin()));
    let other_top = Some(suffixes().site(&url("https://blog.test/").origin()));
    let context = CookieContext {
        top_level_site: top.clone(),
        ..cross_site(&embed)
    };
    assert_eq!(
        jar.set_cookie("t=1; SameSite=None; Secure", &context, NOW),
        Err(CookieRejection::ThirdParty)
    );
    jar.set_cookie("p=1; SameSite=None; Secure; Partitioned", &context, NOW)
        .unwrap();
    assert_eq!(
        jar.set_cookie("q=1; SameSite=None; Partitioned", &context, NOW),
        Err(CookieRejection::NoneWithoutSecure)
    );
    assert_eq!(
        jar.set_cookie("q=1; Secure; Partitioned", &first_party(&embed), NOW),
        Err(CookieRejection::Partitioned)
    );
    assert_eq!(jar.cookie_header(&context, NOW).as_deref(), Some("p=1"));
    let elsewhere = CookieContext {
        top_level_site: other_top,
        ..cross_site(&embed)
    };
    assert_eq!(jar.cookie_header(&elsewhere, NOW), None);
    assert_eq!(jar.cookies()[0].partition(), top.as_ref());
}

#[test]
fn eviction_prefers_non_secure_and_least_recently_used() {
    let mut jar = CookieJar::new(
        suffixes(),
        CookieLimits {
            per_site: 3,
            total: 4,
        },
    );
    set(&mut jar, "https://a.example.com/", "secure=1; Secure").unwrap();
    set(&mut jar, "https://a.example.com/", "one=1").unwrap();
    set(&mut jar, "https://b.example.com/", "two=1").unwrap();
    // Touch `one` so `two` is least recently used among non-secure.
    header(&mut jar, "https://a.example.com/");
    set(&mut jar, "https://a.example.com/", "three=1").unwrap();
    let names = |jar: &CookieJar| {
        let mut names: Vec<String> = jar
            .cookies()
            .iter()
            .map(|cookie| cookie.name().to_owned())
            .collect();
        names.sort();
        names
    };
    assert_eq!(names(&jar), ["one", "secure", "three"]);
    set(&mut jar, "https://other.test/", "x=1").unwrap();
    set(&mut jar, "https://another.test/", "y=1").unwrap();
    assert_eq!(jar.len(), 4);
    assert!(jar.cookies().iter().any(|cookie| cookie.name() == "y"));
    jar.clear_site(url("https://deep.another.test/").host().unwrap());
    assert!(jar.cookies().iter().all(|cookie| cookie.name() != "y"));
    jar.clear();
    assert!(jar.is_empty());
}

/// Independent model check: after any sequence of stores, the jar has no
/// two cookies with the same identity, nothing expired, and respects its
/// limits.
#[test]
fn random_sequences_keep_the_jar_invariants() {
    let hosts = [
        "http://a.example.com/x/y",
        "https://b.example.com/",
        "http://example.com/",
        "https://c.test/p",
    ];
    let names = ["a", "b", "__Host-c", "__Secure-d", ""];
    let attributes = [
        "",
        "; Secure",
        "; Domain=example.com",
        "; Path=/",
        "; Path=/x",
        "; Max-Age=0",
        "; Max-Age=5",
        "; SameSite=None; Secure",
        "; HttpOnly",
    ];
    let mut jar = CookieJar::new(
        suffixes(),
        CookieLimits {
            per_site: 5,
            total: 8,
        },
    );
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = |bound: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        usize::try_from(seed % bound as u64).unwrap()
    };
    for step in 0..3000 {
        let host = hosts[next(hosts.len())];
        let header = format!(
            "{}=v{step}{}{}",
            names[next(names.len())],
            attributes[next(attributes.len())],
            attributes[next(attributes.len())]
        );
        let target = url(host);
        let now = NOW + i64::from(step);
        let _ = jar.set_cookie(&header, &first_party(&target), now);
        let cookies = jar.cookies();
        assert!(cookies.len() <= 8);
        for (index, cookie) in cookies.iter().enumerate() {
            assert!(
                cookies[index + 1..]
                    .iter()
                    .all(|other| !other.same_identity(cookie))
            );
            if cookie.name().starts_with("__Host-") {
                assert!(cookie.secure_only() && cookie.host_only() && cookie.path() == "/");
            }
        }
        // Expired cookies may linger until the next store, but are never sent.
        let sent = jar.cookies_for(&first_party(&target), now);
        assert!(sent.iter().all(|cookie| !cookie.is_expired(now)));
    }
}

#[test]
fn persistent_cookies_round_trip_and_session_cookies_do_not() {
    let mut jar = jar();
    set(&mut jar, "https://a.test/", "session=1").unwrap();
    set(
        &mut jar,
        "https://a.test/",
        "kept=2; Max-Age=600; Secure; HttpOnly",
    )
    .unwrap();
    set(&mut jar, "https://a.test/", "short=3; Max-Age=5").unwrap();
    let saved = jar.export_persistent(NOW);
    let mut restored = super::tests::jar();
    assert_eq!(restored.import_persistent(&saved, NOW + 10), Some(1));
    let cookie = &restored.cookies()[0];
    assert_eq!((cookie.name(), cookie.value()), ("kept", "2"));
    assert!(cookie.secure_only() && cookie.http_only());
    assert_eq!(cookie.expiry(), Some(NOW + 600));
    // Importing again adds nothing; malformed data changes nothing.
    assert_eq!(restored.import_persistent(&saved, NOW + 10), Some(0));
    let mut truncated = saved.clone();
    truncated.pop();
    assert_eq!(restored.import_persistent(&truncated, NOW), None);
    assert_eq!(restored.import_persistent(b"XXXX", NOW), None);
    assert_eq!(restored.len(), 1);
}

//! M6 network parsers and policy decisions (`rustic-net`), which M7 treats
//! as fed by a hostile peer.
//!
//! * `net-http-response`: HTTP/1.1 response framing, interim responses,
//!   chunked bodies and content codings. Invariant: the parse (head, body or
//!   error) does not depend on how the bytes are split, and the body never
//!   exceeds `max_body_bytes`.
//! * `net-cookie`: `Set-Cookie` parsing and the jar's storage model. Stored
//!   cookies respect the size limit, the `__Host-`/`__Secure-` prefixes and
//!   the jar bounds, and the `Cookie` header has no line breaks.
//! * `net-csp`: CSP parsing and every check. Report-only policies never
//!   block, and checking is deterministic.
//! * `net-cors`: CORS check and preflight validation over arbitrary response
//!   headers. A credentialed success needs the exact origin and
//!   `Access-Control-Allow-Credentials: true`.

use rustic_net::cookie::{CookieContext, CookieJar, CookieLimits};
use rustic_net::cors::{CredentialsMode, cors_check, validate_preflight};
use rustic_net::csp::{CspList, FetchDirective, InlineKind};
use rustic_net::navigation::PolicyContainer;
use rustic_net::site::PublicSuffixList;
use rustic_net::{Headers, TransportLimits, parse_response};
use rustic_url::Url;

use crate::Target;
use crate::fingerprint;

/// Seed inputs shared by the network targets, by target name.
pub(crate) fn seeds(target: &str) -> Vec<Vec<u8>> {
    match target {
        "net-http-response" => http_seeds(),
        "net-cookie" => cookie_seeds(),
        "net-csp" => csp_seeds(),
        _ => cors_seeds(),
    }
}

fn http_seeds() -> Vec<Vec<u8>> {
    let heads: [&[u8]; 12] = [
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;ext=1\r\nhello\r\n0\r\nTrailer: x\r\n\r\n",
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 204 No Content\r\n\r\n",
        b"HTTP/1.0 200 OK\r\n\r\nuntil close",
        b"HTTP/1.1 304 Not Modified\r\nETag: \"a\"\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabc",
        b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 2\r\n\r\n\x1f\x8b",
        b"HTTP/1.1 200 OK\r\nContent-Encoding: br\r\n\r\n\x0b\x02\x80hi\x03",
        b"HTTP/1.1 200 OK\r\n bad fold\r\n\r\n",
        b"HTTP/1.1 999 x\r\nA:\tb \r\n\r\n",
        b"HTTP/1.1 200 OK\nContent-Length: 1\n\nx",
    ];
    let mut seeds = Vec::new();
    for (index, head) in heads.iter().enumerate() {
        for variant in 0..10u8 {
            let mut seed = head.to_vec();
            match variant {
                0 => {}
                1 => seed.truncate(seed.len() / 2),
                2 => seed.extend_from_slice(b"trailing garbage"),
                3 => seed.insert(0, b'\r'),
                4 => seed.extend_from_slice(b"\r\n"),
                5 => seed.retain(|&byte| byte != b'\r'),
                6 => seed.extend(std::iter::repeat_n(b'a', 40 + index)),
                7 => seed = seed.to_ascii_lowercase(),
                8 => seed.push(0),
                _ => seed.reverse(),
            }
            seeds.push(seed);
        }
    }
    seeds
}

fn cookie_seeds() -> Vec<Vec<u8>> {
    let values = [
        "a=b",
        "a=b; Path=/; Secure; HttpOnly",
        "__Host-id=1; Secure; Path=/",
        "__Host-id=1; Secure; Path=/; Domain=a.test",
        "__Secure-x=1",
        "x=1; Domain=.a.test; Max-Age=60",
        "x=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT",
        "x=1; SameSite=None; Secure; Partitioned",
        "x=1; SameSite=Strict",
        "=noname",
        "x=1; Domain=test",
        "x=\u{7f}",
    ];
    let mut seeds = Vec::new();
    for value in values {
        for url in 0..10u8 {
            let mut seed = vec![url];
            seed.extend_from_slice(value.as_bytes());
            seeds.push(seed);
        }
    }
    seeds
}

fn csp_seeds() -> Vec<Vec<u8>> {
    let policies = [
        "default-src 'self'",
        "script-src 'nonce-abc' 'strict-dynamic'; object-src 'none'",
        "script-src 'sha256-47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU='",
        "style-src 'unsafe-inline' https:; img-src *",
        "default-src *.a.test:*/path/; report-uri /r",
        "upgrade-insecure-requests; sandbox allow-scripts",
        "frame-ancestors 'none'; base-uri 'self'; form-action http://b.test",
        "connect-src 'self' ws:, script-src 'unsafe-eval'",
        "script-src 'unsafe-hashes' 'sha256-x'",
        "default-src 'none'",
        ";;;",
    ];
    let mut seeds = Vec::new();
    for policy in policies {
        for mode in 0..10u8 {
            let mut seed = vec![mode];
            seed.extend_from_slice(policy.as_bytes());
            seed.extend_from_slice(b"\nhttps://a.test/x.js\nabc\nalert(1)");
            seeds.push(seed);
        }
    }
    seeds
}

fn cors_seeds() -> Vec<Vec<u8>> {
    let headers = [
        "Access-Control-Allow-Origin: *",
        "Access-Control-Allow-Origin: http://a.test\nAccess-Control-Allow-Credentials: true",
        "Access-Control-Allow-Origin: http://a.test\nAccess-Control-Allow-Methods: PUT, *\nAccess-Control-Allow-Headers: x-a, *",
        "Access-Control-Allow-Origin: null",
        "Access-Control-Allow-Origin: http://a.test, http://a.test",
        "Access-Control-Allow-Origin: http://a.test\nAccess-Control-Max-Age: 999999999999",
        "Access-Control-Allow-Origin: http://a.test\nAccess-Control-Allow-Methods: ,",
        "Access-Control-Allow-Credentials: TRUE",
        "",
        "Access-Control-Allow-Origin: http://a.test\nAccess-Control-Allow-Headers: authorization",
        "Access-Control-Allow-Origin: http://A.test",
    ];
    let mut seeds = Vec::new();
    for header in headers {
        for mode in 0..10u8 {
            let mut seed = vec![mode];
            seed.extend_from_slice(header.as_bytes());
            seeds.push(seed);
        }
    }
    seeds
}

fn text(input: &[u8]) -> String {
    String::from_utf8_lossy(input).into_owned()
}

fn headers_from(text: &str) -> Headers {
    let mut headers = Headers::new();
    for line in text.lines() {
        if let Some((name, value)) = line.split_once(':') {
            let _ = headers.append(name.trim(), value.trim().as_bytes());
        }
    }
    headers
}

/// HTTP response framing must be split-invariant and bounded.
pub(crate) struct HttpResponseTarget;

impl Target for HttpResponseTarget {
    fn name(&self) -> &'static str {
        "net-http-response"
    }

    fn max_len(&self) -> usize {
        8192
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"HTTP/1.1 ",
            b"HTTP/1.0 ",
            b"\r\n",
            b"\r\n\r\n",
            b"Content-Length: ",
            b"Transfer-Encoding: chunked",
            b"Content-Encoding: gzip",
            b"Content-Encoding: deflate",
            b"Content-Encoding: br",
            b"0\r\n\r\n",
            b"ffffffffffffffff\r\n",
            b"100 Continue",
            b"\x1f\x8b\x08\x00",
            b"\x78\x9c",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let limits = TransportLimits {
            max_body_bytes: 64 * 1024,
            max_head_bytes: 4096,
            ..TransportLimits::default()
        };
        let head = fingerprint(input) & 1 == 1;
        let whole = parse_response(&[input], &limits, head);
        if let Ok((_, body)) = &whole {
            if body.len() as u64 > limits.max_body_bytes {
                return Err(format!("body of {} bytes passed the limit", body.len()));
            }
        }
        let split = (fingerprint(input) % (input.len() as u64 + 1)) as usize;
        let mut plans: Vec<Vec<&[u8]>> = vec![vec![&input[..split], &input[split..]]];
        if input.len() <= 512 {
            plans.push(input.chunks(1).collect());
        }
        for plan in plans {
            let parsed = parse_response(&plan, &limits, head);
            if parsed != whole {
                return Err(format!(
                    "split into {} chunks changed the parse: {:?} vs {:?}",
                    plan.len(),
                    parsed
                        .as_ref()
                        .map(|(head, body)| (head.status(), body.len())),
                    whole
                        .as_ref()
                        .map(|(head, body)| (head.status(), body.len()))
                ));
            }
        }
        Ok(())
    }
}

const URLS: [&str; 5] = [
    "https://a.test/",
    "http://a.test/dir/page",
    "https://sub.a.test/x",
    "http://127.0.0.1/",
    "https://b.co.uk/",
];

/// The cookie jar keeps its invariants for any `Set-Cookie` sequence.
pub(crate) struct CookieTarget;

impl Target for CookieTarget {
    fn name(&self) -> &'static str {
        "net-cookie"
    }

    fn max_len(&self) -> usize {
        8192
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"; Secure",
            b"; HttpOnly",
            b"; Path=/",
            b"; Domain=",
            b"; Max-Age=",
            b"; Expires=",
            b"; SameSite=None",
            b"; Partitioned",
            b"__Host-",
            b"__Secure-",
            b"\n",
            b"=",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let Some((&selector, rest)) = input.split_first() else {
            return Ok(());
        };
        let limits = CookieLimits {
            per_site: 8,
            total: 16,
        };
        let suffixes = PublicSuffixList::parse("test\nco.uk\nuk\n").map_err(|e| e.to_string())?;
        let mut jar = CookieJar::new(suffixes, limits);
        let now = 1_700_000_000;
        for (index, line) in text(rest).split('\n').enumerate() {
            let url = Url::parse(URLS[(usize::from(selector) + index) % URLS.len()], None)
                .map_err(|_| "seed URL")?;
            let context = CookieContext {
                url: &url,
                same_site: selector & 1 == 0,
                top_level_navigation: selector & 2 == 0,
                safe_method: true,
                http_api: selector & 4 == 0,
                top_level_site: None,
            };
            let _ = jar.set_cookie(line, &context, now);
            if let Some(header) = jar.cookie_header(&context, now) {
                if header.contains(['\r', '\n']) {
                    return Err("a Cookie header contains a line break".into());
                }
            }
        }
        if jar.len() > limits.total {
            return Err(format!("{} cookies exceed the jar limit", jar.len()));
        }
        for cookie in jar.cookies() {
            if cookie.name().len() + cookie.value().len() > 4096 {
                return Err("a stored cookie exceeds 4096 bytes".into());
            }
            let name = cookie.name().to_ascii_lowercase();
            if name.starts_with("__secure-") && !cookie.secure_only() {
                return Err("__Secure- cookie without Secure".into());
            }
            if name.starts_with("__host-")
                && !(cookie.secure_only() && cookie.host_only() && cookie.path() == "/")
            {
                return Err("__Host- cookie breaks its prefix rules".into());
            }
        }
        Ok(())
    }
}

/// CSP checks never panic, are deterministic, and report-only policies
/// never block.
pub(crate) struct CspTarget;

impl Target for CspTarget {
    fn name(&self) -> &'static str {
        "net-csp"
    }

    fn max_len(&self) -> usize {
        4096
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"default-src ",
            b"script-src ",
            b"style-src-attr ",
            b"'self'",
            b"'none'",
            b"'nonce-",
            b"'sha256-",
            b"'strict-dynamic'",
            b"'unsafe-inline'",
            b"'unsafe-eval'",
            b"*.",
            b":*",
            b";",
            b",",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let Some((&mode, rest)) = input.split_first() else {
            return Ok(());
        };
        let text = text(rest);
        let mut parts = text.splitn(4, '\n');
        let policy = parts.next().unwrap_or_default().to_owned();
        let target = parts.next().unwrap_or_default();
        let nonce = parts.next().filter(|nonce| !nonce.is_empty());
        let source = parts.next().unwrap_or_default();
        let url = Url::parse(target, None)
            .unwrap_or_else(|_| Url::parse("https://a.test/x", None).expect("constant URL"));
        let origin = Url::parse("https://a.test/", None)
            .expect("constant URL")
            .origin();
        let report_only = mode & 1 == 1;
        let container = PolicyContainer {
            csp: if report_only {
                Vec::new()
            } else {
                vec![policy.clone()]
            },
            csp_report_only: if report_only {
                vec![policy]
            } else {
                Vec::new()
            },
            ..PolicyContainer::default()
        };
        let list = CspList::from_container(&container);
        let run = |list: &CspList| {
            let mut blocked = Vec::new();
            for directive in [
                FetchDirective::ScriptElement,
                FetchDirective::StyleElement,
                FetchDirective::Image,
                FetchDirective::Connect,
                FetchDirective::Frame,
                FetchDirective::Worker,
            ] {
                let verdict = list.check_request_with_nonce(
                    &url,
                    directive,
                    &origin,
                    usize::from(mode >> 4),
                    nonce,
                    mode & 2 == 0,
                );
                blocked.push(verdict.blocked);
            }
            for kind in [
                InlineKind::ScriptElement,
                InlineKind::ScriptAttribute,
                InlineKind::StyleElement,
                InlineKind::StyleAttribute,
            ] {
                blocked.push(list.check_inline(kind, nonce, source).blocked);
            }
            blocked.push(list.check_eval().blocked);
            blocked.push(list.check_form_action(&url, &origin).blocked);
            blocked.push(list.check_base_uri(&url, &origin).blocked);
            blocked.push(list.check_frame_ancestors(&[url.origin()], &origin).blocked);
            blocked
        };
        let first = run(&list);
        if report_only && first.iter().any(|&blocked| blocked) {
            return Err("a report-only policy blocked".into());
        }
        if run(&CspList::from_container(&container)) != first {
            return Err("CSP verdicts are not deterministic".into());
        }
        Ok(())
    }
}

/// CORS decisions over arbitrary response headers.
pub(crate) struct CorsTarget;

impl Target for CorsTarget {
    fn name(&self) -> &'static str {
        "net-cors"
    }

    fn max_len(&self) -> usize {
        4096
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"Access-Control-Allow-Origin: ",
            b"Access-Control-Allow-Credentials: true",
            b"Access-Control-Allow-Methods: ",
            b"Access-Control-Allow-Headers: ",
            b"Access-Control-Max-Age: ",
            b"http://a.test",
            b"*",
            b"\n",
            b", ",
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let Some((&mode, rest)) = input.split_first() else {
            return Ok(());
        };
        let response = headers_from(&text(rest));
        let origin = "http://a.test";
        let credentials = match mode % 3 {
            0 => CredentialsMode::Omit,
            1 => CredentialsMode::SameOrigin,
            _ => CredentialsMode::Include,
        };
        let allowed = cors_check(origin, credentials, &response).is_ok();
        if allowed && credentials == CredentialsMode::Include {
            let exact = response.combined("access-control-allow-origin").as_deref()
                == Some(origin.as_bytes());
            let flag = response
                .combined("access-control-allow-credentials")
                .as_deref()
                == Some(b"true".as_slice());
            if !exact || !flag {
                return Err("a credentialed CORS check passed without exact grants".into());
            }
        }
        let mut request = Headers::new();
        let _ = request.append("X-Custom", b"1");
        if mode & 8 == 8 {
            let _ = request.append("Authorization", b"x");
        }
        let status = if mode & 16 == 16 { 200 } else { 204 };
        if let Ok(grant) = validate_preflight(
            status,
            &response,
            origin,
            credentials,
            if mode & 32 == 32 { "PUT" } else { "DELETE" },
            &request,
            mode & 64 == 64,
        ) {
            if !allowed {
                return Err("a preflight passed without the CORS check".into());
            }
            if grant.max_age > std::time::Duration::from_secs(7200) {
                return Err("a preflight grant is cached longer than two hours".into());
            }
        }
        Ok(())
    }
}

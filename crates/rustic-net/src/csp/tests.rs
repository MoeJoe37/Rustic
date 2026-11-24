use super::*;

fn url(input: &str) -> Url {
    Url::parse(input, None).unwrap()
}

fn list(enforce: &[&str], report: &[&str]) -> CspList {
    CspList::from_container(&PolicyContainer {
        csp: enforce.iter().map(|value| (*value).to_owned()).collect(),
        csp_report_only: report.iter().map(|value| (*value).to_owned()).collect(),
        ..PolicyContainer::default()
    })
}

fn me() -> Origin {
    url("https://a.test").origin()
}

fn allowed(csp: &CspList, target: &str, directive: FetchDirective) -> bool {
    csp.check_request(&url(target), directive, &me(), 0)
        .allowed()
}

#[test]
fn parsing_keeps_the_first_directive_and_splits_policies() {
    let policy = Policy::parse(
        " Script-Src 'self'  https:;script-src *; ;bad!name x; img-src",
        Disposition::Enforce,
    );
    assert_eq!(
        policy.directive("script-src").unwrap(),
        ["'self'", "https:"]
    );
    assert_eq!(policy.directive("img-src").unwrap(), &[] as &[String]);
    assert!(policy.directive("bad!name").is_none());
    let csp = list(&["img-src 'none', script-src 'self'"], &[]);
    assert_eq!(csp.policies().len(), 2);
    assert!(list(&[""], &[]).is_empty());
}

#[test]
fn fetch_directives_fall_back_to_default_src() {
    let csp = list(
        &["default-src 'self'; img-src *; script-src-elem https://cdn.test"],
        &[],
    );
    assert!(allowed(
        &csp,
        "https://a.test/s.js",
        FetchDirective::StyleElement
    ));
    assert!(!allowed(
        &csp,
        "https://b.test/s.css",
        FetchDirective::StyleElement
    ));
    assert!(allowed(
        &csp,
        "http://anything.test/i.png",
        FetchDirective::Image
    ));
    assert!(!allowed(&csp, "data:image/png,x", FetchDirective::Image));
    assert!(allowed(
        &csp,
        "https://cdn.test/x.js",
        FetchDirective::ScriptElement
    ));
    assert!(!allowed(
        &csp,
        "https://a.test/x.js",
        FetchDirective::ScriptElement
    ));
    assert!(allowed(&csp, "https://a.test/f.woff", FetchDirective::Font));
    // Without a matching directive nothing is restricted.
    let none = list(&["img-src 'none'"], &[]);
    assert!(allowed(
        &none,
        "https://x.test/s.js",
        FetchDirective::ScriptElement
    ));
    assert!(!allowed(
        &none,
        "https://a.test/i.png",
        FetchDirective::Image
    ));
}

#[test]
fn source_expressions() {
    let origin = me();
    let check = |source: &str, target: &str| {
        matches_source_list(&url(target), &[source.to_owned()], &origin, 0)
    };
    // Schemes, with upgrades.
    assert!(check("https:", "https://x.test/"));
    assert!(check("http:", "https://x.test/"));
    assert!(!check("https:", "http://x.test/"));
    assert!(check("data:", "data:,x"));
    // '*' covers network schemes and the document's own scheme only.
    assert!(check("*", "wss://x.test/"));
    assert!(!check("*", "data:,x"));
    // Hosts, wildcards, ports, paths.
    assert!(check("b.test", "https://b.test/x"));
    assert!(
        !check("b.test", "http://b.test/x"),
        "no scheme: the document's scheme"
    );
    assert!(check("*.b.test", "https://c.b.test/"));
    assert!(!check("*.b.test", "https://b.test/"));
    assert!(check("https://b.test:8443", "https://b.test:8443/"));
    assert!(!check("https://b.test:8443", "https://b.test/"));
    assert!(check("https://b.test:*", "https://b.test:1/"));
    assert!(
        check("http://b.test", "https://b.test/"),
        "upgrade to the default port"
    );
    assert!(check("http://b.test:80", "https://b.test/"));
    assert!(check("https://b.test/lib/", "https://b.test/lib/x.js"));
    assert!(!check("https://b.test/lib/", "https://b.test/other.js"));
    assert!(check("https://b.test/lib/a.js", "https://b.test/lib/a.js"));
    assert!(!check(
        "https://b.test/lib/a.js",
        "https://b.test/lib/a.jsx"
    ));
    assert!(check("https://b.test/a%20b", "https://b.test/a%20b"));
    assert!(
        matches_source_list(
            &url("https://b.test/other"),
            &["https://b.test/lib/".into()],
            &origin,
            1
        ),
        "paths are ignored after redirects"
    );
    assert!(check("https://[::1]:8080", "https://[::1]:8080/"));
    assert!(!check("*.0.0.1", "https://127.0.0.1/"));
    // 'self' and 'none'.
    assert!(check("'self'", "https://a.test/x"));
    assert!(check("'self'", "wss://a.test/"));
    assert!(!check("'self'", "https://b.test/"));
    assert!(!check("'none'", "https://a.test/"));
    assert!(!check("'unsafe-inline'", "https://a.test/"));
    let http_self = url("http://a.test").origin();
    assert!(matches_source_list(
        &url("https://a.test/"),
        &["'self'".into()],
        &http_self,
        0
    ));
    assert!(!matches_source_list(
        &url("https://a.test/"),
        &["'self'".into()],
        &Origin::opaque(),
        0
    ));
}

#[test]
fn inline_scripts_and_styles() {
    let source = "alert(1)";
    // sha256 of "alert(1)".
    let hash = base64(digest::digest(&digest::SHA256, source.as_bytes()).as_ref());
    let csp = list(
        &[&format!(
            "script-src 'nonce-abc' 'sha256-{hash}' 'unsafe-inline'; style-src 'unsafe-inline'"
        )],
        &[],
    );
    let check = |kind, nonce, text| csp.check_inline(kind, nonce, text).allowed();
    assert!(check(InlineKind::ScriptElement, Some("abc"), "anything"));
    assert!(!check(InlineKind::ScriptElement, Some("abd"), "anything"));
    assert!(check(InlineKind::ScriptElement, None, source));
    assert!(
        !check(InlineKind::ScriptElement, None, "alert(2)"),
        "unsafe-inline is ignored with a nonce or hash"
    );
    assert!(
        !check(InlineKind::ScriptAttribute, None, source),
        "hashes need 'unsafe-hashes' for attributes"
    );
    assert!(check(InlineKind::StyleElement, None, "p{}"));
    assert!(check(InlineKind::StyleAttribute, None, "color:red"));
    let hashes = list(
        &[&format!("script-src 'unsafe-hashes' 'sha256-{hash}'")],
        &[],
    );
    assert!(
        hashes
            .check_inline(InlineKind::ScriptAttribute, None, source)
            .allowed()
    );
    let url_safe = hash.replace('+', "-").replace('/', "_");
    let base64url = list(
        &[&format!(
            "script-src 'sha256-{}'",
            url_safe.trim_end_matches('=')
        )],
        &[],
    );
    assert!(
        base64url
            .check_inline(InlineKind::ScriptElement, None, source)
            .allowed()
    );
    let strict = list(&["script-src 'unsafe-inline' 'strict-dynamic'"], &[]);
    assert!(
        !strict
            .check_inline(InlineKind::ScriptElement, None, "x")
            .allowed()
    );
    assert!(
        list(&[], &[])
            .check_inline(InlineKind::ScriptElement, None, "x")
            .allowed()
    );
    assert!(
        !list(&["default-src 'self'"], &[])
            .check_inline(InlineKind::StyleAttribute, None, "x")
            .allowed()
    );
}

#[test]
fn nonces_and_strict_dynamic_for_script_urls() {
    let csp = list(
        &["script-src 'nonce-n1' 'strict-dynamic' https://cdn.test"],
        &[],
    );
    let script = url("https://other.test/a.js");
    let check = |nonce, parser_inserted| {
        csp.check_request_with_nonce(
            &script,
            FetchDirective::ScriptElement,
            &me(),
            0,
            nonce,
            parser_inserted,
        )
        .allowed()
    };
    assert!(check(Some("n1"), true));
    assert!(
        !check(None, true),
        "strict-dynamic ignores host sources for parser-inserted scripts"
    );
    assert!(check(None, false), "script-inserted scripts are trusted");
    let plain = list(&["script-src https://cdn.test"], &[]);
    assert!(
        !plain
            .check_request_with_nonce(
                &script,
                FetchDirective::ScriptElement,
                &me(),
                0,
                None,
                false
            )
            .allowed()
    );
}

#[test]
fn eval_forms_base_frames_sandbox_and_upgrades() {
    assert!(list(&[], &[]).check_eval().allowed());
    assert!(!list(&["script-src 'self'"], &[]).check_eval().allowed());
    assert!(
        list(&["default-src 'unsafe-eval'"], &[])
            .check_eval()
            .allowed()
    );
    let csp = list(
        &["form-action 'self'; base-uri 'none'; frame-ancestors https://top.test"],
        &[],
    );
    assert!(
        csp.check_form_action(&url("https://a.test/submit"), &me())
            .allowed()
    );
    assert!(
        !csp.check_form_action(&url("https://b.test/"), &me())
            .allowed()
    );
    assert!(!csp.check_base_uri(&url("https://a.test/"), &me()).allowed());
    let top = url("https://top.test").origin();
    let evil = url("https://evil.test").origin();
    assert!(
        csp.check_frame_ancestors(std::slice::from_ref(&top), &me())
            .allowed()
    );
    assert!(!csp.check_frame_ancestors(&[top, evil], &me()).allowed());
    assert!(
        list(&["default-src *"], &[])
            .check_form_action(&url("https://b.test/"), &me())
            .allowed(),
        "form-action has no fallback"
    );
    assert_eq!(
        list(&["sandbox allow-scripts"], &[]).sandbox(),
        Some(vec!["allow-scripts".to_owned()])
    );
    assert!(sandboxes_origin(&["sandbox allow-scripts".to_owned()]));
    assert!(!sandboxes_origin(&["sandbox allow-same-origin".to_owned()]));
    assert!(!sandboxes_origin(&["img-src *".to_owned()]));
    assert_eq!(
        list(&[], &["sandbox"]).sandbox(),
        None,
        "report-only sandbox does nothing"
    );
    assert!(list(&["upgrade-insecure-requests"], &[]).upgrades_insecure_requests());
    assert_eq!(
        upgrade_url(&url("http://a.test/x?y#z")).serialize(),
        "https://a.test/x?y#z"
    );
    assert_eq!(
        upgrade_url(&url("http://a.test:8080/")).serialize(),
        "https://a.test:8080/"
    );
    assert_eq!(
        upgrade_url(&url("ws://a.test/")).serialize(),
        "wss://a.test/"
    );
    assert_eq!(
        upgrade_url(&url("https://a.test/")).serialize(),
        "https://a.test/"
    );
}

#[test]
fn report_only_policies_report_without_blocking() {
    let csp = list(&["img-src https:"], &["img-src 'self'"]);
    let verdict = csp.check_request(
        &url("https://b.test/i.png"),
        FetchDirective::Image,
        &me(),
        0,
    );
    assert!(verdict.allowed());
    assert_eq!(verdict.violations.len(), 1);
    assert_eq!(verdict.violations[0].disposition, Disposition::Report);
    assert_eq!(verdict.violations[0].directive, "img-src");
    let blocked = csp.check_request(
        &url("http://b.test/i.png#f"),
        FetchDirective::Image,
        &me(),
        0,
    );
    assert!(!blocked.allowed());
    assert_eq!(blocked.violations.len(), 2);
    assert_eq!(blocked.violations[0].blocked, "http://b.test/i.png");
    assert!(
        blocked.violations[0]
            .to_string()
            .starts_with("Refused to load")
    );
}

#[test]
fn non_ascii_tokens_never_split_characters() {
    let list = CspList::from_container(&PolicyContainer {
        csp: vec!["script-src 'self'\u{e9}w 'nonce-\u{e9}\u{e9}'".to_owned()],
        ..PolicyContainer::default()
    });
    let url = Url::parse("https://a.test/x.js", None).unwrap();
    let verdict = list.check_request_with_nonce(
        &url,
        FetchDirective::ScriptElement,
        &me(),
        0,
        Some("\u{e9}\u{e9}"),
        true,
    );
    assert!(verdict.allowed());
}

#[test]
fn wasm_compilation_has_an_independent_csp_keyword_and_default_fallback() {
    let wasm_only = list(&["default-src 'WaSm-UnSaFe-EvAl'"], &[]);
    assert!(wasm_only.check_wasm_compilation().allowed());
    assert!(!wasm_only.check_eval().allowed());

    let override_policy = list(&["default-src 'unsafe-eval'; script-src 'self'"], &[]);
    assert!(!override_policy.check_wasm_compilation().allowed());
    let report = list(&["script-src 'unsafe-eval'"], &["script-src 'none'"]);
    let verdict = report.check_wasm_compilation();
    assert!(verdict.allowed());
    assert_eq!(verdict.violations[0].blocked, "wasm-eval");

    let stacked = list(
        &["script-src 'wasm-unsafe-eval'", "default-src 'none'"],
        &[],
    );
    assert!(!stacked.check_wasm_compilation().allowed());
    assert!(
        list(&["style-src 'none'"], &[])
            .check_wasm_compilation()
            .allowed()
    );
}

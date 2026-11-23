//! The URL interface setters' basic parser state overrides.

use std::borrow::Cow;

use super::{
    EncodeSet, Host, ParseError, Run, Url, UrlParser, UrlPath, default_port, empty_host, encoding,
    query_set, separator, split_scheme, windows_drive_letter,
};

/// Mutable component of a URL record, with the URL interface's setter rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UrlComponent {
    /// Scheme, optionally ending with a colon.
    Protocol,
    /// Userinfo name.
    Username,
    /// Userinfo password.
    Password,
    /// Hostname with an optional port.
    Host,
    /// Hostname alone.
    Hostname,
    /// Port, or empty to remove it.
    Port,
    /// Hierarchical path.
    Pathname,
    /// Query with an optional leading question mark.
    Search,
    /// Fragment with an optional leading number sign.
    Hash,
}

impl UrlParser {
    /// Apply a URL interface component setter with the parser's limits.
    /// Invalid component syntax follows the web setter's recovery rules.
    /// A resource error or cancellation leaves the input record unchanged.
    pub fn with_component(
        &self,
        url: &Url,
        component: UrlComponent,
        value: &str,
    ) -> Result<Url, ParseError> {
        let mut run = Run::new(self, None);
        run.validate_record(url)?;
        if value.len() > self.limits.max_input_bytes {
            return Err(ParseError::LimitExceeded);
        }
        let mut next = url.clone();
        let special = is_special(url.scheme());
        let input = if matches!(component, UrlComponent::Username | UrlComponent::Password) {
            Cow::Borrowed(value)
        } else {
            strip_tabs_and_newlines(value)
        };
        match component {
            UrlComponent::Protocol => set_scheme(&mut next, &input)?,
            UrlComponent::Username if has_userinfo(&next) => {
                next.username = encoding::encode(&input, EncodeSet::UserInfo);
            }
            UrlComponent::Password if has_userinfo(&next) => {
                next.password = encoding::encode(&input, EncodeSet::UserInfo);
            }
            UrlComponent::Host | UrlComponent::Hostname if !is_opaque(&next) => {
                run.set_host(
                    &mut next,
                    &input,
                    special,
                    component == UrlComponent::Hostname,
                );
            }
            UrlComponent::Port if has_userinfo(&next) => {
                if value.is_empty() {
                    next.port = None;
                } else {
                    run.set_port(&mut next, &input);
                }
            }
            UrlComponent::Pathname if !is_opaque(&next) => {
                let mut path = Vec::new();
                if special || !input.is_empty() || next.host.is_none() {
                    let path_input = run.strip_separator(&input, special).unwrap_or(&input);
                    run.path(&mut path, path_input, special, url.scheme() == "file")?;
                }
                next.path = UrlPath::Segments(path);
            }
            UrlComponent::Search => {
                next.query = (!value.is_empty()).then(|| {
                    // The optional prefix is removed before parser preprocessing.
                    let input = strip_tabs_and_newlines(value.strip_prefix('?').unwrap_or(value));
                    encoding::encode(&input, query_set(special))
                });
                strip_opaque_spaces(&mut next);
            }
            UrlComponent::Hash => {
                next.fragment = (!value.is_empty()).then(|| {
                    let input = strip_tabs_and_newlines(value.strip_prefix('#').unwrap_or(value));
                    encoding::encode(&input, EncodeSet::Fragment)
                });
                strip_opaque_spaces(&mut next);
            }
            _ => {}
        }
        run.validate_record(&next)?;
        Ok(next)
    }
}

impl Run<'_> {
    fn set_host(&mut self, url: &mut Url, input: &str, special: bool, hostname: bool) {
        let end = input
            .find(|ch| separator(ch, special) || matches!(ch, '?' | '#'))
            .unwrap_or(input.len());
        let authority = &input[..end];
        if url.scheme == "file" {
            if authority.is_empty() {
                url.host = Some(empty_host());
            } else if !windows_drive_letter(authority) {
                if let Ok(host) = self.host(authority, true) {
                    url.host = Some(if host.domain() == Some("localhost") {
                        empty_host()
                    } else {
                        host
                    });
                }
            }
            return;
        }

        let (host, port) = split_setter_host(authority);
        if port.is_some() && (hostname || host.is_empty()) {
            return;
        }
        if host.is_empty() && (special || has_credentials(url) || url.port.is_some()) {
            return;
        }
        if let Ok(host) = self.host(host, special) {
            url.host = Some(host);
            if let Some(port) = port {
                self.set_port(url, port);
            }
        }
    }

    fn set_port(&mut self, url: &mut Url, input: &str) {
        let end = input
            .find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(input.len());
        if end != 0 {
            if let Ok(port) = self.port(&input[..end]) {
                url.port = Some(port).filter(|port| Some(*port) != default_port(&url.scheme));
            }
        }
    }
}

fn set_scheme(url: &mut Url, input: &str) -> Result<(), ParseError> {
    let candidate = if input.contains(':') {
        Cow::Borrowed(input)
    } else {
        Cow::Owned(format!("{input}:"))
    };
    let Some((scheme, _)) = split_scheme(&candidate) else {
        return Ok(());
    };
    let scheme = scheme.to_ascii_lowercase();
    if is_special(&url.scheme) != is_special(&scheme)
        || (scheme == "file" && (has_credentials(url) || url.port.is_some()))
        || (url.scheme == "file" && url.host.as_ref().is_some_and(Host::is_empty))
    {
        return Ok(());
    }
    if scheme == "blob" {
        return Err(ParseError::UnsupportedBlob);
    }
    if url.port == default_port(&scheme) {
        url.port = None;
    }
    url.scheme = scheme;
    Ok(())
}

fn split_setter_host(authority: &str) -> (&str, Option<&str>) {
    let mut inside_brackets = false;
    for (index, ch) in authority.char_indices() {
        match ch {
            '[' => inside_brackets = true,
            ']' => inside_brackets = false,
            ':' if !inside_brackets => return (&authority[..index], Some(&authority[index + 1..])),
            _ => {}
        }
    }
    (authority, None)
}

fn strip_tabs_and_newlines(input: &str) -> Cow<'_, str> {
    if input.contains(['\t', '\r', '\n']) {
        Cow::Owned(input.replace(['\t', '\r', '\n'], ""))
    } else {
        Cow::Borrowed(input)
    }
}

fn is_special(scheme: &str) -> bool {
    scheme == "file" || default_port(scheme).is_some()
}

fn is_opaque(url: &Url) -> bool {
    matches!(url.path, UrlPath::Opaque(_))
}

fn has_userinfo(url: &Url) -> bool {
    url.scheme != "file" && url.host.as_ref().is_some_and(|host| !host.is_empty())
}

fn has_credentials(url: &Url) -> bool {
    !url.username.is_empty() || !url.password.is_empty()
}

fn strip_opaque_spaces(url: &mut Url) {
    if url.query.is_none() && url.fragment.is_none() {
        if let UrlPath::Opaque(path) = &mut url.path {
            path.truncate(path.trim_end_matches(' ').len());
        }
    }
}

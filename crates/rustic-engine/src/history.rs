//! Session history for one top-level traversable (HTML §7.3 "session
//! history", §7.4.6 "traverse the history by a delta", "reload"), as a
//! bounded list of entries with a current index.
//!
//! The engine owns the entries; the network layer's navigation produces
//! what an entry records. Pushing truncates forward entries (HTML
//! "finalize a cross-document navigation" with history handling "push"),
//! `replace` swaps the current entry (location.replace, redirects of a
//! reload), and traversal returns the entry to load again (no bfcache:
//! every traversal reloads the document).

use rustic_url::Url;

/// Chromium's limit on entries per tab; older entries are dropped.
pub const MAX_HISTORY_ENTRIES: usize = 50;

/// One session history entry.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    /// The document URL.
    pub url: Url,
    /// The document title when it was last seen.
    pub title: String,
    /// `history.pushState`/`replaceState` serialized state, if any.
    pub state: Option<String>,
    /// The request method that created the document (`POST` entries ask
    /// before resubmission on reload or traversal).
    pub method: String,
    /// The request body to resubmit for a `POST` entry.
    pub body: Vec<u8>,
    /// Scroll position to restore, in whole CSS pixels.
    pub scroll: (i32, i32),
}

impl HistoryEntry {
    /// A `GET` entry for `url`.
    pub fn new(url: Url) -> Self {
        Self {
            url,
            title: String::new(),
            state: None,
            method: "GET".to_owned(),
            body: Vec::new(),
            scroll: (0, 0),
        }
    }

    /// Whether reloading or returning to it would resubmit a form.
    pub fn needs_resubmission(&self) -> bool {
        self.method != "GET"
    }
}

/// How a navigation affects the history (HTML "history handling").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryHandling {
    /// Add an entry after the current one, dropping forward entries.
    Push,
    /// Replace the current entry.
    Replace,
}

/// Why a history operation did nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryError {
    /// There is no entry yet.
    Empty,
    /// The delta leads outside the list.
    OutOfRange,
}

/// The session history of a top-level traversable.
#[derive(Clone, Debug, Default)]
pub struct SessionHistory {
    entries: Vec<HistoryEntry>,
    current: usize,
}

impl SessionHistory {
    /// An empty history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a committed navigation.
    pub fn commit(&mut self, entry: HistoryEntry, handling: HistoryHandling) {
        if self.entries.is_empty() {
            self.entries.push(entry);
            self.current = 0;
            return;
        }
        match handling {
            HistoryHandling::Replace => self.entries[self.current] = entry,
            HistoryHandling::Push => {
                self.entries.truncate(self.current + 1);
                self.entries.push(entry);
                if self.entries.len() > MAX_HISTORY_ENTRIES {
                    self.entries.remove(0);
                }
                self.current = self.entries.len() - 1;
            }
        }
    }

    /// The current entry.
    pub fn current(&self) -> Option<&HistoryEntry> {
        self.entries.get(self.current)
    }

    /// The current entry, to update its title, scroll or state.
    pub fn current_mut(&mut self) -> Option<&mut HistoryEntry> {
        self.entries.get_mut(self.current)
    }

    /// Every entry, oldest first.
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// `history.length`.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing was committed yet.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The current index.
    pub fn index(&self) -> usize {
        self.current
    }

    /// Whether `go(delta)` would move.
    pub fn can_go(&self, delta: isize) -> bool {
        self.target(delta).is_ok()
    }

    fn target(&self, delta: isize) -> Result<usize, HistoryError> {
        if self.entries.is_empty() {
            return Err(HistoryError::Empty);
        }
        self.current
            .checked_add_signed(delta)
            .filter(|&index| index < self.entries.len())
            .ok_or(HistoryError::OutOfRange)
    }

    /// `history.go(delta)` (`back` is -1, `forward` is +1): move and return
    /// the entry to load. `go(0)` is a reload.
    pub fn go(&mut self, delta: isize) -> Result<&HistoryEntry, HistoryError> {
        let index = self.target(delta)?;
        self.current = index;
        Ok(&self.entries[index])
    }

    /// `history.back()`.
    pub fn back(&mut self) -> Result<&HistoryEntry, HistoryError> {
        self.go(-1)
    }

    /// `history.forward()`.
    pub fn forward(&mut self) -> Result<&HistoryEntry, HistoryError> {
        self.go(1)
    }

    /// `location.reload()`: the current entry, unchanged.
    pub fn reload(&self) -> Result<&HistoryEntry, HistoryError> {
        self.current().ok_or(HistoryError::Empty)
    }

    /// `history.pushState(state, "", url)`: a same-document entry. The URL
    /// must be same origin with the current one (HTML "can have its URL
    /// rewritten"); it is resolved by the caller.
    pub fn push_state(
        &mut self,
        state: Option<String>,
        url: Option<Url>,
    ) -> Result<(), HistoryError> {
        let current = self.current().ok_or(HistoryError::Empty)?.clone();
        let url = url.unwrap_or_else(|| current.url.clone());
        if !can_rewrite(&current.url, &url) {
            return Err(HistoryError::OutOfRange);
        }
        self.commit(
            HistoryEntry {
                url,
                state,
                ..current
            },
            HistoryHandling::Push,
        );
        Ok(())
    }

    /// `history.replaceState(state, "", url)`.
    pub fn replace_state(
        &mut self,
        state: Option<String>,
        url: Option<Url>,
    ) -> Result<(), HistoryError> {
        let current = self.current_mut().ok_or(HistoryError::Empty)?;
        let url = url.unwrap_or_else(|| current.url.clone());
        if !can_rewrite(&current.url, &url) {
            return Err(HistoryError::OutOfRange);
        }
        current.url = url;
        current.state = state;
        Ok(())
    }
}

/// HTML "can have its URL rewritten": same scheme, credentials, host and
/// port for http(s); for other schemes only the query and fragment may
/// change.
pub fn can_rewrite(document: &Url, target: &Url) -> bool {
    if document.scheme() != target.scheme()
        || document.username() != target.username()
        || document.password() != target.password()
        || document.host() != target.host()
        || document.port() != target.port()
    {
        return false;
    }
    if matches!(document.scheme(), "http" | "https") {
        return true;
    }
    document.pathname() == target.pathname()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(input: &str) -> HistoryEntry {
        HistoryEntry::new(Url::parse(input, None).unwrap())
    }

    fn urls(history: &SessionHistory) -> Vec<String> {
        history
            .entries()
            .iter()
            .map(|entry| entry.url.serialize())
            .collect()
    }

    #[test]
    fn push_traverse_and_truncate() {
        let mut history = SessionHistory::new();
        assert_eq!(history.back().unwrap_err(), HistoryError::Empty);
        assert_eq!(history.reload().unwrap_err(), HistoryError::Empty);
        history.commit(entry("http://a.test/1"), HistoryHandling::Replace);
        history.commit(entry("http://a.test/2"), HistoryHandling::Push);
        history.commit(entry("http://a.test/3"), HistoryHandling::Push);
        assert_eq!((history.len(), history.index()), (3, 2));
        assert_eq!(history.back().unwrap().url.serialize(), "http://a.test/2");
        assert_eq!(history.go(-1).unwrap().url.serialize(), "http://a.test/1");
        assert_eq!(history.go(-1).unwrap_err(), HistoryError::OutOfRange);
        assert!(history.can_go(2) && !history.can_go(3));
        assert_eq!(
            history.forward().unwrap().url.serialize(),
            "http://a.test/2"
        );
        assert_eq!(history.reload().unwrap().url.serialize(), "http://a.test/2");
        assert_eq!(history.go(0).unwrap().url.serialize(), "http://a.test/2");
        // A new navigation from the middle drops the forward entry.
        history.commit(entry("http://a.test/4"), HistoryHandling::Push);
        assert_eq!(
            urls(&history),
            ["http://a.test/1", "http://a.test/2", "http://a.test/4"]
        );
        history.commit(entry("http://a.test/5"), HistoryHandling::Replace);
        assert_eq!(
            urls(&history),
            ["http://a.test/1", "http://a.test/2", "http://a.test/5"]
        );
        assert_eq!(
            history.go(isize::MIN).unwrap_err(),
            HistoryError::OutOfRange
        );
        assert_eq!(
            history.go(isize::MAX).unwrap_err(),
            HistoryError::OutOfRange
        );
    }

    #[test]
    fn the_list_is_bounded() {
        let mut history = SessionHistory::new();
        for index in 0..MAX_HISTORY_ENTRIES + 10 {
            history.commit(
                entry(&format!("http://a.test/{index}")),
                HistoryHandling::Push,
            );
        }
        assert_eq!(history.len(), MAX_HISTORY_ENTRIES);
        assert_eq!(history.entries()[0].url.serialize(), "http://a.test/10");
        assert_eq!(history.index(), MAX_HISTORY_ENTRIES - 1);
    }

    #[test]
    fn post_entries_and_state() {
        let mut history = SessionHistory::new();
        let mut post = entry("http://a.test/submit");
        post.method = "POST".to_owned();
        post.body = b"q=1".to_vec();
        history.commit(post, HistoryHandling::Push);
        assert!(history.reload().unwrap().needs_resubmission());
        let same = Url::parse("http://a.test/other?x", None).unwrap();
        history
            .push_state(Some("{\"n\":1}".to_owned()), Some(same))
            .unwrap();
        assert_eq!(
            history.current().unwrap().state.as_deref(),
            Some("{\"n\":1}")
        );
        assert_eq!(history.len(), 2);
        let cross = Url::parse("http://b.test/", None).unwrap();
        assert_eq!(
            history.push_state(None, Some(cross.clone())).unwrap_err(),
            HistoryError::OutOfRange
        );
        assert_eq!(
            history.replace_state(None, Some(cross)).unwrap_err(),
            HistoryError::OutOfRange
        );
        history.replace_state(Some("2".to_owned()), None).unwrap();
        assert_eq!(history.current().unwrap().state.as_deref(), Some("2"));
        history.current_mut().unwrap().title = "Title".to_owned();
        assert_eq!(history.back().unwrap().state, None);
    }

    #[test]
    fn url_rewriting_rules() {
        let url = |input: &str| Url::parse(input, None).unwrap();
        assert!(can_rewrite(
            &url("https://a.test/x"),
            &url("https://a.test/y?q#f")
        ));
        assert!(!can_rewrite(
            &url("https://a.test/x"),
            &url("http://a.test/x")
        ));
        assert!(!can_rewrite(
            &url("https://a.test/x"),
            &url("https://a.test:444/x")
        ));
        assert!(!can_rewrite(
            &url("https://a.test/x"),
            &url("https://u@a.test/x")
        ));
        assert!(can_rewrite(&url("file:///a/b"), &url("file:///a/b?q")));
        assert!(!can_rewrite(&url("file:///a/b"), &url("file:///a/c")));
    }
}

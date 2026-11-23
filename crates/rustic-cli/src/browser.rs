//! The browser shell's model (UI-01): tabs, each with its session history,
//! load state, zoom, scroll position and find query; the address field and
//! find field being edited; and the keyboard shortcuts and strip clicks
//! that drive them. It holds no pixels and starts no processes: the
//! window's content ([`crate::view`]) renders what it asks for and reports
//! back. Everything here is the browser's own state, kept apart from any
//! document (UI-01: "keep chrome separate from DOM").

use std::path::{Path, PathBuf};

use rustic_shell::{Ime, Input, Key, Modifiers, Response, ScrollInput};

use crate::chrome::{self, AddressView, FindView, Region, Strip, TabView};

/// Most tabs one window holds.
pub const MAX_TABS: usize = 32;
/// Most session history entries one tab keeps (the oldest are dropped).
pub const MAX_HISTORY: usize = 50;
/// Longest address or find text, in characters.
pub const MAX_FIELD: usize = 2048;
/// The zoom steps, as in other browsers.
pub const ZOOM_LEVELS: [f64; 17] = [
    0.25, 0.33, 0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0,
];

/// What a tab shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    /// The empty page.
    Blank,
    /// A local file.
    File(PathBuf),
    /// A network URL (`http:` or `https:`), serialized.
    Url(String),
}

impl Target {
    /// What the user typed into the address field, as a target. Absolute
    /// `http(s):` and `file:` URLs, existing local paths (relative to
    /// `base`), `about:blank`, and bare host names (which get `https://`).
    /// There is no search: anything else is an error the field shows.
    pub fn from_entry(text: &str, base: &Path) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() || text.eq_ignore_ascii_case("about:blank") {
            return Ok(Self::Blank);
        }
        if let Ok(url) = rustic_url::Url::parse(text, None) {
            match url.scheme() {
                "http" | "https" => return Ok(Self::Url(url.serialize())),
                "file" => {
                    return Ok(Self::File(PathBuf::from(url.pathname())));
                }
                // A Windows drive path parses as a one-letter scheme.
                scheme if scheme.len() > 1 => {
                    return Err(format!("{scheme}: addresses cannot be opened here"));
                }
                _ => {}
            }
        }
        let path = base.join(text);
        if path.is_file() {
            return Ok(Self::File(path));
        }
        let host = text.split(['/', '?', '#']).next().unwrap_or_default();
        if !text.contains(char::is_whitespace)
            && host.contains('.')
            && !host.starts_with('.')
            && !host.ends_with('.')
        {
            if let Ok(url) = rustic_url::Url::parse(&format!("https://{text}"), None) {
                return Ok(Self::Url(url.serialize()));
            }
        }
        Err(format!(
            "\"{}\" is not an address or a file (there is no search)",
            text.chars().take(80).collect::<String>()
        ))
    }

    /// How the tab and the edited address show it.
    pub fn label(&self) -> String {
        match self {
            Self::Blank => "about:blank".to_owned(),
            Self::File(path) => path.to_string_lossy().into_owned(),
            Self::Url(url) => url.clone(),
        }
    }
}

/// A tab's load state.
#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    /// Showing its document, or nothing to load.
    Idle,
    /// A navigation or reload is in flight.
    Loading,
    /// The user stopped the load.
    Stopped,
    /// The load failed; the trusted error page shows why.
    Failed(String),
}

/// One session history entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// What it shows.
    pub target: Target,
    /// Its scroll position, restored on back and forward.
    pub scroll: (f64, f64),
}

/// One tab's find query and the page's answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Find {
    /// The query.
    pub field: LineEdit,
    /// The active match (counted from 0).
    pub index: usize,
    /// The match count and active match the page reported, with the
    /// active match's top and bottom in CSS pixels.
    pub result: Option<(usize, usize, f64, f64)>,
}

/// One tab.
#[derive(Clone, Debug)]
pub struct Tab {
    /// Its identity in this window (never reused).
    pub id: u64,
    history: Vec<Entry>,
    index: usize,
    /// Its load state.
    pub load: Load,
    /// Its zoom factor.
    pub zoom: f64,
    /// The committed URL the broker reported for the current entry (the
    /// address display's source; never a renderer's claim).
    pub committed: Option<rustic_url::Url>,
    /// Its find bar, when open.
    pub find: Option<Find>,
    /// Bumped by every navigation, reload and stop, so results of older
    /// loads are recognized and dropped.
    pub generation: u64,
}

impl Tab {
    fn new(target: Target, id: u64) -> Self {
        Self {
            id,
            history: vec![Entry {
                target,
                scroll: (0.0, 0.0),
            }],
            index: 0,
            load: Load::Idle,
            zoom: 1.0,
            committed: None,
            find: None,
            generation: 0,
        }
    }

    /// The current entry.
    pub fn entry(&self) -> &Entry {
        &self.history[self.index]
    }

    /// The current entry, mutably (the scroll position).
    pub fn entry_mut(&mut self) -> &mut Entry {
        &mut self.history[self.index]
    }

    /// Whether back leads anywhere.
    pub fn can_back(&self) -> bool {
        self.index > 0
    }

    /// Whether forward leads anywhere.
    pub fn can_forward(&self) -> bool {
        self.index + 1 < self.history.len()
    }

    /// The history, oldest first, and the current index.
    #[cfg(test)]
    pub fn history(&self) -> (&[Entry], usize) {
        (&self.history, self.index)
    }

    fn start_load(&mut self) {
        self.generation += 1;
        self.committed = None;
        self.load = if self.entry().target == Target::Blank {
            Load::Idle
        } else {
            Load::Loading
        };
        if let Some(find) = &mut self.find {
            find.result = None;
            find.index = 0;
        }
    }

    fn navigate(&mut self, target: Target) {
        self.history.truncate(self.index + 1);
        self.history.push(Entry {
            target,
            scroll: (0.0, 0.0),
        });
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
        self.index = self.history.len() - 1;
        self.start_load();
    }

    fn title(&self) -> String {
        match (&self.committed, &self.entry().target) {
            (Some(url), _) => {
                let address = chrome::address(url);
                if url.scheme() == "file" {
                    address
                        .detail
                        .rsplit('/')
                        .next()
                        .unwrap_or_default()
                        .to_owned()
                } else {
                    address.host
                }
            }
            (None, Target::File(path)) => path
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            (None, target) => target.label(),
        }
    }
}

/// A one-line text field: the address or the find query.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineEdit {
    text: String,
    /// The caret, in characters.
    caret: usize,
    /// Whether everything is selected (typing replaces it).
    selected: bool,
    /// The IME composition at the caret.
    preedit: String,
}

impl LineEdit {
    /// A field holding `text`, all selected.
    pub fn selected(text: &str) -> Self {
        Self {
            text: text.chars().take(MAX_FIELD).collect(),
            caret: text.chars().count().min(MAX_FIELD),
            selected: true,
            preedit: String::new(),
        }
    }

    /// The text.
    pub fn text(&self) -> &str {
        &self.text
    }

    fn byte(&self, caret: usize) -> usize {
        self.text
            .char_indices()
            .nth(caret)
            .map_or(self.text.len(), |(at, _)| at)
    }

    /// Type `text` at the caret (replacing a selection).
    pub fn insert(&mut self, text: &str) {
        if self.selected {
            self.text.clear();
            self.caret = 0;
            self.selected = false;
        }
        let room = MAX_FIELD.saturating_sub(self.text.chars().count());
        let typed: String = text
            .chars()
            .filter(|c| !c.is_control())
            .take(room)
            .collect();
        let at = self.byte(self.caret);
        self.text.insert_str(at, &typed);
        self.caret += typed.chars().count();
    }

    /// Apply an editing key; returns whether it was one.
    pub fn key(&mut self, key: &Key) -> bool {
        let count = self.text.chars().count();
        match key {
            Key::Backspace | Key::Delete if self.selected => {
                self.text.clear();
                self.caret = 0;
            }
            Key::Backspace if self.caret > 0 => {
                let (from, to) = (self.byte(self.caret - 1), self.byte(self.caret));
                self.text.replace_range(from..to, "");
                self.caret -= 1;
            }
            Key::Delete if self.caret < count => {
                let (from, to) = (self.byte(self.caret), self.byte(self.caret + 1));
                self.text.replace_range(from..to, "");
            }
            Key::Left if self.selected => self.caret = 0,
            Key::Left => self.caret = self.caret.saturating_sub(1),
            Key::Right => self.caret = (self.caret + 1).min(count),
            Key::Home => self.caret = 0,
            Key::End => self.caret = count,
            Key::Backspace | Key::Delete => {}
            _ => return false,
        }
        self.selected = false;
        true
    }

    /// Apply an IME event.
    pub fn ime(&mut self, ime: &Ime) {
        match ime {
            Ime::Preedit(text, _) => {
                self.preedit = text.chars().filter(|c| !c.is_control()).collect();
            }
            Ime::Commit(text) => {
                self.preedit.clear();
                self.insert(text);
            }
            Ime::Enabled | Ime::Disabled => self.preedit.clear(),
        }
    }

    fn view(&self) -> (String, usize, bool, String) {
        (
            self.text.clone(),
            self.caret,
            self.selected,
            self.preedit.clone(),
        )
    }
}

/// Where keyboard input goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Focus {
    /// The page (scrolling; page input once renderers are long-lived).
    Page,
    /// The address field.
    Address,
    /// The find field.
    Find,
}

/// A browser command, from a shortcut or a strip click.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Open a tab with the empty page and edit its address.
    NewTab,
    /// Close a tab (the active one with `None`); the last one closes the
    /// window.
    CloseTab(Option<usize>),
    /// Switch to the next tab (wrapping).
    NextTab,
    /// Switch to the previous tab (wrapping).
    PreviousTab,
    /// Switch to this tab (the last one for 9, as in other browsers).
    SelectTab(usize),
    /// Go back in the active tab's history.
    Back,
    /// Go forward.
    Forward,
    /// Reload the active tab.
    Reload,
    /// Stop its load.
    Stop,
    /// Zoom in one step.
    ZoomIn,
    /// Zoom out one step.
    ZoomOut,
    /// Back to 100%.
    ZoomReset,
    /// Edit the address (everything selected).
    FocusAddress,
    /// Open or focus the find bar.
    Find,
    /// The next match.
    FindNext,
    /// The previous match.
    FindPrevious,
    /// Close the find bar.
    CloseFind,
    /// Open this target in the active tab.
    Navigate(Target),
    /// Close the download notice.
    CloseDownload,
}

/// The browser shortcut for a key, if any. `mod` is Command on macOS and
/// Control elsewhere.
pub fn shortcut(key: &Key, modifiers: Modifiers) -> Option<Command> {
    let command = modifiers.command();
    let alt_only = modifiers.alt && !modifiers.ctrl && !modifiers.meta;
    Some(match key {
        Key::Character('t') if command => Command::NewTab,
        Key::Character('w') if command => Command::CloseTab(None),
        Key::Function(4) if modifiers.ctrl => Command::CloseTab(None),
        Key::Tab if modifiers.ctrl && modifiers.shift => Command::PreviousTab,
        Key::Tab if modifiers.ctrl => Command::NextTab,
        Key::PageDown if modifiers.ctrl => Command::NextTab,
        Key::PageUp if modifiers.ctrl => Command::PreviousTab,
        Key::Character(c @ '1'..='9') if command || (cfg!(target_os = "linux") && alt_only) => {
            Command::SelectTab(*c as usize - '1' as usize)
        }
        Key::Left if alt_only => Command::Back,
        Key::Right if alt_only => Command::Forward,
        Key::Character('[') if command => Command::Back,
        Key::Character(']') if command => Command::Forward,
        Key::Function(5) if modifiers.plain() => Command::Reload,
        Key::Character('r') if command => Command::Reload,
        Key::Character('=' | '+') if command => Command::ZoomIn,
        Key::Character('-') if command => Command::ZoomOut,
        Key::Character('0') if command => Command::ZoomReset,
        Key::Character('l') if command => Command::FocusAddress,
        Key::Character('d') if alt_only => Command::FocusAddress,
        Key::Function(6) if modifiers.plain() => Command::FocusAddress,
        Key::Character('f') if command => Command::Find,
        Key::Function(3) if modifiers.shift => Command::FindPrevious,
        Key::Function(3) => Command::FindNext,
        Key::Character('g') if command && modifiers.shift => Command::FindPrevious,
        Key::Character('g') if command => Command::FindNext,
        _ => return None,
    })
}

/// What the window's content must do after the browser changed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Effect {
    /// Repaint the window.
    pub redraw: bool,
    /// Close the window.
    pub close: bool,
}

/// One browser window.
#[derive(Debug)]
pub struct Browser {
    tabs: Vec<Tab>,
    active: usize,
    focus: Focus,
    address: LineEdit,
    /// Why the last address entry was refused, shown until the next edit.
    pub entry_error: Option<String>,
    /// The window's permission decisions and prompt (TD-173).
    pub permissions: chrome::Permissions,
    /// The download notice.
    pub download: Option<String>,
    /// The profile's label.
    pub profile: String,
    /// Where relative address entries resolve.
    base: PathBuf,
    next_id: u64,
}

impl Browser {
    /// A window with one tab showing `first`.
    pub fn new(first: Target, profile: String, base: PathBuf) -> Self {
        let mut tab = Tab::new(first, 0);
        tab.start_load();
        let focus = if tab.entry().target == Target::Blank {
            Focus::Address
        } else {
            Focus::Page
        };
        Self {
            tabs: vec![tab],
            active: 0,
            focus,
            address: LineEdit::default(),
            entry_error: None,
            permissions: chrome::Permissions::default(),
            download: None,
            profile,
            base,
            next_id: 1,
        }
    }

    /// The tabs.
    #[cfg(test)]
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    /// The active tab's index.
    #[cfg(test)]
    pub fn active(&self) -> usize {
        self.active
    }

    /// The active tab.
    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    /// The active tab, mutably.
    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// A tab by identity, mutably.
    pub fn tab_by_id(&mut self, id: u64) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.id == id)
    }

    /// The navigation in tab `id` turned out to be a download: say so in
    /// the download notice and, as other browsers do, keep the page that
    /// was shown before it (the entry for the download is dropped).
    pub fn download_finished(&mut self, id: u64, notice: String) {
        self.download = Some(notice.clone());
        let Some(tab) = self.tab_by_id(id) else {
            return;
        };
        if tab.index > 0 {
            tab.history.remove(tab.index);
            tab.index -= 1;
            tab.start_load();
        } else {
            tab.load = Load::Failed(notice);
        }
    }

    /// Where keyboard input goes.
    pub fn focus(&self) -> Focus {
        self.focus
    }

    fn focus_address(&mut self) {
        let shown = match (&self.tab().committed, &self.tab().entry().target) {
            (Some(url), _) => url.serialize_without_fragment(),
            (None, target) => match target {
                Target::Blank => String::new(),
                other => other.label(),
            },
        };
        self.address = LineEdit::selected(&shown);
        self.focus = Focus::Address;
    }

    /// Apply a command.
    pub fn apply(&mut self, command: Command) -> Effect {
        let redraw = Effect {
            redraw: true,
            close: false,
        };
        match command {
            Command::NewTab => {
                if self.tabs.len() >= MAX_TABS {
                    return Effect::default();
                }
                self.tabs.push(Tab::new(Target::Blank, self.next_id));
                self.next_id += 1;
                self.switch(self.tabs.len() - 1);
                self.focus_address();
            }
            Command::CloseTab(index) => {
                let index = index.unwrap_or(self.active);
                if index >= self.tabs.len() {
                    return Effect::default();
                }
                if self.tabs.len() == 1 {
                    return Effect {
                        redraw: false,
                        close: true,
                    };
                }
                self.tabs.remove(index);
                if self.active > index || self.active >= self.tabs.len() {
                    self.active = self.active.saturating_sub(1);
                }
                self.after_switch();
            }
            Command::NextTab => self.switch((self.active + 1) % self.tabs.len()),
            Command::PreviousTab => {
                self.switch((self.active + self.tabs.len() - 1) % self.tabs.len());
            }
            Command::SelectTab(index) => {
                let index = if index >= 8 {
                    self.tabs.len() - 1
                } else {
                    index
                };
                if index >= self.tabs.len() {
                    return Effect::default();
                }
                self.switch(index);
            }
            Command::Back | Command::Forward => {
                let tab = self.tab_mut();
                let can = if command == Command::Back {
                    tab.can_back()
                } else {
                    tab.can_forward()
                };
                if !can {
                    return Effect::default();
                }
                if command == Command::Back {
                    tab.index -= 1;
                } else {
                    tab.index += 1;
                }
                tab.start_load();
                self.leave_address();
                self.permissions.navigated(std::time::Instant::now());
            }
            Command::Reload => {
                self.tab_mut().start_load();
                self.permissions.navigated(std::time::Instant::now());
            }
            Command::Stop => {
                let tab = self.tab_mut();
                if tab.load != Load::Loading {
                    return Effect::default();
                }
                tab.generation += 1;
                tab.load = Load::Stopped;
            }
            Command::ZoomIn | Command::ZoomOut | Command::ZoomReset => {
                let tab = self.tab_mut();
                let at = ZOOM_LEVELS
                    .iter()
                    .position(|&level| level >= tab.zoom - 1e-9)
                    .unwrap_or(ZOOM_LEVELS.len() - 1);
                tab.zoom = match command {
                    Command::ZoomIn => ZOOM_LEVELS[(at + 1).min(ZOOM_LEVELS.len() - 1)],
                    Command::ZoomOut => ZOOM_LEVELS[at.saturating_sub(1)],
                    _ => 1.0,
                };
            }
            Command::FocusAddress => self.focus_address(),
            Command::Find => {
                let tab = self.tab_mut();
                match &mut tab.find {
                    Some(find) => find.field.selected = true,
                    None => tab.find = Some(Find::default()),
                }
                self.focus = Focus::Find;
            }
            Command::FindNext | Command::FindPrevious => {
                let Some(find) = &mut self.tab_mut().find else {
                    return self.apply(Command::Find);
                };
                let count = find.result.map_or(0, |(count, ..)| count);
                if count == 0 {
                    return Effect::default();
                }
                find.index = if command == Command::FindNext {
                    (find.index + 1) % count
                } else {
                    (find.index + count - 1) % count
                };
            }
            Command::CloseFind => {
                self.tab_mut().find = None;
                if self.focus == Focus::Find {
                    self.focus = Focus::Page;
                }
            }
            Command::Navigate(target) => {
                self.tab_mut().navigate(target);
                self.leave_address();
                self.permissions.navigated(std::time::Instant::now());
            }
            Command::CloseDownload => self.download = None,
        }
        redraw
    }

    fn switch(&mut self, index: usize) {
        if index != self.active {
            // The prompt belongs to the tab's document; switching away
            // dismisses it, so its answer cannot land on another page.
            self.permissions.navigated(std::time::Instant::now());
        }
        self.active = index;
        self.after_switch();
    }

    fn after_switch(&mut self) {
        if self.tab().entry().target == Target::Blank && self.tab().load == Load::Idle {
            self.focus_address();
        } else {
            self.leave_address();
        }
    }

    fn leave_address(&mut self) {
        self.entry_error = None;
        if self.focus == Focus::Address {
            self.focus = Focus::Page;
        }
    }

    /// One input from the window. Returns the shell's response and what
    /// to do.
    pub fn input(&mut self, input: &Input, strip_width: f64) -> (Response, Effect) {
        let handled = |effect: Effect| {
            (
                Response {
                    handled: true,
                    redraw: effect.redraw,
                    close: effect.close,
                },
                effect,
            )
        };
        match input {
            Input::Key(key, modifiers) => {
                if let Some(command) = shortcut(key, *modifiers) {
                    return handled(self.apply(command));
                }
                self.key(key, *modifiers)
            }
            Input::Text(text) => match self.focus {
                Focus::Address => {
                    self.address.insert(text);
                    self.entry_error = None;
                    handled(Effect {
                        redraw: true,
                        close: false,
                    })
                }
                Focus::Find => {
                    self.edit_find(|field| field.insert(text));
                    handled(Effect {
                        redraw: true,
                        close: false,
                    })
                }
                Focus::Page => (Response::UNHANDLED, Effect::default()),
            },
            Input::Ime(ime) => match self.focus {
                Focus::Address => {
                    self.address.ime(ime);
                    handled(Effect {
                        redraw: true,
                        close: false,
                    })
                }
                Focus::Find => {
                    let commits = matches!(ime, Ime::Commit(_));
                    if commits {
                        self.edit_find(|field| field.ime(ime));
                    } else if let Some(find) = &mut self.tab_mut().find {
                        find.field.ime(ime);
                    }
                    handled(Effect {
                        redraw: true,
                        close: false,
                    })
                }
                Focus::Page => (Response::UNHANDLED, Effect::default()),
            },
            Input::ChromeClick(x, y) => {
                let region = self.with_strip(|strip| chrome::hit(strip, strip_width, *x, *y));
                match region {
                    Some(region) => handled(self.click(region)),
                    None => (Response::HANDLED, Effect::default()),
                }
            }
            Input::PageClick(..) => {
                // A click on the page takes focus from the fields.
                let was = self.focus;
                self.focus = Focus::Page;
                if was == Focus::Address {
                    self.entry_error = None;
                }
                (
                    Response {
                        handled: false,
                        redraw: was != Focus::Page,
                        close: false,
                    },
                    Effect {
                        redraw: was != Focus::Page,
                        close: false,
                    },
                )
            }
            Input::Scroll(_) => (Response::UNHANDLED, Effect::default()),
        }
    }

    fn edit_find(&mut self, edit: impl FnOnce(&mut LineEdit)) {
        if let Some(find) = &mut self.tab_mut().find {
            let before = find.field.text().to_owned();
            edit(&mut find.field);
            if find.field.text() != before {
                find.index = 0;
                find.result = None;
            }
        }
    }

    fn key(&mut self, key: &Key, modifiers: Modifiers) -> (Response, Effect) {
        let redraw = Effect {
            redraw: true,
            close: false,
        };
        let done = |effect: Effect| {
            (
                Response {
                    handled: true,
                    redraw: effect.redraw,
                    close: effect.close,
                },
                effect,
            )
        };
        match self.focus {
            Focus::Address => match key {
                Key::Enter => {
                    let typed = self.address.text().to_owned();
                    match Target::from_entry(&typed, &self.base) {
                        Ok(target) => done(self.apply(Command::Navigate(target))),
                        Err(error) => {
                            self.entry_error = Some(error);
                            done(redraw)
                        }
                    }
                }
                Key::Escape => {
                    self.leave_address();
                    done(redraw)
                }
                Key::Tab => {
                    self.leave_address();
                    done(redraw)
                }
                other => {
                    // Typed characters arrive as text; their key presses
                    // are used here so the page does not scroll.
                    if self.address.key(other) {
                        self.entry_error = None;
                    }
                    done(redraw)
                }
            },
            Focus::Find => match key {
                Key::Enter if modifiers.shift => done(self.apply(Command::FindPrevious)),
                Key::Enter => done(self.apply(Command::FindNext)),
                Key::Escape => done(self.apply(Command::CloseFind)),
                other => {
                    let other = other.clone();
                    self.edit_find(|field| {
                        field.key(&other);
                    });
                    done(redraw)
                }
            },
            Focus::Page => match key {
                Key::Escape if self.permissions.pending().is_some() => {
                    let answered = self
                        .permissions
                        .answer(chrome::Answer::Dismiss, std::time::Instant::now())
                        .is_some();
                    done(Effect {
                        redraw: answered,
                        close: false,
                    })
                }
                Key::Escape if self.tab().load == Load::Loading => done(self.apply(Command::Stop)),
                Key::Escape if self.tab().find.is_some() => done(self.apply(Command::CloseFind)),
                _ => (Response::UNHANDLED, Effect::default()),
            },
        }
    }

    fn click(&mut self, region: Region) -> Effect {
        let now = std::time::Instant::now();
        let redraw = Effect {
            redraw: true,
            close: false,
        };
        match region {
            Region::Tab(index) => self.apply(Command::SelectTab(index.min(7))),
            Region::CloseTab(index) => self.apply(Command::CloseTab(Some(index))),
            Region::NewTab => self.apply(Command::NewTab),
            Region::Back => self.apply(Command::Back),
            Region::Forward => self.apply(Command::Forward),
            Region::Reload if self.tab().load == Load::Loading => self.apply(Command::Stop),
            Region::Reload => self.apply(Command::Reload),
            Region::Address => {
                if self.focus != Focus::Address {
                    self.focus_address();
                }
                redraw
            }
            Region::Zoom => self.apply(Command::ZoomReset),
            Region::FindField => {
                self.focus = Focus::Find;
                redraw
            }
            Region::FindPrevious => self.apply(Command::FindPrevious),
            Region::FindNext => self.apply(Command::FindNext),
            Region::FindClose => self.apply(Command::CloseFind),
            Region::Allow | Region::Block => {
                let answer = if region == Region::Allow {
                    chrome::Answer::Allow
                } else {
                    chrome::Answer::Block
                };
                Effect {
                    redraw: self.permissions.answer(answer, now).is_some(),
                    close: false,
                }
            }
            Region::DownloadClose => self.apply(Command::CloseDownload),
        }
    }

    /// The strip for the current state.
    pub fn with_strip<R>(&self, f: impl FnOnce(&Strip<'_>) -> R) -> R {
        let tabs: Vec<TabView> = self
            .tabs
            .iter()
            .map(|tab| TabView {
                title: tab.title(),
                loading: tab.load == Load::Loading,
            })
            .collect();
        let tab = self.tab();
        let address = if self.focus == Focus::Address {
            let (text, caret, selected, preedit) = self.address.view();
            AddressView::Editing {
                text,
                caret,
                selected,
                preedit,
            }
        } else {
            AddressView::Committed(match (&tab.committed, &tab.entry().target) {
                (Some(url), _) => chrome::address(url),
                (None, target) => chrome::Address {
                    security: match target {
                        Target::File(_) => chrome::Security::File,
                        _ => chrome::Security::Opaque,
                    },
                    host: match target {
                        Target::File(_) => "file://".to_owned(),
                        other => other.label(),
                    },
                    detail: match target {
                        Target::File(path) => path.to_string_lossy().into_owned(),
                        _ => String::new(),
                    },
                },
            })
        };
        let find = tab.find.as_ref().map(|find| {
            let (text, caret, _, preedit) = find.field.view();
            FindView {
                text,
                caret,
                preedit,
                result: find.result.map(|(count, active, ..)| (count, active)),
            }
        });
        let download = self.download.clone().or_else(|| self.entry_error.clone());
        let strip = Strip {
            tabs: &tabs,
            active: self.active,
            back: tab.can_back(),
            forward: tab.can_forward(),
            loading: tab.load == Load::Loading,
            address,
            zoom: tab.zoom,
            find,
            prompt: self.permissions.pending(),
            download,
            profile: &self.profile,
        };
        f(&strip)
    }

    /// The text-entry caret area for the input method, in CSS pixels of
    /// the window, when a field has focus.
    pub fn text_entry(&self, strip_width: f64) -> Option<(f64, f64, f64, f64)> {
        let wanted = match self.focus {
            Focus::Page => return None,
            Focus::Address => Region::Address,
            Focus::Find => Region::FindField,
        };
        self.with_strip(|strip| {
            chrome::layout(strip, strip_width)
                .into_iter()
                .find(|(region, _)| *region == wanted)
                .map(|(_, rect)| rect)
        })
    }

    /// The browser's state on one line (scripted window tests): tabs,
    /// focus, the active tab's load state, history position, zoom,
    /// scroll, find result, the address shown, the field being edited and
    /// any notice. Text is escaped to printable ASCII.
    pub fn describe(&self) -> String {
        let tab = self.tab();
        let load = match &tab.load {
            Load::Idle => "idle".to_owned(),
            Load::Loading => "loading".to_owned(),
            Load::Stopped => "stopped".to_owned(),
            Load::Failed(_) => "failed".to_owned(),
        };
        let focus = match self.focus {
            Focus::Page => "page",
            Focus::Address => "address",
            Focus::Find => "find",
        };
        let find = tab.find.as_ref().map_or_else(
            || "-".to_owned(),
            |find| match find.result {
                Some((count, active, ..)) => format!("{active}/{count}"),
                None => "?".to_owned(),
            },
        );
        let address = tab
            .committed
            .as_ref()
            .map_or_else(|| tab.entry().target.label(), rustic_url::Url::serialize);
        let field = match self.focus {
            Focus::Address => self.address.text().to_owned(),
            Focus::Find => tab
                .find
                .as_ref()
                .map(|find| find.field.text().to_owned())
                .unwrap_or_default(),
            Focus::Page => String::new(),
        };
        let notice = self
            .download
            .clone()
            .or_else(|| self.entry_error.clone())
            .unwrap_or_default();
        let line = format!(
            "tabs={} active={} focus={focus} load={load} history={}/{} zoom={} scroll={},{} \
             find={find} address={address} field=[{field}] notice=[{notice}]",
            self.tabs.len(),
            self.active,
            tab.index + 1,
            tab.history.len(),
            tab.zoom,
            tab.entry().scroll.0,
            tab.entry().scroll.1,
        );
        line.chars()
            .map(|c| {
                if c.is_ascii_graphic() || c == ' ' {
                    c
                } else {
                    '?'
                }
            })
            .collect()
    }

    /// Apply a scroll input to the active tab; `page` is the page area's
    /// height in CSS pixels. Returns whether the position changed.
    pub fn scroll(&mut self, input: &ScrollInput, scale: f64, page: f64) -> bool {
        let entry = self.tab_mut().entry_mut();
        let next = rustic_shell::scrolled_by(input, entry.scroll, scale, page);
        let changed = next != entry.scroll;
        entry.scroll = next;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browser() -> Browser {
        Browser::new(
            Target::Url("https://a.test/".into()),
            "Default".into(),
            PathBuf::from("."),
        )
    }

    fn key(c: char) -> Input {
        Input::Key(
            Key::Character(c),
            Modifiers {
                ctrl: !cfg!(target_os = "macos"),
                meta: cfg!(target_os = "macos"),
                ..Modifiers::default()
            },
        )
    }

    #[test]
    fn entries_become_targets_without_search() {
        let base = std::env::temp_dir();
        assert_eq!(
            Target::from_entry("https://a.test/x", &base),
            Ok(Target::Url("https://a.test/x".into()))
        );
        assert_eq!(
            Target::from_entry("example.org/path", &base),
            Ok(Target::Url("https://example.org/path".into()))
        );
        assert_eq!(Target::from_entry("  ", &base), Ok(Target::Blank));
        assert!(Target::from_entry("two words", &base).is_err());
        assert!(Target::from_entry("javascript:alert(1)", &base).is_err());
        assert!(Target::from_entry("localhost", &base).is_err());
        let file = base.join(format!("rustic-browser-{}.html", std::process::id()));
        std::fs::write(&file, "<p>x").unwrap();
        assert_eq!(
            Target::from_entry(file.file_name().unwrap().to_str().unwrap(), &base),
            Ok(Target::File(file.clone()))
        );
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn history_goes_back_and_forward_and_new_navigations_drop_forward() {
        let mut b = browser();
        b.apply(Command::Navigate(Target::Url("https://b.test/".into())));
        b.tab_mut().entry_mut().scroll = (0.0, 300.0);
        b.apply(Command::Navigate(Target::Url("https://c.test/".into())));
        assert!(b.tab().can_back() && !b.tab().can_forward());
        b.apply(Command::Back);
        assert_eq!(
            b.tab().entry().target,
            Target::Url("https://b.test/".into())
        );
        // The entry's scroll position comes back with it.
        assert_eq!(b.tab().entry().scroll, (0.0, 300.0));
        assert_eq!(b.tab().load, Load::Loading);
        b.apply(Command::Back);
        assert!(!b.tab().can_back());
        assert_eq!(b.apply(Command::Back), Effect::default());
        b.apply(Command::Forward);
        b.apply(Command::Navigate(Target::Url("https://d.test/".into())));
        let (entries, index) = b.tab().history();
        assert_eq!(entries.len(), 3);
        assert_eq!(index, 2);
        assert!(!b.tab().can_forward());
        for n in 0..MAX_HISTORY + 5 {
            b.apply(Command::Navigate(Target::Url(format!(
                "https://e{n}.test/"
            ))));
        }
        assert_eq!(b.tab().history().0.len(), MAX_HISTORY);
    }

    #[test]
    fn tabs_open_switch_and_close() {
        let mut b = browser();
        let (response, _) = b.input(&key('t'), 800.0);
        assert!(response.handled && response.redraw);
        assert_eq!(b.tabs().len(), 2);
        assert_eq!(b.active(), 1);
        // A new tab edits its address.
        assert_eq!(b.focus(), Focus::Address);
        b.input(&Input::Text("b.test".into()), 800.0);
        b.input(&Input::Key(Key::Enter, Modifiers::default()), 800.0);
        assert_eq!(
            b.tab().entry().target,
            Target::Url("https://b.test/".into())
        );
        assert_eq!(b.focus(), Focus::Page);
        b.apply(Command::NextTab);
        assert_eq!(b.active(), 0);
        b.apply(Command::PreviousTab);
        assert_eq!(b.active(), 1);
        b.apply(Command::SelectTab(0));
        assert_eq!(b.active(), 0);
        b.apply(Command::SelectTab(8));
        assert_eq!(b.active(), 1);
        b.apply(Command::CloseTab(Some(0)));
        assert_eq!(b.tabs().len(), 1);
        assert_eq!(b.active(), 0);
        assert_eq!(
            b.tab().entry().target,
            Target::Url("https://b.test/".into())
        );
        // Closing the last tab closes the window.
        assert!(b.apply(Command::CloseTab(None)).close);
        for _ in 0..MAX_TABS + 3 {
            b.apply(Command::NewTab);
        }
        assert_eq!(b.tabs().len(), MAX_TABS);
    }

    #[test]
    fn reload_and_stop_change_the_generation_so_old_results_drop() {
        let mut b = browser();
        let first = b.tab().generation;
        assert_eq!(b.tab().load, Load::Loading);
        b.apply(Command::Stop);
        assert_eq!(b.tab().load, Load::Stopped);
        assert!(b.tab().generation > first);
        assert_eq!(b.apply(Command::Stop), Effect::default());
        let stopped = b.tab().generation;
        b.apply(Command::Reload);
        assert_eq!(b.tab().load, Load::Loading);
        assert!(b.tab().generation > stopped);
        // Escape on the page stops a load.
        b.input(&Input::Key(Key::Escape, Modifiers::default()), 800.0);
        assert_eq!(b.tab().load, Load::Stopped);
    }

    #[test]
    fn zoom_steps_through_the_levels_and_clamps() {
        let mut b = browser();
        b.apply(Command::ZoomIn);
        assert_eq!(b.tab().zoom, 1.1);
        b.apply(Command::ZoomReset);
        for _ in 0..30 {
            b.apply(Command::ZoomOut);
        }
        assert_eq!(b.tab().zoom, 0.25);
        for _ in 0..30 {
            b.apply(Command::ZoomIn);
        }
        assert_eq!(b.tab().zoom, 5.0);
        // Zoom is per tab.
        b.apply(Command::NewTab);
        assert_eq!(b.tab().zoom, 1.0);
    }

    #[test]
    fn the_address_field_edits_with_the_caret_and_ime() {
        let mut b = browser();
        b.apply(Command::FocusAddress);
        // Everything is selected; typing replaces it.
        b.input(&Input::Text("ac.test".into()), 800.0);
        for _ in 0..6 {
            b.input(&Input::Key(Key::Left, Modifiers::default()), 800.0);
        }
        b.input(&Input::Text("b".into()), 800.0);
        b.input(&Input::Key(Key::End, Modifiers::default()), 800.0);
        b.input(&Input::Key(Key::Backspace, Modifiers::default()), 800.0);
        b.input(&Input::Ime(Ime::Preedit("x".into(), None)), 800.0);
        b.input(&Input::Ime(Ime::Commit("t/日本".into())), 800.0);
        assert_eq!(b.address.text(), "abc.tes".to_owned() + "t/日本");
        // Space typed into the field does not scroll the page.
        let (response, _) = b.input(&Input::Key(Key::Space, Modifiers::default()), 800.0);
        assert!(response.handled);
        b.input(&Input::Key(Key::Enter, Modifiers::default()), 800.0);
        match &b.tab().entry().target {
            Target::Url(url) => assert!(url.starts_with("https://abc.test/")),
            other => panic!("{other:?}"),
        }
        // A refused entry keeps the field and says why.
        b.apply(Command::FocusAddress);
        b.input(&Input::Text("not an address".into()), 800.0);
        b.input(&Input::Key(Key::Enter, Modifiers::default()), 800.0);
        assert_eq!(b.focus(), Focus::Address);
        assert!(b.entry_error.is_some());
        b.input(&Input::Key(Key::Escape, Modifiers::default()), 800.0);
        assert_eq!(b.focus(), Focus::Page);
        assert!(b.entry_error.is_none());
    }

    #[test]
    fn find_cycles_through_reported_matches() {
        let mut b = browser();
        b.input(&key('f'), 800.0);
        assert_eq!(b.focus(), Focus::Find);
        b.input(&Input::Text("word".into()), 800.0);
        assert_eq!(b.tab().find.as_ref().unwrap().field.text(), "word");
        // Nothing reported yet: next does nothing.
        b.input(&Input::Key(Key::Enter, Modifiers::default()), 800.0);
        assert_eq!(b.tab().find.as_ref().unwrap().index, 0);
        b.tab_mut().find.as_mut().unwrap().result = Some((3, 0, 10.0, 20.0));
        b.input(&Input::Key(Key::Enter, Modifiers::default()), 800.0);
        assert_eq!(b.tab().find.as_ref().unwrap().index, 1);
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        b.input(&Input::Key(Key::Enter, shift), 800.0);
        b.input(&Input::Key(Key::Enter, shift), 800.0);
        assert_eq!(b.tab().find.as_ref().unwrap().index, 2);
        // Editing the query restarts at the first match.
        b.input(&Input::Text("s".into()), 800.0);
        assert_eq!(b.tab().find.as_ref().unwrap().index, 0);
        assert_eq!(b.tab().find.as_ref().unwrap().result, None);
        b.input(&Input::Key(Key::Escape, Modifiers::default()), 800.0);
        assert!(b.tab().find.is_none());
        assert_eq!(b.focus(), Focus::Page);
    }

    #[test]
    fn strip_clicks_reach_the_regions_they_draw() {
        let mut b = browser();
        b.apply(Command::NewTab);
        let regions = b.with_strip(|strip| chrome::layout(strip, 800.0));
        let center = |wanted: Region| {
            let (_, (x, y, w, h)) = *regions.iter().find(|(r, _)| *r == wanted).unwrap();
            Input::ChromeClick(x + w / 2.0, y + h / 2.0)
        };
        b.input(&center(Region::Tab(0)), 800.0);
        assert_eq!(b.active(), 0);
        b.input(&center(Region::Zoom), 800.0);
        assert_eq!(b.tab().zoom, 1.0);
        b.input(&center(Region::Address), 800.0);
        assert_eq!(b.focus(), Focus::Address);
        b.input(&center(Region::CloseTab(1)), 800.0);
        assert_eq!(b.tabs().len(), 1);
        // A click on the page takes focus from the address.
        b.input(&Input::PageClick(5.0, 5.0, Modifiers::default()), 800.0);
        assert_eq!(b.focus(), Focus::Page);
        let html = b.with_strip(|strip| chrome::document(strip, 800));
        assert!(html.is_ascii());
    }

    #[test]
    fn switching_tabs_dismisses_the_prompt() {
        let mut b = browser();
        b.apply(Command::NewTab);
        b.apply(Command::SelectTab(0));
        let origin = rustic_url::Url::parse("https://a.test/", None)
            .unwrap()
            .origin();
        b.permissions.request(
            &origin,
            chrome::Permission::Camera,
            true,
            std::time::Instant::now(),
        );
        assert!(b.permissions.pending().is_some());
        b.apply(Command::SelectTab(1));
        assert!(b.permissions.pending().is_none());
    }
}

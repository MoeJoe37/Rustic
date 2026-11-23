//! Accessibility roles: the WAI-ARIA 1.2 role set Rustic exposes, plus the
//! few host-language roles platform APIs distinguish (text nodes, the
//! document, labels).

macro_rules! roles {
    ($($variant:ident = $name:literal $(| $aria:literal)?,)*) => {
        /// The role of one accessibility node.
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        #[non_exhaustive]
        pub enum Role {
            $(
                #[doc = concat!("`", $name, "`")]
                $variant,
            )*
        }

        impl Role {
            /// Every role, in declaration order (the wire code is the index).
            pub const ALL: &'static [Role] = &[$(Role::$variant,)*];

            /// The role's name in dumps: the ARIA role token where one exists.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Role::$variant => $name,)*
                }
            }

            /// The role an ARIA `role` token names, if Rustic supports it as
            /// an author role (abstract roles and host-only roles are not).
            pub fn from_aria(token: &str) -> Option<Role> {
                $($(
                    if token.eq_ignore_ascii_case($aria) {
                        return Some(Role::$variant);
                    }
                )?)*
                None
            }
        }
    };
}

roles! {
    Document = "document" | "document",
    StaticText = "text",
    Generic = "generic" | "generic",
    Alert = "alert" | "alert",
    AlertDialog = "alertdialog" | "alertdialog",
    Application = "application" | "application",
    Article = "article" | "article",
    Banner = "banner" | "banner",
    Blockquote = "blockquote" | "blockquote",
    Button = "button" | "button",
    Caption = "caption" | "caption",
    Cell = "cell" | "cell",
    CheckBox = "checkbox" | "checkbox",
    Code = "code" | "code",
    ColumnHeader = "columnheader" | "columnheader",
    ComboBox = "combobox" | "combobox",
    Complementary = "complementary" | "complementary",
    ContentInfo = "contentinfo" | "contentinfo",
    Definition = "definition" | "definition",
    Deletion = "deletion" | "deletion",
    Dialog = "dialog" | "dialog",
    Directory = "directory" | "directory",
    Emphasis = "emphasis" | "emphasis",
    Feed = "feed" | "feed",
    Figure = "figure" | "figure",
    Form = "form" | "form",
    Grid = "grid" | "grid",
    GridCell = "gridcell" | "gridcell",
    Group = "group" | "group",
    Heading = "heading" | "heading",
    Image = "img" | "img",
    Insertion = "insertion" | "insertion",
    Label = "label",
    Legend = "legend",
    Link = "link" | "link",
    List = "list" | "list",
    ListBox = "listbox" | "listbox",
    ListItem = "listitem" | "listitem",
    Log = "log" | "log",
    Main = "main" | "main",
    Mark = "mark" | "mark",
    Marquee = "marquee" | "marquee",
    Math = "math" | "math",
    Menu = "menu" | "menu",
    MenuBar = "menubar" | "menubar",
    MenuItem = "menuitem" | "menuitem",
    MenuItemCheckBox = "menuitemcheckbox" | "menuitemcheckbox",
    MenuItemRadio = "menuitemradio" | "menuitemradio",
    Meter = "meter" | "meter",
    Navigation = "navigation" | "navigation",
    Note = "note" | "note",
    ListBoxOption = "option" | "option",
    Paragraph = "paragraph" | "paragraph",
    ProgressIndicator = "progressbar" | "progressbar",
    RadioButton = "radio" | "radio",
    RadioGroup = "radiogroup" | "radiogroup",
    Region = "region" | "region",
    Row = "row" | "row",
    RowGroup = "rowgroup" | "rowgroup",
    RowHeader = "rowheader" | "rowheader",
    ScrollBar = "scrollbar" | "scrollbar",
    Search = "search" | "search",
    SearchBox = "searchbox" | "searchbox",
    Separator = "separator" | "separator",
    Slider = "slider" | "slider",
    SpinButton = "spinbutton" | "spinbutton",
    Status = "status" | "status",
    Strong = "strong" | "strong",
    Subscript = "subscript" | "subscript",
    Superscript = "superscript" | "superscript",
    Switch = "switch" | "switch",
    Tab = "tab" | "tab",
    Table = "table" | "table",
    TabList = "tablist" | "tablist",
    TabPanel = "tabpanel" | "tabpanel",
    Term = "term" | "term",
    TextInput = "textbox" | "textbox",
    Time = "time" | "time",
    Timer = "timer" | "timer",
    Toolbar = "toolbar" | "toolbar",
    Tooltip = "tooltip" | "tooltip",
    Tree = "tree" | "tree",
    TreeGrid = "treegrid" | "treegrid",
    TreeItem = "treeitem" | "treeitem",
    LineBreak = "linebreak",
    DescriptionList = "descriptionlist",
    Details = "details",
    DisclosureTriangle = "summary",
    Abbr = "abbr",
}

impl Role {
    /// The role's wire code.
    pub fn code(self) -> u16 {
        Self::ALL
            .iter()
            .position(|role| *role == self)
            .map_or(0, |index| index as u16)
    }

    /// The role with wire code `code`.
    pub fn from_code(code: u16) -> Option<Role> {
        Self::ALL.get(usize::from(code)).copied()
    }

    /// Whether AccName step 2F computes this role's name from its content.
    pub const fn name_from_content(self) -> bool {
        matches!(
            self,
            Role::Button
                | Role::Cell
                | Role::CheckBox
                | Role::ColumnHeader
                | Role::GridCell
                | Role::Heading
                | Role::Link
                | Role::MenuItem
                | Role::MenuItemCheckBox
                | Role::MenuItemRadio
                | Role::ListBoxOption
                | Role::RadioButton
                | Role::Row
                | Role::RowHeader
                | Role::Switch
                | Role::Tab
                | Role::Tooltip
                | Role::TreeItem
                | Role::DisclosureTriangle
                | Role::Term
                | Role::StaticText
        )
    }

    /// Whether names of this role are prohibited (ARIA 1.2 "name
    /// prohibited"): an author name is ignored.
    pub const fn name_prohibited(self) -> bool {
        matches!(
            self,
            Role::Caption
                | Role::Code
                | Role::Deletion
                | Role::Emphasis
                | Role::Insertion
                | Role::Paragraph
                | Role::Strong
                | Role::Subscript
                | Role::Superscript
                | Role::Mark
                | Role::Time
        )
    }

    /// Roles whose value the user can type (AccName "embedded control").
    pub const fn is_text_input(self) -> bool {
        matches!(
            self,
            Role::TextInput | Role::SearchBox | Role::ComboBox | Role::SpinButton
        )
    }

    /// Widgets with a range value.
    pub const fn is_range(self) -> bool {
        matches!(
            self,
            Role::Slider
                | Role::SpinButton
                | Role::ProgressIndicator
                | Role::Meter
                | Role::ScrollBar
        )
    }

    /// Roles that carry a checked state.
    pub const fn is_checkable(self) -> bool {
        matches!(
            self,
            Role::CheckBox
                | Role::RadioButton
                | Role::Switch
                | Role::MenuItemCheckBox
                | Role::MenuItemRadio
        )
    }

    /// Landmark roles (screen readers list them for navigation).
    pub const fn is_landmark(self) -> bool {
        matches!(
            self,
            Role::Banner
                | Role::Complementary
                | Role::ContentInfo
                | Role::Form
                | Role::Main
                | Role::Navigation
                | Role::Region
                | Role::Search
        )
    }

    /// Live-region roles and their implicit politeness.
    pub const fn implicit_live(self) -> Option<Live> {
        match self {
            Role::Alert => Some(Live::Assertive),
            Role::Log | Role::Status => Some(Live::Polite),
            _ => None,
        }
    }
}

/// `aria-live` politeness.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Live {
    /// Announce when idle.
    Polite,
    /// Announce immediately.
    Assertive,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_and_aria_tokens_resolve() {
        for role in Role::ALL {
            assert_eq!(Role::from_code(role.code()), Some(*role));
        }
        assert_eq!(Role::from_aria("BUTTON"), Some(Role::Button));
        assert_eq!(Role::from_aria("img"), Some(Role::Image));
        // Host-only roles are not author roles.
        assert_eq!(Role::from_aria("text"), None);
        assert_eq!(Role::from_aria("label"), None);
        assert_eq!(Role::from_aria("widget"), None);
    }
}

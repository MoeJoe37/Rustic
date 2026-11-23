//! The page's accessibility tree on the platform's accessibility API
//! (M8 `a11y.bridges`, ADR 0073).
//!
//! AccessKit carries one tree to UI Automation on Windows,
//! NSAccessibility on macOS and AT-SPI on Linux and the BSDs. This module
//! turns a [`rustic_a11y::AxTree`] (CSS pixels, viewport coordinates) into
//! an AccessKit update under the window's root node, and an AccessKit
//! action request back into a [`rustic_a11y::ActionRequest`] for one page
//! node. The tree comes from a renderer and has already been decoded as
//! hostile input; nothing here trusts it further than its types.

use accesskit::{
    Action, ActionData, Affine, Invalid, Live, Node, NodeId, Rect, Role, TextPosition,
    TextSelection, Toggled, TreeId, TreeInfo, TreeUpdate,
};
use rustic_a11y::{AxId, AxNode, AxTree, Checked};

/// The window's own node.
pub(crate) const WINDOW: NodeId = NodeId(0);

/// The id of a text field's synthetic text run (AccessKit exposes text
/// editing through `TextRun` children).
fn run_id(field: AxId) -> NodeId {
    NodeId(field.0 | 1 << 63)
}

fn role(node: &AxNode) -> Role {
    use rustic_a11y::Role as R;
    match node.role {
        R::Document => Role::Document,
        R::StaticText => Role::Label,
        R::Generic => Role::GenericContainer,
        R::Alert => Role::Alert,
        R::AlertDialog => Role::AlertDialog,
        R::Application => Role::Application,
        R::Article => Role::Article,
        R::Banner => Role::Banner,
        R::Blockquote => Role::Blockquote,
        R::Button => Role::Button,
        R::Caption => Role::Caption,
        R::Cell => Role::Cell,
        R::CheckBox => Role::CheckBox,
        R::Code => Role::Code,
        R::ColumnHeader => Role::ColumnHeader,
        R::ComboBox if node.states.editable => Role::EditableComboBox,
        R::ComboBox => Role::ComboBox,
        R::Complementary => Role::Complementary,
        R::ContentInfo => Role::ContentInfo,
        R::Definition => Role::Definition,
        R::Deletion => Role::ContentDeletion,
        R::Dialog => Role::Dialog,
        R::Directory | R::List => Role::List,
        R::Emphasis => Role::Emphasis,
        R::Feed => Role::Feed,
        R::Figure => Role::Figure,
        R::Form => Role::Form,
        R::Grid => Role::Grid,
        R::GridCell => Role::GridCell,
        R::Group => Role::Group,
        R::Heading => Role::Heading,
        R::Image => Role::Image,
        R::Insertion => Role::ContentInsertion,
        R::Label => Role::Label,
        R::Legend => Role::Legend,
        R::Link => Role::Link,
        R::ListBox => Role::ListBox,
        R::ListItem => Role::ListItem,
        R::Log => Role::Log,
        R::Main => Role::Main,
        R::Mark => Role::Mark,
        R::Marquee => Role::Marquee,
        R::Math => Role::Math,
        R::Menu => Role::Menu,
        R::MenuBar => Role::MenuBar,
        R::MenuItem => Role::MenuItem,
        R::MenuItemCheckBox => Role::MenuItemCheckBox,
        R::MenuItemRadio => Role::MenuItemRadio,
        R::Meter => Role::Meter,
        R::Navigation => Role::Navigation,
        R::Note => Role::Note,
        R::ListBoxOption => Role::ListBoxOption,
        R::Paragraph => Role::Paragraph,
        R::ProgressIndicator => Role::ProgressIndicator,
        R::RadioButton => Role::RadioButton,
        R::RadioGroup => Role::RadioGroup,
        R::Region => Role::Region,
        R::Row => Role::Row,
        R::RowGroup => Role::RowGroup,
        R::RowHeader => Role::RowHeader,
        R::ScrollBar => Role::ScrollBar,
        R::Search => Role::Search,
        R::SearchBox => Role::SearchInput,
        R::Separator => Role::Splitter,
        R::Slider => Role::Slider,
        R::SpinButton => Role::SpinButton,
        R::Status => Role::Status,
        R::Strong => Role::Strong,
        R::Subscript | R::Superscript => Role::GenericContainer,
        R::Switch => Role::Switch,
        R::Tab => Role::Tab,
        R::Table => Role::Table,
        R::TabList => Role::TabList,
        R::TabPanel => Role::TabPanel,
        R::Term => Role::Term,
        R::TextInput if node.states.protected => Role::PasswordInput,
        R::TextInput if node.states.multiline => Role::MultilineTextInput,
        R::TextInput => Role::TextInput,
        R::Time => Role::Time,
        R::Timer => Role::Timer,
        R::Toolbar => Role::Toolbar,
        R::Tooltip => Role::Tooltip,
        R::Tree => Role::Tree,
        R::TreeGrid => Role::TreeGrid,
        R::TreeItem => Role::TreeItem,
        R::LineBreak => Role::LineBreak,
        R::DescriptionList => Role::DescriptionList,
        R::Details => Role::Details,
        R::DisclosureTriangle => Role::DisclosureTriangle,
        R::Abbr => Role::Abbr,
        _ => Role::GenericContainer,
    }
}

fn toggled(value: Checked) -> Toggled {
    match value {
        Checked::True => Toggled::True,
        Checked::False => Toggled::False,
        Checked::Mixed => Toggled::Mixed,
    }
}

fn rect(bounds: rustic_a11y::Rect) -> Rect {
    Rect {
        x0: bounds.x,
        y0: bounds.y,
        x1: bounds.x + bounds.width,
        y1: bounds.y + bounds.height,
    }
}

/// Characters of `text` before UTF-16 offset `offset`.
fn char_index(text: &str, offset: u32) -> usize {
    let mut units = 0u32;
    for (index, c) in text.chars().enumerate() {
        if units >= offset {
            return index;
        }
        units += c.len_utf16() as u32;
    }
    text.chars().count()
}

/// UTF-16 offset of character `index` of `text`.
fn utf16_offset(text: &str, index: usize) -> u32 {
    text.chars().take(index).map(|c| c.len_utf16() as u32).sum()
}

fn page_node(ax: &AxNode) -> Node {
    let mut node = Node::new(role(ax));
    if ax.role == rustic_a11y::Role::StaticText {
        // AccessKit reads a label's text from its value.
        node.set_value(ax.name.as_str());
    } else if !ax.name.is_empty() {
        node.set_label(ax.name.as_str());
    }
    if !ax.description.is_empty() {
        node.set_description(ax.description.as_str());
    }
    if let Some(value) = &ax.value {
        node.set_value(value.as_str());
    }
    if let Some(placeholder) = &ax.placeholder {
        node.set_placeholder(placeholder.as_str());
    }
    if let Some(url) = &ax.url {
        node.set_url(url.as_str());
    }
    if let Some(level) = ax.level {
        node.set_level(level as usize);
    }
    if let Some(bounds) = ax.bounds {
        node.set_bounds(rect(bounds));
    }
    let s = &ax.states;
    if let Some(checked) = s.checked.or(s.pressed) {
        node.set_toggled(toggled(checked));
    }
    if let Some(expanded) = s.expanded {
        node.set_expanded(expanded);
    }
    if let Some(selected) = s.selected {
        node.set_selected(selected);
    }
    if s.disabled {
        node.set_disabled();
    }
    if s.required {
        node.set_required();
    }
    if s.invalid {
        node.set_invalid(Invalid::True);
    }
    if s.readonly {
        node.set_read_only();
    }
    if s.multiselectable {
        node.set_multiselectable();
    }
    if s.modal {
        node.set_modal();
    }
    if s.busy {
        node.set_busy();
    }
    if let Some(live) = ax.live {
        node.set_live(match live {
            rustic_a11y::Live::Polite => Live::Polite,
            rustic_a11y::Live::Assertive => Live::Assertive,
        });
    }
    if let Some(range) = ax.range {
        if let Some(min) = range.min {
            node.set_min_numeric_value(min);
        }
        if let Some(max) = range.max {
            node.set_max_numeric_value(max);
        }
        if let Some(now) = range.now {
            node.set_numeric_value(now);
        }
    }
    let actions = ax.actions();
    for (on, action) in [
        (actions.focus, Action::Focus),
        (actions.click, Action::Click),
        (actions.set_value, Action::SetValue),
        (actions.set_selection, Action::SetTextSelection),
        (actions.scroll_into_view, Action::ScrollIntoView),
    ] {
        if on {
            node.add_action(action);
        }
    }
    node
}

/// A full AccessKit tree: the window (titled `title`, `size` device
/// pixels) holding the page, whose CSS-pixel coordinates are offset by
/// `top` CSS pixels (the trusted strip above it) and scaled by `scale`.
pub(crate) fn tree_update(
    page: Option<&AxTree>,
    title: &str,
    size: (u32, u32),
    scale: f64,
    top: f64,
) -> TreeUpdate {
    let mut window = Node::new(Role::Window);
    window.set_label(title);
    window.set_bounds(Rect {
        x0: 0.0,
        y0: 0.0,
        x1: f64::from(size.0),
        y1: f64::from(size.1),
    });
    let Some(page) = page.filter(|page| !page.nodes.is_empty()) else {
        return TreeUpdate {
            nodes: vec![(WINDOW, window)],
            tree: Some(TreeInfo::new(WINDOW)),
            tree_id: TreeId::ROOT,
            focus: WINDOW,
        };
    };
    let index = page.index();
    let mut nodes = Vec::with_capacity(page.nodes.len() + 1);
    let root = NodeId(page.root().id.0);
    window.set_children(vec![root]);
    nodes.push((WINDOW, window));
    for (i, ax) in page.nodes.iter().enumerate() {
        let id = NodeId(ax.id.0);
        let mut node = page_node(ax);
        let mut children: Vec<NodeId> = ax
            .children
            .iter()
            .filter(|child| index.contains_key(child))
            .map(|child| NodeId(child.0))
            .collect();
        if i == 0 {
            node.set_transform(Affine::scale(scale) * Affine::translate((0.0, top)));
        }
        // Editable text: one text run holding the value, so assistive
        // technology can read characters and move the caret.
        let run = run_id(ax.id);
        if ax.states.editable && !index.contains_key(&AxId(run.0)) {
            let text = ax.value.clone().unwrap_or_default();
            let mut text_run = Node::new(Role::TextRun);
            let lengths: Vec<u8> = text.chars().map(|c| c.len_utf8() as u8).collect();
            text_run.set_value(text.as_str());
            text_run.set_character_lengths(lengths);
            if let Some(bounds) = ax.bounds {
                text_run.set_bounds(rect(bounds));
            }
            if let Some((anchor, focus)) = ax.selection {
                node.set_text_selection(TextSelection {
                    anchor: TextPosition {
                        node: run,
                        character_index: char_index(&text, anchor),
                    },
                    focus: TextPosition {
                        node: run,
                        character_index: char_index(&text, focus),
                    },
                });
            }
            children.push(run);
            nodes.push((run, text_run));
        }
        node.set_children(children);
        nodes.push((id, node));
    }
    let focus = if index.contains_key(&page.focus) && page.focus != page.root().id {
        NodeId(page.focus.0)
    } else {
        root
    };
    TreeUpdate {
        nodes,
        tree: Some(TreeInfo::new(WINDOW)),
        tree_id: TreeId::ROOT,
        focus,
    }
}

/// The page request an AccessKit action stands for, if the target is a
/// page node that supports it.
pub(crate) fn page_action(
    page: &AxTree,
    request: &accesskit::ActionRequest,
) -> Option<(AxId, rustic_a11y::ActionRequest)> {
    let mut target = AxId(request.target_node.0);
    if page.get(target).is_none() {
        // A text run stands for its field.
        target = AxId(request.target_node.0 & !(1 << 63));
    }
    let node = page.get(target)?;
    let actions = node.actions();
    let action = match (request.action, &request.data) {
        (Action::Focus, _) if actions.focus => rustic_a11y::ActionRequest::Focus,
        (Action::Click, _) if actions.click => rustic_a11y::ActionRequest::Click,
        (Action::SetValue, Some(ActionData::Value(value))) if actions.set_value => {
            rustic_a11y::ActionRequest::SetValue(value.to_string())
        }
        (Action::SetTextSelection, Some(ActionData::SetTextSelection(selection)))
            if actions.set_selection =>
        {
            let text = node.value.as_deref().unwrap_or("");
            rustic_a11y::ActionRequest::SetSelection(
                utf16_offset(text, selection.anchor.character_index),
                utf16_offset(text, selection.focus.character_index),
            )
        }
        (Action::ScrollIntoView, _) if actions.scroll_into_view => {
            rustic_a11y::ActionRequest::ScrollIntoView
        }
        _ => return None,
    };
    Some((target, action))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustic_a11y::{AxNode as Ax, Role as R};

    fn ax(id: u64, role: R, name: &str) -> Ax {
        Ax {
            name: name.into(),
            ..placeholder(id, role)
        }
    }

    fn placeholder(id: u64, role: R) -> Ax {
        Ax {
            id: AxId(id),
            role,
            name: String::new(),
            description: String::new(),
            value: None,
            placeholder: None,
            url: None,
            states: rustic_a11y::States::default(),
            level: None,
            range: None,
            live: None,
            bounds: None,
            selection: None,
            children: Vec::new(),
        }
    }

    fn sample() -> AxTree {
        let mut doc = ax(1, R::Document, "Page");
        doc.children = vec![AxId(2), AxId(3)];
        let mut button = ax(2, R::Button, "Send");
        button.states.focusable = true;
        button.bounds = Some(rustic_a11y::Rect {
            x: 10.0,
            y: 20.0,
            width: 30.0,
            height: 40.0,
        });
        let mut field = ax(3, R::TextInput, "Name");
        field.value = Some("a\u{1F600}b".into());
        field.states.editable = true;
        field.states.focusable = true;
        field.states.focused = true;
        // UTF-16 offsets 1..3 select the emoji (characters 1..2).
        field.selection = Some((1, 3));
        AxTree {
            nodes: vec![doc, button, field],
            focus: AxId(3),
            truncated: false,
        }
    }

    #[test]
    fn the_page_sits_under_the_window_scaled_below_the_strip() {
        let update = tree_update(Some(&sample()), "Title", (800, 600), 2.0, 30.0);
        assert_eq!(update.focus, NodeId(3));
        let get = |id: NodeId| {
            update
                .nodes
                .iter()
                .find(|(node, _)| *node == id)
                .map(|(_, node)| node)
                .unwrap()
        };
        assert_eq!(get(WINDOW).role(), Role::Window);
        assert_eq!(get(WINDOW).label(), Some("Title"));
        assert_eq!(get(WINDOW).children(), &[NodeId(1)]);
        let doc = get(NodeId(1));
        assert_eq!(doc.role(), Role::Document);
        let corner = *doc.transform().unwrap() * accesskit::Point::new(10.0, 20.0);
        assert_eq!((corner.x, corner.y), (20.0, 100.0));
        let button = get(NodeId(2));
        assert_eq!(button.role(), Role::Button);
        assert_eq!(button.label(), Some("Send"));
        assert!(button.supports_action(Action::Focus));
        assert!(button.supports_action(Action::Click));
        let field = get(NodeId(3));
        assert_eq!(field.value(), Some("a\u{1F600}b"));
        let run = run_id(AxId(3));
        assert_eq!(field.children(), &[run]);
        assert_eq!(get(run).character_lengths(), &[1, 4, 1]);
        let selection = field.text_selection().unwrap();
        assert_eq!(
            (
                selection.anchor.character_index,
                selection.focus.character_index
            ),
            (1, 2)
        );
        // Without a page, only the window.
        assert_eq!(tree_update(None, "T", (1, 1), 1.0, 0.0).nodes.len(), 1);
    }

    #[test]
    fn actions_map_back_to_page_requests() {
        let page = sample();
        let request =
            |target: NodeId, action: Action, data: Option<ActionData>| accesskit::ActionRequest {
                action,
                target_tree: TreeId::ROOT,
                target_node: target,
                data,
            };
        assert_eq!(
            page_action(&page, &request(NodeId(2), Action::Click, None)),
            Some((AxId(2), rustic_a11y::ActionRequest::Click))
        );
        assert_eq!(
            page_action(
                &page,
                &request(
                    NodeId(3),
                    Action::SetValue,
                    Some(ActionData::Value("x".into()))
                )
            ),
            Some((AxId(3), rustic_a11y::ActionRequest::SetValue("x".into())))
        );
        let run = run_id(AxId(3));
        let selection = TextSelection {
            anchor: TextPosition {
                node: run,
                character_index: 2,
            },
            focus: TextPosition {
                node: run,
                character_index: 3,
            },
        };
        assert_eq!(
            page_action(
                &page,
                &request(
                    run,
                    Action::SetTextSelection,
                    Some(ActionData::SetTextSelection(selection))
                )
            ),
            Some((AxId(3), rustic_a11y::ActionRequest::SetSelection(3, 4)))
        );
        // Unsupported actions and unknown nodes do nothing.
        assert_eq!(
            page_action(&page, &request(NodeId(1), Action::Click, None)),
            None
        );
        assert_eq!(
            page_action(&page, &request(NodeId(99), Action::Focus, None)),
            None
        );
        assert_eq!(
            page_action(&page, &request(WINDOW, Action::Focus, None)),
            None
        );
    }
}

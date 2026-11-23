//! The renderer-to-browser encoding of an [`AxTree`].
//!
//! A renderer builds the tree next to the DOM and sends it to the broker,
//! which hands it to the platform bridge. The broker trusts none of it:
//! [`decode`] bounds every count and string before allocating, requires
//! each node's parent to come before it (so the result is a tree, never a
//! cycle), rejects duplicate ids, non-finite geometry and unknown roles,
//! and never panics.

use std::collections::{HashMap, HashSet};
use std::fmt;

use rustic_ipc::{DecodeError, Reader, Writer};

use crate::{AxId, AxNode, AxTree, Checked, Live, RangeValue, Rect, Role, States, TreeLimits};

const VERSION: u8 = 1;
const NO_PARENT: u32 = u32::MAX;

/// Why an encoded tree was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    /// A field did not decode.
    Decode(DecodeError),
    /// An encoding version this build does not read.
    Version(u8),
    /// The node count is zero or over the limit.
    NodeCount,
    /// A string is over the limit.
    TextTooLong,
    /// A parent reference does not point to an earlier node.
    BadParent,
    /// Two nodes share an id.
    DuplicateId,
    /// A role code this build does not know.
    UnknownRole(u16),
    /// Geometry or a range value is not finite.
    BadNumber,
    /// The tree is deeper than the limit.
    TooDeep,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => write!(f, "accessibility tree: {error}"),
            Self::Version(v) => write!(f, "accessibility tree: unknown version {v}"),
            Self::NodeCount => f.write_str("accessibility tree: node count out of range"),
            Self::TextTooLong => f.write_str("accessibility tree: text over the limit"),
            Self::BadParent => f.write_str("accessibility tree: parent does not precede child"),
            Self::DuplicateId => f.write_str("accessibility tree: duplicate node id"),
            Self::UnknownRole(code) => write!(f, "accessibility tree: unknown role {code}"),
            Self::BadNumber => f.write_str("accessibility tree: non-finite number"),
            Self::TooDeep => f.write_str("accessibility tree: too deep"),
        }
    }
}

impl std::error::Error for WireError {}

impl From<DecodeError> for WireError {
    fn from(error: DecodeError) -> Self {
        WireError::Decode(error)
    }
}

fn tri(value: Option<Checked>) -> u8 {
    match value {
        None => 0,
        Some(Checked::False) => 1,
        Some(Checked::True) => 2,
        Some(Checked::Mixed) => 3,
    }
}

fn untri(code: u8) -> Result<Option<Checked>, WireError> {
    Ok(match code {
        0 => None,
        1 => Some(Checked::False),
        2 => Some(Checked::True),
        3 => Some(Checked::Mixed),
        _ => return Err(DecodeError::InvalidFlag.into()),
    })
}

fn opt_bool(value: Option<bool>) -> u8 {
    match value {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }
}

fn unopt_bool(code: u8) -> Result<Option<bool>, WireError> {
    Ok(match code {
        0 => None,
        1 => Some(false),
        2 => Some(true),
        _ => return Err(DecodeError::InvalidFlag.into()),
    })
}

fn flags(s: &States) -> u32 {
    [
        s.focusable,
        s.focused,
        s.disabled,
        s.required,
        s.invalid,
        s.readonly,
        s.multiline,
        s.multiselectable,
        s.editable,
        s.modal,
        s.busy,
        s.protected,
    ]
    .iter()
    .enumerate()
    .fold(0, |bits, (i, &on)| bits | (u32::from(on) << i))
}

const FLAG_COUNT: u32 = 12;

fn opt_f64(writer: &mut Writer, value: Option<f64>) {
    match value {
        Some(v) => {
            writer.bool(true);
            writer.u64(v.to_bits());
        }
        None => writer.bool(false),
    }
}

/// Encode `tree`.
pub fn encode(tree: &AxTree) -> Vec<u8> {
    let index = tree.index();
    let mut parent = vec![NO_PARENT; tree.nodes.len()];
    for (i, node) in tree.nodes.iter().enumerate() {
        for child in &node.children {
            if let Some(&c) = index.get(child) {
                parent[c] = i as u32;
            }
        }
    }
    encode_with_parents(tree, &parent)
}

fn encode_with_parents(tree: &AxTree, parent: &[u32]) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(VERSION);
    w.u64(tree.focus.0);
    w.bool(tree.truncated);
    w.u32(tree.nodes.len() as u32);
    for (i, node) in tree.nodes.iter().enumerate() {
        w.u64(node.id.0);
        w.u16(node.role.code());
        w.u32(parent[i]);
        w.str(&node.name);
        w.str(&node.description);
        w.opt_str(node.value.as_deref());
        w.opt_str(node.placeholder.as_deref());
        w.opt_str(node.url.as_deref());
        w.u32(flags(&node.states));
        w.u8(tri(node.states.checked));
        w.u8(tri(node.states.pressed));
        w.u8(opt_bool(node.states.expanded));
        w.u8(opt_bool(node.states.selected));
        w.u32(node.level.unwrap_or(0));
        w.u8(match node.live {
            None => 0,
            Some(Live::Polite) => 1,
            Some(Live::Assertive) => 2,
        });
        match node.range {
            Some(range) => {
                w.bool(true);
                opt_f64(&mut w, range.min);
                opt_f64(&mut w, range.max);
                opt_f64(&mut w, range.now);
            }
            None => w.bool(false),
        }
        match node.bounds {
            Some(b) => {
                w.bool(true);
                for v in [b.x, b.y, b.width, b.height] {
                    w.u64(v.to_bits());
                }
            }
            None => w.bool(false),
        }
        match node.selection {
            Some((anchor, focus)) => {
                w.bool(true);
                w.u32(anchor);
                w.u32(focus);
            }
            None => w.bool(false),
        }
    }
    w.into_bytes()
}

fn finite(reader: &mut Reader<'_>) -> Result<f64, WireError> {
    let value = f64::from_bits(reader.u64()?);
    if value.is_finite() {
        Ok(value)
    } else {
        Err(WireError::BadNumber)
    }
}

fn read_opt_f64(reader: &mut Reader<'_>) -> Result<Option<f64>, WireError> {
    if reader.bool()? {
        finite(reader).map(Some)
    } else {
        Ok(None)
    }
}

/// Decode an untrusted encoded tree within `limits`.
pub fn decode(bytes: &[u8], limits: TreeLimits) -> Result<AxTree, WireError> {
    let mut r = Reader::new(bytes);
    let version = r.u8()?;
    if version != VERSION {
        return Err(WireError::Version(version));
    }
    let focus = AxId(r.u64()?);
    let truncated = r.bool()?;
    let count = r.u32()? as usize;
    if count == 0 || count > limits.max_nodes {
        return Err(WireError::NodeCount);
    }
    // Each node takes well over 32 bytes; refuse counts the input cannot
    // hold before reserving for them.
    if count > bytes.len() / 32 {
        return Err(DecodeError::Truncated.into());
    }
    let mut nodes: Vec<AxNode> = Vec::with_capacity(count);
    let mut depth: Vec<usize> = Vec::with_capacity(count);
    let mut parents: Vec<u32> = Vec::with_capacity(count);
    let mut seen = HashSet::with_capacity(count);
    let text = |r: &mut Reader<'_>| -> Result<String, WireError> {
        let s = r.string()?;
        if s.len() > limits.max_text_bytes {
            return Err(WireError::TextTooLong);
        }
        Ok(s)
    };
    let opt_text = |r: &mut Reader<'_>| -> Result<Option<String>, WireError> {
        match r.opt_string()? {
            Some(s) if s.len() > limits.max_text_bytes => Err(WireError::TextTooLong),
            other => Ok(other),
        }
    };
    for i in 0..count {
        let id = AxId(r.u64()?);
        if !seen.insert(id) {
            return Err(WireError::DuplicateId);
        }
        let code = r.u16()?;
        let role = Role::from_code(code).ok_or(WireError::UnknownRole(code))?;
        let parent = r.u32()?;
        let level = if i == 0 {
            if parent != NO_PARENT {
                return Err(WireError::BadParent);
            }
            0
        } else {
            let p = parent as usize;
            if parent == NO_PARENT || p >= i {
                return Err(WireError::BadParent);
            }
            depth[p] + 1
        };
        if level > limits.max_depth {
            return Err(WireError::TooDeep);
        }
        let mut node = AxNode::new(id, role);
        node.name = text(&mut r)?;
        node.description = text(&mut r)?;
        node.value = opt_text(&mut r)?;
        node.placeholder = opt_text(&mut r)?;
        node.url = opt_text(&mut r)?;
        let bits = r.u32()?;
        if bits >> FLAG_COUNT != 0 {
            return Err(DecodeError::InvalidFlag.into());
        }
        let on = |n: u32| bits & (1 << n) != 0;
        node.states = States {
            focusable: on(0),
            focused: on(1),
            disabled: on(2),
            required: on(3),
            invalid: on(4),
            readonly: on(5),
            multiline: on(6),
            multiselectable: on(7),
            editable: on(8),
            modal: on(9),
            busy: on(10),
            protected: on(11),
            checked: untri(r.u8()?)?,
            pressed: untri(r.u8()?)?,
            expanded: unopt_bool(r.u8()?)?,
            selected: unopt_bool(r.u8()?)?,
        };
        node.level = Some(r.u32()?).filter(|&l| l != 0);
        node.live = match r.u8()? {
            0 => None,
            1 => Some(Live::Polite),
            2 => Some(Live::Assertive),
            _ => return Err(DecodeError::InvalidFlag.into()),
        };
        if r.bool()? {
            node.range = Some(RangeValue {
                min: read_opt_f64(&mut r)?,
                max: read_opt_f64(&mut r)?,
                now: read_opt_f64(&mut r)?,
            });
        }
        if r.bool()? {
            let rect = Rect {
                x: finite(&mut r)?,
                y: finite(&mut r)?,
                width: finite(&mut r)?,
                height: finite(&mut r)?,
            };
            if !rect.is_valid() {
                return Err(WireError::BadNumber);
            }
            node.bounds = Some(rect);
        }
        if r.bool()? {
            node.selection = Some((r.u32()?, r.u32()?));
        }
        nodes.push(node);
        depth.push(level);
        parents.push(parent);
    }
    r.finish()?;
    let ids: Vec<AxId> = nodes.iter().map(|n| n.id).collect();
    for (i, &parent) in parents.iter().enumerate().skip(1) {
        nodes[parent as usize].children.push(ids[i]);
    }
    // A preorder sequence has each subtree contiguous; a parent list that
    // is not preorder still forms a tree, so reorder into preorder.
    let nodes = preorder(nodes);
    let focus = if seen.contains(&focus) {
        focus
    } else {
        nodes[0].id
    };
    Ok(AxTree {
        nodes,
        focus,
        truncated,
    })
}

fn preorder(nodes: Vec<AxNode>) -> Vec<AxNode> {
    let index: HashMap<AxId, usize> = nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
    let mut order = Vec::with_capacity(nodes.len());
    let mut stack = vec![0usize];
    while let Some(i) = stack.pop() {
        order.push(i);
        for child in nodes[i].children.iter().rev() {
            stack.push(index[child]);
        }
    }
    let mut slots: Vec<Option<AxNode>> = nodes.into_iter().map(Some).collect();
    order.into_iter().filter_map(|i| slots[i].take()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AxTree {
        let mut root = AxNode::new(AxId(1), Role::Document);
        root.name = "Title".into();
        root.children = vec![AxId(2), AxId(4)];
        let mut button = AxNode::new(AxId(2), Role::Button);
        button.name = "Go".into();
        button.states.focusable = true;
        button.states.focused = true;
        button.bounds = Some(Rect {
            x: 1.0,
            y: 2.0,
            width: 30.0,
            height: 20.0,
        });
        button.children = vec![AxId(3)];
        let mut text = AxNode::new(AxId(3), Role::StaticText);
        text.name = "Go".into();
        let mut field = AxNode::new(AxId(4), Role::TextInput);
        field.value = Some("abc".into());
        field.states.editable = true;
        field.states.checked = None;
        field.selection = Some((1, 3));
        field.range = Some(RangeValue {
            min: Some(0.0),
            max: None,
            now: Some(0.5),
        });
        AxTree {
            nodes: vec![root, button, text, field],
            focus: AxId(2),
            truncated: false,
        }
    }

    #[test]
    fn round_trips() {
        let tree = sample();
        let decoded = decode(&encode(&tree), TreeLimits::default()).unwrap();
        assert_eq!(decoded, tree);
    }

    #[test]
    fn refuses_hostile_input_without_panicking() {
        let bytes = encode(&sample());
        // Every truncation fails cleanly.
        for len in 0..bytes.len() {
            assert!(decode(&bytes[..len], TreeLimits::default()).is_err());
        }
        // Every single-byte corruption either decodes to a valid tree or
        // fails; none panics.
        for i in 0..bytes.len() {
            for flip in [0x01u8, 0x80, 0xff] {
                let mut copy = bytes.clone();
                copy[i] ^= flip;
                if let Ok(tree) = decode(&copy, TreeLimits::default()) {
                    assert!(tree.nodes[0].children.len() < tree.nodes.len());
                }
            }
        }
        let mut extra = bytes.clone();
        extra.push(0);
        assert_eq!(
            decode(&extra, TreeLimits::default()),
            Err(WireError::Decode(DecodeError::TrailingBytes))
        );
        let small = TreeLimits {
            max_nodes: 3,
            ..TreeLimits::default()
        };
        assert_eq!(decode(&bytes, small), Err(WireError::NodeCount));
        let short = TreeLimits {
            max_text_bytes: 2,
            ..TreeLimits::default()
        };
        assert_eq!(decode(&bytes, short), Err(WireError::TextTooLong));
    }

    #[test]
    fn parents_must_precede_children() {
        let tree = sample();
        for forged in [
            [NO_PARENT, 2, 1, 0],
            [NO_PARENT, 0, 3, 0],
            [0, 0, 1, 0],
            [NO_PARENT, NO_PARENT, 1, 0],
        ] {
            assert_eq!(
                decode(&encode_with_parents(&tree, &forged), TreeLimits::default()),
                Err(WireError::BadParent),
                "{forged:?}"
            );
        }
        let shallow = TreeLimits {
            max_depth: 1,
            ..TreeLimits::default()
        };
        assert_eq!(decode(&encode(&tree), shallow), Err(WireError::TooDeep));
    }
}

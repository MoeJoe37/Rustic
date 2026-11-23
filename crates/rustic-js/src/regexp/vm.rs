//! The matcher: patterns compiled to a small instruction list and run by
//! a backtracking machine with explicit stacks, so neither the pattern
//! nor the input can exhaust the Rust stack, and every step is counted.
//! Patterns without backreferences or lookarounds also get a second
//! program for the linear-time matcher in `pike.rs`, which takes over
//! when a search backtracks too much (TD-111).

use super::case::{canonicalize, close};
use super::class::CharSet;
use super::parse::{MAX_COUNT, Node};
use super::{ExecError, Flags};

/// A capture position or register that is unset (a group that did not
/// participate).
pub const UNSET: u32 = u32::MAX;

/// Backtracking entries (choices and undo records) a match may hold.
const MAX_BACKTRACK: usize = 1 << 22;

/// Matcher steps between checks of the cancellation hook.
const POLL_INTERVAL: u32 = 1 << 14;

#[derive(Clone, Copy, Debug)]
pub(super) enum Inst {
    /// One character equal to `c`.
    Char {
        c: u32,
        back: bool,
    },
    /// One character in `sets[set]`.
    Set {
        set: u32,
        back: bool,
    },
    /// `.` without `s`: anything but a line terminator.
    Dot {
        back: bool,
    },
    /// Any character.
    AnyChar {
        back: bool,
    },
    Start {
        multiline: bool,
    },
    End {
        multiline: bool,
    },
    WordBoundary {
        negated: bool,
        unicode_ignore_case: bool,
    },
    /// Try `next` first; on failure go to `alt`.
    Split {
        next: u32,
        alt: u32,
    },
    Jump {
        to: u32,
    },
    /// `regs[reg] = position`
    Save {
        reg: u32,
    },
    /// The text of the first participating group of `refs[list]`.
    BackRef {
        list: u32,
        ignore_case: bool,
        back: bool,
    },
    /// A lookaround whose body runs from the next instruction to
    /// `end` (a `Match`).
    Look {
        negative: bool,
        end: u32,
    },
    /// Start a counted loop: counter `reg` = 0.
    LoopInit {
        reg: u32,
    },
    /// The top of a counted loop: leave for `exit` or go on to the
    /// `LoopEnter` after it, in the order `greedy` asks for.
    LoopHead {
        reg: u32,
        min: u32,
        max: u32,
        greedy: bool,
        exit: u32,
    },
    /// Start an iteration: note the position, clear the captures inside.
    LoopEnter {
        reg: u32,
        clear: (u32, u32),
    },
    /// The bottom: reject an empty iteration once `min` is met, count it,
    /// go back to `head`.
    LoopTail {
        reg: u32,
        min: u32,
        head: u32,
    },
    /// A greedy or lazy run of one-character `atom`s (the next
    /// instruction), `min..=max` of them, without captures inside.
    Star {
        min: u32,
        max: u32,
        greedy: bool,
        /// The instruction after the atom.
        exit: u32,
    },
    Match,
    /// Linear-time programs only: `regs[from..to] = UNSET` (the
    /// captures inside a repetition, at the start of each iteration).
    Clear {
        from: u32,
        to: u32,
    },
    /// Linear-time programs only: fail if the iteration that started
    /// when `regs[reg]` was saved matched the empty string.
    Progress {
        reg: u32,
    },
}

/// A compiled pattern.
#[derive(Debug)]
pub(crate) struct Program {
    pub(super) insts: Vec<Inst>,
    pub(super) sets: Vec<CharSet>,
    refs: Vec<Vec<u32>>,
    /// Registers: two per capture (group 0 first), then loop state.
    pub registers: u32,
    pub(super) unicode: bool,
}

/// Instructions a linear-time program may have: repetitions are unrolled
/// there, so a large counted repetition is left to the backtracker alone.
const PIKE_MAX_INSTS: usize = 1 << 15;

/// Thread state a linear-time program may need (instructions times
/// registers, in `u32`s per thread list).
const PIKE_MAX_STATE: usize = 1 << 21;

struct Compiler {
    insts: Vec<Inst>,
    sets: Vec<CharSet>,
    refs: Vec<Vec<u32>>,
    next_reg: u32,
    unicode: bool,
    /// Compiling for the linear-time matcher: repetitions unrolled.
    pike: bool,
    /// Repetitions open around the current node (linear-time programs).
    depth: u32,
    max_depth: u32,
    /// The linear-time program grew past [`PIKE_MAX_INSTS`].
    too_big: bool,
}

impl Compiler {
    fn new(groups: u32, flags: Flags, pike: bool) -> Compiler {
        Compiler {
            insts: Vec::new(),
            sets: Vec::new(),
            refs: Vec::new(),
            next_reg: 2 * (groups + 1),
            unicode: flags.unicode || flags.sets,
            pike,
            depth: 0,
            max_depth: 0,
            too_big: false,
        }
    }

    fn finish(mut self, node: &Node, flags: Flags) -> Program {
        self.emit(Inst::Save { reg: 0 });
        self.node(node, flags, false);
        self.emit(Inst::Save { reg: 1 });
        self.emit(Inst::Match);
        Program {
            insts: self.insts,
            sets: self.sets,
            refs: self.refs,
            registers: self.next_reg + self.max_depth,
            unicode: self.unicode,
        }
    }
}

/// Compile `node` (with `groups` capturing groups) under `flags`.
pub(crate) fn compile(node: &Node, groups: u32, flags: Flags) -> Program {
    Compiler::new(groups, flags, false).finish(node, flags)
}

/// Compile `node` for the linear-time matcher: `None` when it has a
/// backreference or a lookaround, which that matcher cannot run, or
/// unrolls past the size limits.
pub(crate) fn compile_pike(node: &Node, groups: u32, flags: Flags) -> Option<Program> {
    if !linear(node) {
        return None;
    }
    let mut c = Compiler::new(groups, flags, true);
    c.emit(Inst::Save { reg: 0 });
    c.node(node, flags, false);
    c.emit(Inst::Save { reg: 1 });
    c.emit(Inst::Match);
    let registers = c.next_reg + c.max_depth;
    if c.too_big
        || c.insts.len() > PIKE_MAX_INSTS
        || c.insts.len() * registers as usize > PIKE_MAX_STATE
    {
        return None;
    }
    Some(Program {
        insts: c.insts,
        sets: c.sets,
        refs: c.refs,
        registers,
        unicode: c.unicode,
    })
}

/// Whether `node` has no backreference or lookaround.
fn linear(node: &Node) -> bool {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        match node {
            Node::Look { .. } | Node::BackRef(_) | Node::NamedRef(_) => return false,
            Node::Capture(inner, _)
            | Node::Repeat { node: inner, .. }
            | Node::Modified { node: inner, .. } => stack.push(inner),
            Node::Concat(nodes) | Node::Alt(nodes) => stack.extend(nodes),
            Node::Empty
            | Node::Char(_)
            | Node::Any
            | Node::Class(_)
            | Node::StringClass(..)
            | Node::Start
            | Node::End
            | Node::WordBoundary(_) => {}
        }
    }
    true
}

impl Compiler {
    fn emit(&mut self, inst: Inst) -> u32 {
        self.insts.push(inst);
        (self.insts.len() - 1) as u32
    }

    fn here(&self) -> u32 {
        self.insts.len() as u32
    }

    fn set(&mut self, set: CharSet, flags: Flags) -> u32 {
        let set = if flags.ignore_case {
            close(&set, self.unicode)
        } else {
            set
        };
        self.sets.push(set);
        (self.sets.len() - 1) as u32
    }

    /// One character `c` as an instruction.
    fn char(&mut self, c: u32, flags: Flags, back: bool) {
        if flags.ignore_case {
            let set = close(&CharSet::single(c), self.unicode);
            if set.ranges() != [(c, c)] {
                let set = self.set(set, Flags::default());
                self.emit(Inst::Set { set, back });
                return;
            }
        }
        self.emit(Inst::Char { c, back });
    }

    /// Whether `node` always consumes exactly one character and holds no
    /// captures, so a loop over it can be a [`Inst::Star`].
    fn single_char(node: &Node) -> bool {
        matches!(node, Node::Char(_) | Node::Any | Node::Class(_))
    }

    fn node(&mut self, node: &Node, flags: Flags, back: bool) {
        match node {
            Node::Empty => {}
            Node::Char(c) => self.char(*c, flags, back),
            Node::Any => {
                self.emit(if flags.dot_all {
                    Inst::AnyChar { back }
                } else {
                    Inst::Dot { back }
                });
            }
            Node::Class(set) => {
                let set = self.set((**set).clone(), flags);
                self.emit(Inst::Set { set, back });
            }
            Node::StringClass(strings, set) => {
                // The strings, longest first, then the single characters.
                let mut alternatives: Vec<Node> = strings
                    .iter()
                    .map(|s| Node::Concat(s.iter().map(|&c| Node::Char(c)).collect()))
                    .collect();
                if !set.is_empty() {
                    alternatives.push(Node::Class(set.clone()));
                }
                if alternatives.is_empty() {
                    // Matches nothing.
                    let set = self.set(CharSet::default(), flags);
                    self.emit(Inst::Set { set, back });
                } else {
                    self.node(&Node::Alt(alternatives), flags, back);
                }
            }
            Node::Start => {
                self.emit(Inst::Start {
                    multiline: flags.multiline,
                });
            }
            Node::End => {
                self.emit(Inst::End {
                    multiline: flags.multiline,
                });
            }
            Node::WordBoundary(negated) => {
                self.emit(Inst::WordBoundary {
                    negated: *negated,
                    unicode_ignore_case: self.unicode && flags.ignore_case,
                });
            }
            Node::Capture(inner, index) => {
                let (start, end) = (2 * index, 2 * index + 1);
                // Matching backwards, the end is found first.
                let (first, second) = if back { (end, start) } else { (start, end) };
                self.emit(Inst::Save { reg: first });
                self.node(inner, flags, back);
                self.emit(Inst::Save { reg: second });
            }
            Node::Look {
                node,
                behind,
                negative,
            } => {
                let look = self.emit(Inst::Look {
                    negative: *negative,
                    end: 0,
                });
                self.node(node, flags, *behind);
                let end = self.emit(Inst::Match);
                self.insts[look as usize] = Inst::Look {
                    negative: *negative,
                    end,
                };
            }
            Node::BackRef(groups) => {
                self.refs.push(groups.clone());
                let list = (self.refs.len() - 1) as u32;
                self.emit(Inst::BackRef {
                    list,
                    ignore_case: flags.ignore_case,
                    back,
                });
            }
            Node::Concat(nodes) => {
                if back {
                    for n in nodes.iter().rev() {
                        self.node(n, flags, back);
                    }
                } else {
                    for n in nodes {
                        self.node(n, flags, back);
                    }
                }
            }
            Node::Alt(alternatives) => {
                let mut jumps = Vec::new();
                for (i, alternative) in alternatives.iter().enumerate() {
                    if i + 1 < alternatives.len() {
                        let split = self.emit(Inst::Split { next: 0, alt: 0 });
                        self.node(alternative, flags, back);
                        jumps.push(self.emit(Inst::Jump { to: 0 }));
                        let alt = self.here();
                        self.insts[split as usize] = Inst::Split {
                            next: split + 1,
                            alt,
                        };
                    } else {
                        self.node(alternative, flags, back);
                    }
                }
                let end = self.here();
                for j in jumps {
                    self.insts[j as usize] = Inst::Jump { to: end };
                }
            }
            Node::Repeat { .. } => self.repeat(node, flags, back),
            Node::Modified { node, add, remove } => {
                let mut inner = flags;
                inner.ignore_case = (flags.ignore_case || add.ignore_case) && !remove.ignore_case;
                inner.multiline = (flags.multiline || add.multiline) && !remove.multiline;
                inner.dot_all = (flags.dot_all || add.dot_all) && !remove.dot_all;
                self.node(node, inner, back);
            }
            Node::NamedRef(_) => unreachable!("names are resolved by the parser"),
        }
    }

    fn repeat(&mut self, repeat: &Node, flags: Flags, back: bool) {
        let Node::Repeat {
            node,
            min,
            max,
            greedy,
            captures,
        } = repeat
        else {
            unreachable!("a repetition");
        };
        let (node, min, max, greedy, captures) = (&**node, *min, *max, *greedy, *captures);
        if max == 0 {
            return;
        }
        if self.pike {
            self.unroll(node, min, max, greedy, captures, flags);
            return;
        }
        if Self::single_char(node) {
            let star = self.emit(Inst::Star {
                min,
                max,
                greedy,
                exit: 0,
            });
            self.node(node, flags, back);
            let exit = self.here();
            self.insts[star as usize] = Inst::Star {
                min,
                max,
                greedy,
                exit,
            };
            return;
        }
        let reg = self.next_reg;
        // The counter and the position at the start of the iteration.
        self.next_reg += 2;
        self.emit(Inst::LoopInit { reg });
        let head = self.emit(Inst::LoopHead {
            reg,
            min,
            max,
            greedy,
            exit: 0,
        });
        self.emit(Inst::LoopEnter {
            reg,
            clear: (2 * captures.0, 2 * captures.1),
        });
        self.node(node, flags, back);
        self.emit(Inst::LoopTail { reg, min, head });
        let exit = self.here();
        self.insts[head as usize] = Inst::LoopHead {
            reg,
            min,
            max,
            greedy,
            exit,
        };
    }
}

impl Compiler {
    /// A repetition for the linear-time matcher: `min` copies, then
    /// `max - min` optional copies or one loop. An optional iteration
    /// that matches the empty string fails, as the backtracker's
    /// `LoopTail` does.
    fn unroll(
        &mut self,
        node: &Node,
        min: u32,
        max: u32,
        greedy: bool,
        captures: (u32, u32),
        flags: Flags,
    ) {
        let optional = if max == MAX_COUNT { 1 } else { max - min };
        if min as usize > PIKE_MAX_INSTS || optional as usize > PIKE_MAX_INSTS {
            self.too_big = true;
        }
        if self.too_big {
            return;
        }
        let mark = self.next_reg + self.depth;
        self.depth += 1;
        self.max_depth = self.max_depth.max(self.depth);
        let clear = (2 * captures.0, 2 * captures.1);
        for _ in 0..min {
            if clear.0 < clear.1 {
                self.emit(Inst::Clear {
                    from: clear.0,
                    to: clear.1,
                });
            }
            self.node(node, flags, false);
            if self.insts.len() > PIKE_MAX_INSTS {
                self.too_big = true;
                break;
            }
        }
        let mut splits = Vec::new();
        if !self.too_big {
            for _ in 0..optional {
                splits.push(self.emit(Inst::Split { next: 0, alt: 0 }));
                self.emit(Inst::Save { reg: mark });
                if clear.0 < clear.1 {
                    self.emit(Inst::Clear {
                        from: clear.0,
                        to: clear.1,
                    });
                }
                self.node(node, flags, false);
                self.emit(Inst::Progress { reg: mark });
                if self.insts.len() > PIKE_MAX_INSTS {
                    self.too_big = true;
                    break;
                }
            }
        }
        if let (MAX_COUNT, Some(&split)) = (max, splits.first()) {
            self.emit(Inst::Jump { to: split });
        }
        let exit = self.here();
        for split in splits {
            let body = split + 1;
            self.insts[split as usize] = if greedy {
                Inst::Split {
                    next: body,
                    alt: exit,
                }
            } else {
                Inst::Split {
                    next: exit,
                    alt: body,
                }
            };
        }
        self.depth -= 1;
    }
}

/// Text to match: code units, stored as bytes (Latin-1) or UTF-16.
pub(crate) trait Input {
    fn len(&self) -> usize;
    fn unit(&self, i: usize) -> u16;
}

impl Input for [u8] {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }
    fn unit(&self, i: usize) -> u16 {
        u16::from(self[i])
    }
}

impl Input for [u16] {
    fn len(&self) -> usize {
        <[u16]>::len(self)
    }
    fn unit(&self, i: usize) -> u16 {
        self[i]
    }
}

/// A backtracking entry: resume at `pc` and `pos` after undoing the
/// trail down to `trail`; or, for a [`Inst::Star`], try the run one
/// shorter (greedy) or longer (lazy).
#[derive(Clone, Copy)]
enum Choice {
    At {
        pc: u32,
        pos: u32,
        trail: u32,
    },
    Star {
        /// The `Star` instruction.
        pc: u32,
        pos: u32,
        count: u32,
        trail: u32,
    },
    /// The floor of a lookaround's own backtracking.
    Barrier,
}

/// Backtracks a search may take before it moves to the linear-time
/// matcher; `exhausted` once it ran out, or the backtracking stacks
/// reached [`MAX_BACKTRACK`].
pub(crate) struct Budget {
    pub left: u64,
    pub exhausted: bool,
}

struct Machine<'a, I: Input + ?Sized> {
    program: &'a Program,
    input: &'a I,
    regs: Vec<u32>,
    trail: Vec<(u32, u32)>,
    choices: Vec<Choice>,
    countdown: u32,
    poll: &'a mut dyn FnMut() -> bool,
    budget: &'a mut Budget,
}

impl Program {
    /// Bytes the program holds.
    pub(crate) fn footprint(&self) -> usize {
        std::mem::size_of::<Inst>() * self.insts.len()
            + self
                .sets
                .iter()
                .map(|s| 8 * s.ranges().len() + 24)
                .sum::<usize>()
            + self.refs.iter().map(|r| 4 * r.len() + 24).sum::<usize>()
    }

    /// Match at exactly `pos`: the registers (captures first) on success.
    /// Running out of `budget` ends the match with
    /// [`ExecError::TooComplex`] and marks the budget exhausted.
    pub(crate) fn match_at<I: Input + ?Sized>(
        &self,
        input: &I,
        pos: usize,
        poll: &mut dyn FnMut() -> bool,
        budget: &mut Budget,
    ) -> Result<Option<Vec<u32>>, ExecError> {
        let mut m = Machine {
            program: self,
            input,
            regs: vec![UNSET; self.registers as usize],
            trail: Vec::new(),
            choices: Vec::new(),
            countdown: POLL_INTERVAL,
            poll,
            budget,
        };
        if m.run(0, pos as u32)?.is_some() {
            Ok(Some(m.regs))
        } else {
            Ok(None)
        }
    }
}

fn is_line_terminator(c: u32) -> bool {
    matches!(c, 0x0A | 0x0D | 0x2028 | 0x2029)
}

fn is_word(c: u32, unicode_ignore_case: bool) -> bool {
    matches!(c, 0x30..=0x39 | 0x41..=0x5A | 0x5F | 0x61..=0x7A)
        || (unicode_ignore_case && (c == 0x17F || c == 0x212A))
}

impl<I: Input + ?Sized> Machine<'_, I> {
    fn set_reg(&mut self, reg: u32, value: u32) {
        let old = self.regs[reg as usize];
        if old != value {
            self.trail.push((reg, old));
            self.regs[reg as usize] = value;
        }
    }

    fn undo(&mut self, to: u32) {
        while self.trail.len() > to as usize {
            let (reg, old) = self.trail.pop().expect("checked length");
            self.regs[reg as usize] = old;
        }
    }

    fn push(&mut self, choice: Choice) -> Result<(), ExecError> {
        if self.choices.len() + self.trail.len() >= MAX_BACKTRACK {
            self.budget.exhausted = true;
            return Err(ExecError::TooComplex);
        }
        self.choices.push(choice);
        Ok(())
    }

    fn tick(&mut self) -> Result<(), ExecError> {
        self.countdown -= 1;
        if self.countdown == 0 {
            self.countdown = POLL_INTERVAL;
            if (self.poll)() {
                return Err(ExecError::Cancelled);
            }
        }
        Ok(())
    }

    fn read(&self, pos: u32, back: bool) -> Option<(u32, u32)> {
        read(self.program, self.input, pos, back)
    }

    fn step_pos(pos: u32, width: u32, back: bool) -> u32 {
        if back { pos - width } else { pos + width }
    }

    fn single(&self, inst: Inst, pos: u32) -> Option<u32> {
        single(self.program, self.input, inst, pos)
    }

    fn back_reference(&self, list: u32, ignore_case: bool, back: bool, pos: u32) -> Option<u32> {
        let groups = &self.program.refs[list as usize];
        let Some((start, end)) = groups.iter().find_map(|&g| {
            let (s, e) = (self.regs[2 * g as usize], self.regs[2 * g as usize + 1]);
            (s != UNSET && e != UNSET).then_some((s, e))
        }) else {
            return Some(pos);
        };
        let n = end - start;
        let (from, to) = if back {
            (pos.checked_sub(n)?, pos)
        } else {
            let to = pos.checked_add(n)?;
            if to as usize > self.input.len() {
                return None;
            }
            (pos, to)
        };
        for i in 0..n {
            let a = u32::from(self.input.unit((start + i) as usize));
            let b = u32::from(self.input.unit((from + i) as usize));
            if a != b
                && !(ignore_case
                    && canonicalize(a, self.program.unicode)
                        == canonicalize(b, self.program.unicode))
            {
                return None;
            }
        }
        Some(if back { from } else { to })
    }

    /// Run from `pc` at `pos` until a `Match` (the position there) or
    /// failure with no choice left above the floor on entry.
    fn run(&mut self, mut pc: u32, mut pos: u32) -> Result<Option<u32>, ExecError> {
        let floor = self.choices.len();
        self.push(Choice::Barrier)?;
        loop {
            self.tick()?;
            let inst = self.program.insts[pc as usize];
            let ok = match inst {
                Inst::Char { .. } | Inst::Set { .. } | Inst::Dot { .. } | Inst::AnyChar { .. } => {
                    match self.single(inst, pos) {
                        Some(next) => {
                            pos = next;
                            pc += 1;
                            true
                        }
                        None => false,
                    }
                }
                Inst::Start { .. } | Inst::End { .. } | Inst::WordBoundary { .. } => {
                    pc += 1;
                    assertion(self.input, inst, pos)
                }
                Inst::Clear { from, to } => {
                    for r in from..to {
                        self.set_reg(r, UNSET);
                    }
                    pc += 1;
                    true
                }
                Inst::Progress { reg } => {
                    pc += 1;
                    self.regs[reg as usize] != pos
                }
                Inst::Split { next, alt } => {
                    let trail = self.trail.len() as u32;
                    self.push(Choice::At {
                        pc: alt,
                        pos,
                        trail,
                    })?;
                    pc = next;
                    true
                }
                Inst::Jump { to } => {
                    pc = to;
                    true
                }
                Inst::Save { reg } => {
                    self.set_reg(reg, pos);
                    pc += 1;
                    true
                }
                Inst::BackRef {
                    list,
                    ignore_case,
                    back,
                } => match self.back_reference(list, ignore_case, back, pos) {
                    Some(next) => {
                        pos = next;
                        pc += 1;
                        true
                    }
                    None => false,
                },
                Inst::Look { negative, end } => {
                    let trail = self.trail.len() as u32;
                    let matched = self.run(pc + 1, pos)?.is_some();
                    if negative {
                        // Captures set inside a negative lookaround never
                        // survive it.
                        self.undo(trail);
                    }
                    pc = end + 1;
                    matched != negative
                }
                Inst::LoopInit { reg } => {
                    self.set_reg(reg, 0);
                    pc += 1;
                    true
                }
                Inst::LoopHead {
                    reg,
                    min,
                    max,
                    greedy,
                    exit,
                } => {
                    let count = self.regs[reg as usize];
                    let trail = self.trail.len() as u32;
                    if count >= max {
                        pc = exit;
                    } else if count < min {
                        pc += 1;
                    } else if greedy {
                        self.push(Choice::At {
                            pc: exit,
                            pos,
                            trail,
                        })?;
                        pc += 1;
                    } else {
                        self.push(Choice::At {
                            pc: pc + 1,
                            pos,
                            trail,
                        })?;
                        pc = exit;
                    }
                    true
                }
                Inst::LoopEnter { reg, clear } => {
                    self.set_reg(reg + 1, pos);
                    for r in clear.0..clear.1 {
                        self.set_reg(r, UNSET);
                    }
                    pc += 1;
                    true
                }
                Inst::LoopTail { reg, min, head } => {
                    let count = self.regs[reg as usize];
                    if count >= min && self.regs[reg as usize + 1] == pos {
                        // An empty iteration past the minimum fails.
                        false
                    } else {
                        self.set_reg(reg, count.saturating_add(1).min(MAX_COUNT - 1));
                        pc = head;
                        true
                    }
                }
                Inst::Star {
                    min,
                    max,
                    greedy,
                    exit,
                } => {
                    let atom = self.program.insts[pc as usize + 1];
                    let mut count = 0;
                    let mut at = pos;
                    // The minimum is required either way.
                    let mut ok = true;
                    while count < min {
                        match self.single(atom, at) {
                            Some(next) => {
                                at = next;
                                count += 1;
                            }
                            None => {
                                ok = false;
                                break;
                            }
                        }
                        self.tick()?;
                    }
                    if ok {
                        if greedy {
                            let start = count;
                            let mut positions_back = at;
                            while count < max {
                                match self.single(atom, positions_back) {
                                    Some(next) => {
                                        positions_back = next;
                                        count += 1;
                                    }
                                    None => break,
                                }
                                self.tick()?;
                            }
                            if count > start {
                                let trail = self.trail.len() as u32;
                                self.push(Choice::Star {
                                    pc,
                                    pos: positions_back,
                                    count: count - start,
                                    trail,
                                })?;
                            }
                            pos = positions_back;
                        } else {
                            if count < max {
                                let trail = self.trail.len() as u32;
                                self.push(Choice::Star {
                                    pc,
                                    pos: at,
                                    count: max - count,
                                    trail,
                                })?;
                            }
                            pos = at;
                        }
                        pc = exit;
                    }
                    ok
                }
                Inst::Match => {
                    // Drop this run's choices: a lookaround is atomic, and
                    // the whole match is done.
                    self.choices.truncate(floor);
                    return Ok(Some(pos));
                }
            };
            if ok {
                continue;
            }
            // Backtrack.
            loop {
                let choice = self.choices.pop();
                if matches!(choice, Some(Choice::At { .. } | Choice::Star { .. })) {
                    if self.budget.left == 0 {
                        self.budget.exhausted = true;
                        return Err(ExecError::TooComplex);
                    }
                    self.budget.left -= 1;
                }
                match choice {
                    None | Some(Choice::Barrier) => {
                        debug_assert_eq!(self.choices.len(), floor);
                        return Ok(None);
                    }
                    Some(Choice::At {
                        pc: to,
                        pos: at,
                        trail,
                    }) => {
                        self.undo(trail);
                        pc = to;
                        pos = at;
                        break;
                    }
                    Some(Choice::Star {
                        pc: star,
                        pos: at,
                        count,
                        trail,
                    }) => {
                        self.undo(trail);
                        let Inst::Star { greedy, exit, .. } = self.program.insts[star as usize]
                        else {
                            unreachable!("star choices point at stars");
                        };
                        let atom = self.program.insts[star as usize + 1];
                        if greedy {
                            // Give back one character.
                            let back = atom_direction(atom);
                            let Some((_, width)) = self.read(at, !back) else {
                                continue;
                            };
                            let shorter = Self::step_pos(at, width, !back);
                            if count > 1 {
                                self.push(Choice::Star {
                                    pc: star,
                                    pos: shorter,
                                    count: count - 1,
                                    trail,
                                })?;
                            }
                            pc = exit;
                            pos = shorter;
                        } else {
                            // Take one more character.
                            let Some(longer) = self.single(atom, at) else {
                                continue;
                            };
                            if count > 1 {
                                self.push(Choice::Star {
                                    pc: star,
                                    pos: longer,
                                    count: count - 1,
                                    trail,
                                })?;
                            }
                            pc = exit;
                            pos = longer;
                        }
                        break;
                    }
                }
            }
        }
    }
}

/// The character after (`back` false) or before `pos`, and its width.
pub(super) fn read<I: Input + ?Sized>(
    program: &Program,
    input: &I,
    pos: u32,
    back: bool,
) -> Option<(u32, u32)> {
    let pos = pos as usize;
    let len = input.len();
    if back {
        if pos == 0 {
            return None;
        }
        let low = u32::from(input.unit(pos - 1));
        if program.unicode && (0xDC00..0xE000).contains(&low) && pos >= 2 {
            let high = u32::from(input.unit(pos - 2));
            if (0xD800..0xDC00).contains(&high) {
                return Some((0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00), 2));
            }
        }
        Some((low, 1))
    } else {
        if pos >= len {
            return None;
        }
        let high = u32::from(input.unit(pos));
        if program.unicode && (0xD800..0xDC00).contains(&high) && pos + 1 < len {
            let low = u32::from(input.unit(pos + 1));
            if (0xDC00..0xE000).contains(&low) {
                return Some((0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00), 2));
            }
        }
        Some((high, 1))
    }
}

/// Whether a one-character instruction matches at `pos`; the new
/// position if so.
pub(super) fn single<I: Input + ?Sized>(
    program: &Program,
    input: &I,
    inst: Inst,
    pos: u32,
) -> Option<u32> {
    let (ok, back, read) = match inst {
        Inst::Char { c, back } => {
            let r = read(program, input, pos, back)?;
            (r.0 == c, back, r)
        }
        Inst::Set { set, back } => {
            let r = read(program, input, pos, back)?;
            (program.sets[set as usize].contains(r.0), back, r)
        }
        Inst::Dot { back } => {
            let r = read(program, input, pos, back)?;
            (!is_line_terminator(r.0), back, r)
        }
        Inst::AnyChar { back } => {
            let r = read(program, input, pos, back)?;
            (true, back, r)
        }
        _ => unreachable!("not a one-character instruction"),
    };
    ok.then(|| if back { pos - read.1 } else { pos + read.1 })
}

/// Whether a zero-width assertion (`Start`, `End`, `WordBoundary`)
/// holds at `pos`.
pub(super) fn assertion<I: Input + ?Sized>(input: &I, inst: Inst, pos: u32) -> bool {
    let p = pos as usize;
    match inst {
        Inst::Start { multiline } => {
            p == 0 || (multiline && is_line_terminator(u32::from(input.unit(p - 1))))
        }
        Inst::End { multiline } => {
            p == input.len() || (multiline && is_line_terminator(u32::from(input.unit(p))))
        }
        Inst::WordBoundary {
            negated,
            unicode_ignore_case,
        } => {
            let a = p > 0 && is_word(u32::from(input.unit(p - 1)), unicode_ignore_case);
            let b = p < input.len() && is_word(u32::from(input.unit(p)), unicode_ignore_case);
            (a != b) != negated
        }
        _ => unreachable!("not an assertion"),
    }
}

fn atom_direction(inst: Inst) -> bool {
    match inst {
        Inst::Char { back, .. }
        | Inst::Set { back, .. }
        | Inst::Dot { back }
        | Inst::AnyChar { back } => back,
        _ => false,
    }
}

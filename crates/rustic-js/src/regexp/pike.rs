//! The linear-time matcher (TD-111): a Pike VM over a program from
//! [`vm::compile_pike`], for patterns with no backreference or
//! lookaround. It runs every alternative in step over the input, one
//! thread per instruction, kept in the backtracker's priority order, so
//! a match costs O(input × program) whatever the pattern and gives the
//! leftmost match the backtracker would have chosen. Searches start on
//! the backtracker and move here only when it backtracks too much.
//!
//! One known difference: two threads reaching the same instruction at
//! the same position are merged, keeping the higher-priority one, even
//! when only the lower one's repetition could still accept an empty
//! iteration. That changes at most which captures an empty iteration
//! leaves, never whether or where the match is found.

use super::ExecError;
use super::vm::{self, Input, Inst, Program, UNSET};

/// Matcher steps between checks of the cancellation hook.
const POLL_INTERVAL: u32 = 1 << 14;

/// Threads at one position, highest priority first.
struct Threads {
    pcs: Vec<u32>,
    /// `registers` values per thread.
    regs: Vec<u32>,
    /// The generation each instruction was last added in.
    seen: Vec<u32>,
}

enum Job {
    Explore(u32),
    Restore(u32, u32),
}

struct Pike<'a, I: Input + ?Sized> {
    program: &'a Program,
    input: &'a I,
    width: usize,
    jobs: Vec<Job>,
    countdown: u32,
    poll: &'a mut dyn FnMut() -> bool,
}

impl<I: Input + ?Sized> Pike<'_, I> {
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

    /// Add the thread at `pc` with registers `regs` to `list` at `pos`,
    /// following jumps, splits, saves and assertions in priority order.
    /// `regs` is restored before returning.
    fn add(
        &mut self,
        list: &mut Threads,
        generation: u32,
        pc: u32,
        pos: u32,
        regs: &mut [u32],
    ) -> Result<(), ExecError> {
        self.jobs.push(Job::Explore(pc));
        while let Some(job) = self.jobs.pop() {
            let pc = match job {
                Job::Restore(reg, value) => {
                    regs[reg as usize] = value;
                    continue;
                }
                Job::Explore(pc) => pc,
            };
            if list.seen[pc as usize] == generation {
                continue;
            }
            list.seen[pc as usize] = generation;
            self.tick()?;
            let inst = self.program.insts[pc as usize];
            match inst {
                Inst::Jump { to } => self.jobs.push(Job::Explore(to)),
                Inst::Split { next, alt } => {
                    self.jobs.push(Job::Explore(alt));
                    self.jobs.push(Job::Explore(next));
                }
                Inst::Save { reg } => {
                    self.jobs.push(Job::Restore(reg, regs[reg as usize]));
                    regs[reg as usize] = pos;
                    self.jobs.push(Job::Explore(pc + 1));
                }
                Inst::Clear { from, to } => {
                    for reg in from..to {
                        self.jobs.push(Job::Restore(reg, regs[reg as usize]));
                        regs[reg as usize] = UNSET;
                    }
                    self.jobs.push(Job::Explore(pc + 1));
                }
                Inst::Progress { reg } => {
                    if regs[reg as usize] != pos {
                        self.jobs.push(Job::Explore(pc + 1));
                    }
                }
                Inst::Start { .. } | Inst::End { .. } | Inst::WordBoundary { .. } => {
                    if vm::assertion(self.input, inst, pos) {
                        self.jobs.push(Job::Explore(pc + 1));
                    }
                }
                Inst::Char { .. }
                | Inst::Set { .. }
                | Inst::Dot { .. }
                | Inst::AnyChar { .. }
                | Inst::Match => {
                    list.pcs.push(pc);
                    list.regs.extend_from_slice(regs);
                }
                Inst::BackRef { .. }
                | Inst::Look { .. }
                | Inst::LoopInit { .. }
                | Inst::LoopHead { .. }
                | Inst::LoopEnter { .. }
                | Inst::LoopTail { .. }
                | Inst::Star { .. } => {
                    unreachable!("not in a linear-time program")
                }
            }
        }
        Ok(())
    }
}

impl Threads {
    fn new(insts: usize) -> Threads {
        Threads {
            pcs: Vec::new(),
            regs: Vec::new(),
            seen: vec![0; insts],
        }
    }

    fn clear(&mut self) {
        self.pcs.clear();
        self.regs.clear();
    }
}

impl Program {
    /// The first match at `start` or after it (only at `start` when
    /// `anchored`): the registers on success. Later starts follow
    /// AdvanceStringIndex, one character at a time.
    pub(crate) fn pike_search<I: Input + ?Sized>(
        &self,
        input: &I,
        start: usize,
        anchored: bool,
        poll: &mut dyn FnMut() -> bool,
    ) -> Result<Option<Vec<u32>>, ExecError> {
        let width = self.registers as usize;
        let mut m = Pike {
            program: self,
            input,
            width,
            jobs: Vec::new(),
            countdown: POLL_INTERVAL,
            poll,
        };
        let mut current = Threads::new(self.insts.len());
        let mut next = Threads::new(self.insts.len());
        let mut fresh = vec![UNSET; width];
        let mut found: Option<Vec<u32>> = None;
        let mut generation = 1;
        let mut pos = start as u32;
        loop {
            // A new start, behind every thread already running.
            if found.is_none() && (!anchored || pos as usize == start) {
                m.add(&mut current, generation, 0, pos, &mut fresh)?;
            }
            if current.pcs.is_empty() && (found.is_some() || anchored) {
                break;
            }
            // Every thread steps over the same character: with `u`, a
            // surrogate pair is one, as AdvanceStringIndex reads it.
            let step = vm::read(self, input, pos, false).map(|(_, w)| w);
            generation += 1;
            next.clear();
            for i in 0..current.pcs.len() {
                m.tick()?;
                let pc = current.pcs[i];
                let inst = self.insts[pc as usize];
                if let Inst::Match = inst {
                    // Lower-priority threads can only find a worse match.
                    found = Some(current.regs[i * m.width..(i + 1) * m.width].to_vec());
                    break;
                }
                let Some(step) = step else { continue };
                if vm::single(self, input, inst, pos).is_some() {
                    let regs = &mut current.regs[i * m.width..(i + 1) * m.width];
                    m.add(&mut next, generation, pc + 1, pos + step, regs)?;
                }
            }
            let Some(step) = step else { break };
            std::mem::swap(&mut current, &mut next);
            pos += step;
        }
        Ok(found)
    }
}

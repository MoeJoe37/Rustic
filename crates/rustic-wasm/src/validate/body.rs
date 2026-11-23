use super::*;
use crate::{BlockType, LocalGroup};

pub(super) fn function(
    context: &Context<'_, '_>,
    function: &Declaration<Function>,
    budget: &mut Budget<'_>,
) -> Result<(), ValidationError> {
    let signature = context.signature(function.value.type_index, function.offset)?;
    let locals = Locals::new(signature, &function.value.locals, function.offset, budget)?;
    let mut validator = Body {
        context,
        locals,
        visible_globals: context.globals.len(),
        returns: &signature.results,
        frames: Vec::new(),
        values: Vec::new(),
        budget,
        offset: function.offset,
    };
    validator.open(FrameKind::Block, &[], &signature.results)?;
    for instruction in &function.value.instructions {
        validator.check(instruction)?;
    }
    Ok(())
}

pub(super) fn constant(
    context: &Context<'_, '_>,
    instructions: &[Instruction],
    ty: Ty,
    visible_globals: usize,
    budget: &mut Budget<'_>,
) -> Result<(), ValidationError> {
    let returns = [ty];
    let mut validator = Body {
        context,
        locals: Locals {
            params: &[],
            groups: Vec::new(),
        },
        visible_globals,
        returns: &returns,
        frames: Vec::new(),
        values: Vec::new(),
        budget,
        offset: instructions[0].offset,
    };
    validator.open(FrameKind::Block, &[], &returns)?;
    for instruction in instructions {
        validator.offset = instruction.offset;
        match (instruction.opcode, &instruction.immediate) {
            (Opcode::Core(0x23), Immediate::Index(index)) => {
                let global = validator.global(*index)?;
                if global.mutable {
                    return Err(error(instruction.offset, Fault::NonConstant));
                }
            }
            (Opcode::Core(0x0b | 0x41..=0x44 | 0x6a..=0x6c | 0x7c..=0x7e | 0xd0 | 0xd2), _) => {}
            _ => return Err(error(instruction.offset, Fault::NonConstant)),
        }
        validator.check(instruction)?;
    }
    Ok(())
}

struct Locals<'a> {
    params: &'a [Ty],
    groups: Vec<(usize, Ty)>,
}

impl<'a> Locals<'a> {
    fn new(
        signature: &'a FunctionType,
        groups: &[LocalGroup],
        offset: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Self, ValidationError> {
        let mut locals = Self {
            params: &signature.params,
            groups: Vec::new(),
        };
        let mut end = locals.params.len();
        if end > budget.limits.locals {
            return Err(error(offset, Fault::LimitExceeded));
        }
        for group in groups {
            budget.spend(offset, 1)?;
            declared_type(group.value_type, offset)?;
            end = end
                .checked_add(group.count as usize)
                .filter(|&count| count <= budget.limits.locals)
                .ok_or_else(|| error(offset, Fault::LimitExceeded))?;
            if group.count != 0 {
                append(&mut locals.groups, (end, group.value_type), offset)?;
            }
        }
        Ok(locals)
    }

    fn get(&self, index: u32, offset: usize) -> Result<Ty, ValidationError> {
        let index = index as usize;
        if let Some(&param) = self.params.get(index) {
            return Ok(param);
        }
        let group = self.groups.partition_point(|&(end, _)| end <= index);
        self.groups
            .get(group)
            .map(|&(_, ty)| ty)
            .ok_or_else(|| error(offset, Fault::InvalidIndex(IndexSpace::Local)))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    Block,
    Loop,
    If,
    Else,
}

#[derive(Clone, Copy)]
struct Frame<'a> {
    kind: FrameKind,
    height: usize,
    unreachable: bool,
    params: &'a [Ty],
    results: &'a [Ty],
}

impl<'a> Frame<'a> {
    fn label(self) -> &'a [Ty] {
        if self.kind == FrameKind::Loop {
            self.params
        } else {
            self.results
        }
    }
}

struct Body<'a, 'context, 'bytes, 'budget, 'token> {
    context: &'a Context<'context, 'bytes>,
    locals: Locals<'a>,
    visible_globals: usize,
    returns: &'a [Ty],
    frames: Vec<Frame<'a>>,
    // None represents the polymorphic bottom type while typing dead code.
    values: Vec<Option<Ty>>,
    budget: &'budget mut Budget<'token>,
    offset: usize,
}

impl<'a> Body<'a, '_, '_, '_, '_> {
    fn check(&mut self, instruction: &'a Instruction) -> Result<(), ValidationError> {
        self.offset = instruction.offset;
        self.budget.spend(self.offset, 1)?;
        let op = instruction.opcode;
        let is_tail = matches!(op, Opcode::Core(0x12 | 0x13));
        match (op, &instruction.immediate) {
            (Opcode::Core(0x00), _) => self.make_unreachable(),
            (Opcode::Core(0x01), _) => {}
            (Opcode::Core(0x02..=0x04), Immediate::Block(signature)) => {
                let (params, results) = self.block_signature(signature)?;
                let kind = match op {
                    Opcode::Core(0x03) => FrameKind::Loop,
                    Opcode::Core(0x04) => {
                        self.pop(Ty::I32)?;
                        FrameKind::If
                    }
                    _ => FrameKind::Block,
                };
                self.open(kind, params, results)?;
            }
            (Opcode::Core(0x05), _) => {
                let frame = self.close()?;
                self.enter(FrameKind::Else, frame.params, frame.results)?;
            }
            (Opcode::Core(0x0b), _) => {
                let frame = self.close()?;
                if frame.kind == FrameKind::If
                    && !matches_values(frame.params, frame.results, self.offset, self.budget)?
                {
                    return Err(self.error(Fault::InvalidSignature));
                }
                if !self.frames.is_empty() {
                    self.push_types(frame.results)?;
                }
            }
            (Opcode::Core(0x0c | 0x0d), Immediate::Index(depth)) => {
                let label = self.label(*depth)?;
                if op == Opcode::Core(0x0d) {
                    self.pop(Ty::I32)?;
                    self.pop_types(label)?;
                    self.push_types(label)?;
                } else {
                    self.pop_types(label)?;
                    self.make_unreachable();
                }
            }
            (Opcode::Core(0x0e), Immediate::BranchTable { targets, default }) => {
                self.pop(Ty::I32)?;
                let default_types = self.label(*default)?;
                for &depth in targets {
                    self.budget.spend(self.offset, 1)?;
                    let types = self.label(depth)?;
                    if types.len() != default_types.len() {
                        return Err(self.error(Fault::InvalidSignature));
                    }
                    self.check_tail(types)?;
                }
                self.pop_types(default_types)?;
                self.make_unreachable();
            }
            (Opcode::Core(0x0f), _) => {
                self.pop_types(self.returns)?;
                self.make_unreachable();
            }
            (Opcode::Core(0x10 | 0x12), Immediate::Index(function)) => {
                self.call(self.context.function(*function, self.offset)?, is_tail)?;
            }
            (Opcode::Core(0x11 | 0x13), Immediate::Indices(ty, table)) => {
                let table = self.context.table(*table, self.offset)?;
                if !matches_type(Some(table.element_type), FUNCREF) {
                    return Err(self.error(Fault::TypeMismatch));
                }
                self.pop(Ty::I32)?;
                self.call(self.context.signature(*ty, self.offset)?, is_tail)?;
            }
            (Opcode::Core(0x1a), _) => {
                self.pop_any()?;
            }
            (Opcode::Core(0x1b), _) => {
                self.pop(Ty::I32)?;
                let a = self.pop_any()?;
                let b = self.pop_any()?;
                if a.is_some_and(|t| b.is_some_and(|v| t != v))
                    || [a, b]
                        .into_iter()
                        .flatten()
                        .any(|t| matches!(t, Ty::Ref { .. }))
                {
                    return Err(self.error(Fault::TypeMismatch));
                }
                self.push(a.or(b))?;
            }
            (Opcode::Core(0x1c), Immediate::Select(types)) => {
                let [ty] = types.as_slice() else {
                    return Err(self.error(Fault::InvalidSignature));
                };
                declared_type(*ty, self.offset)?;
                self.effect(&[*ty, *ty, Ty::I32], &[*ty])?;
            }
            (Opcode::Core(0x20..=0x22), Immediate::Index(local)) => {
                let ty = self.locals.get(*local, self.offset)?;
                match op {
                    Opcode::Core(0x20) => self.push(Some(ty))?,
                    Opcode::Core(0x21) => self.pop(ty)?,
                    _ => self.effect(&[ty], &[ty])?,
                }
            }
            (Opcode::Core(0x23 | 0x24), Immediate::Index(global)) => {
                let global = self.global(*global)?;
                if op == Opcode::Core(0x23) {
                    self.push(Some(global.value_type))?;
                } else if global.mutable {
                    self.pop(global.value_type)?;
                } else {
                    return Err(self.error(Fault::ImmutableGlobal));
                }
            }
            (Opcode::Core(0x25 | 0x26), Immediate::Index(table)) => {
                let ty = self.context.table(*table, self.offset)?.element_type;
                if op == Opcode::Core(0x25) {
                    self.effect(&[Ty::I32], &[ty])?;
                } else {
                    self.pop_types(&[Ty::I32, ty])?;
                }
            }
            (
                Opcode::Core(code @ 0x28..=0x3e),
                Immediate::Memory {
                    memory,
                    alignment,
                    offset,
                },
            ) => {
                self.context.memory(*memory, self.offset)?;
                let (ty, natural_alignment) = numeric::memory_access(code);
                if *alignment > natural_alignment || *offset > u64::from(u32::MAX) {
                    return Err(self.error(Fault::InvalidLimits));
                }
                if code < 0x36 {
                    self.effect(&[Ty::I32], &[ty])?;
                } else {
                    self.pop_types(&[Ty::I32, ty])?;
                }
            }
            (Opcode::Core(0x3f | 0x40), Immediate::Index(memory)) => {
                self.context.memory(*memory, self.offset)?;
                if op == Opcode::Core(0x40) {
                    self.pop(Ty::I32)?;
                }
                self.push(Some(Ty::I32))?;
            }
            (Opcode::Core(0xd0), Immediate::HeapType(heap_type)) => {
                let ty = Ty::Ref {
                    heap_type: *heap_type,
                    nullable: true,
                };
                declared_type(ty, self.offset)?;
                self.push(Some(ty))?;
            }
            (Opcode::Core(0xd1 | 0xd4), _) => {
                let reference = self.pop_any()?;
                if reference.is_some_and(|ty| !matches!(ty, Ty::Ref { .. })) {
                    return Err(self.error(Fault::TypeMismatch));
                }
                if op == Opcode::Core(0xd1) {
                    self.push(Some(Ty::I32))?;
                } else {
                    let narrowed = match reference {
                        Some(Ty::Ref { heap_type, .. }) => Some(Ty::Ref {
                            heap_type,
                            nullable: false,
                        }),
                        _ => None,
                    };
                    self.push(narrowed)?;
                }
            }
            (Opcode::Core(0xd2), Immediate::Index(function)) => {
                if !*index(
                    &self.context.referenced_functions,
                    *function,
                    IndexSpace::Function,
                    self.offset,
                )? {
                    return Err(self.error(Fault::UndeclaredFunction));
                }
                self.push(Some(Ty::Ref {
                    heap_type: -16,
                    nullable: false,
                }))?;
            }
            (Opcode::Misc(8..=17), immediate) => self.bulk(op, immediate)?,
            _ => match numeric::effect(op) {
                Some((params, result)) => self.effect(params, &[result])?,
                None => return Err(self.error(Fault::UnsupportedInstruction)),
            },
        }
        Ok(())
    }

    fn bulk(&mut self, op: Opcode, immediate: &Immediate) -> Result<(), ValidationError> {
        let context = self.context;
        let at = self.offset;
        match (op, immediate) {
            (Opcode::Misc(8), Immediate::Indices(data, memory)) => {
                context.data(*data, at)?;
                context.memory(*memory, at)?;
            }
            (Opcode::Misc(9), Immediate::Index(data)) => return context.data(*data, at),
            (Opcode::Misc(10), Immediate::Indices(dst, src)) => {
                context.memory(*dst, at)?;
                context.memory(*src, at)?;
            }
            (Opcode::Misc(11), Immediate::Index(memory)) => context.memory(*memory, at)?,
            (Opcode::Misc(12), Immediate::Indices(element, table)) => {
                let element = context.element(*element, at)?;
                let table = context.table(*table, at)?.element_type;
                if !matches_type(Some(element), table) {
                    return Err(self.error(Fault::TypeMismatch));
                }
            }
            (Opcode::Misc(13), Immediate::Index(element)) => {
                context.element(*element, at)?;
                return Ok(());
            }
            (Opcode::Misc(14), Immediate::Indices(dst, src)) => {
                let dst = context.table(*dst, at)?.element_type;
                let src = context.table(*src, at)?.element_type;
                if !matches_type(Some(src), dst) {
                    return Err(self.error(Fault::TypeMismatch));
                }
            }
            (Opcode::Misc(15), Immediate::Index(table)) => {
                let element = context.table(*table, at)?.element_type;
                return self.effect(&[element, Ty::I32], &[Ty::I32]);
            }
            (Opcode::Misc(16), Immediate::Index(table)) => {
                context.table(*table, at)?;
                return self.push(Some(Ty::I32));
            }
            (Opcode::Misc(17), Immediate::Index(table)) => {
                let element = context.table(*table, at)?.element_type;
                return self.pop_types(&[Ty::I32, element, Ty::I32]);
            }
            _ => return Err(self.error(Fault::UnsupportedInstruction)),
        }
        self.pop_types(&[Ty::I32; 3])
    }

    fn call(&mut self, signature: &FunctionType, tail: bool) -> Result<(), ValidationError> {
        self.pop_types(&signature.params)?;
        if tail {
            if !matches_values(&signature.results, self.returns, self.offset, self.budget)? {
                return Err(self.error(Fault::InvalidSignature));
            }
            self.make_unreachable();
            Ok(())
        } else {
            self.push_types(&signature.results)
        }
    }

    fn global(&self, index: u32) -> Result<GlobalType, ValidationError> {
        check_index(index, self.visible_globals, IndexSpace::Global, self.offset)?;
        self.context.global(index, self.offset)
    }

    fn block_signature(
        &self,
        signature: &'a BlockType,
    ) -> Result<(&'a [Ty], &'a [Ty]), ValidationError> {
        match signature {
            BlockType::Empty => Ok((&[], &[])),
            BlockType::Result(ty) => {
                declared_type(*ty, self.offset)?;
                Ok((&[], std::slice::from_ref(ty)))
            }
            BlockType::TypeIndex(index) => {
                let signature = self.context.signature(*index, self.offset)?;
                Ok((&signature.params, &signature.results))
            }
        }
    }

    fn label(&self, depth: u32) -> Result<&'a [Ty], ValidationError> {
        self.frames
            .iter()
            .rev()
            .nth(depth as usize)
            .map(|frame| frame.label())
            .ok_or_else(|| self.error(Fault::InvalidIndex(IndexSpace::Label)))
    }

    fn enter(
        &mut self,
        kind: FrameKind,
        params: &'a [Ty],
        results: &'a [Ty],
    ) -> Result<(), ValidationError> {
        let height = self.values.len();
        append(
            &mut self.frames,
            Frame {
                kind,
                height,
                unreachable: false,
                params,
                results,
            },
            self.offset,
        )?;
        self.push_types(params)
    }

    fn open(
        &mut self,
        kind: FrameKind,
        params: &'a [Ty],
        results: &'a [Ty],
    ) -> Result<(), ValidationError> {
        self.pop_types(params)?;
        self.enter(kind, params, results)
    }

    fn close(&mut self) -> Result<Frame<'a>, ValidationError> {
        let frame = *self.frame();
        self.pop_types(frame.results)?;
        if self.values.len() != frame.height {
            return Err(self.error(Fault::ExtraOperands));
        }
        self.frames.pop();
        Ok(frame)
    }

    fn frame(&self) -> &Frame<'a> {
        self.frames
            .last()
            .expect("a decoded expression always has an open frame")
    }

    fn make_unreachable(&mut self) {
        let frame = self.frames.last_mut().expect("an open control frame");
        self.values.truncate(frame.height);
        frame.unreachable = true;
    }

    fn check_tail(&mut self, types: &[Ty]) -> Result<(), ValidationError> {
        self.budget.spend(self.offset, types.len())?;
        let frame = self.frame();
        let values = &self.values[frame.height..];
        if !frame.unreachable && values.len() < types.len() {
            return Err(self.error(Fault::StackUnderflow));
        }
        if values
            .iter()
            .rev()
            .zip(types.iter().rev())
            .any(|(&a, &b)| !matches_type(a, b))
        {
            return Err(self.error(Fault::TypeMismatch));
        }
        Ok(())
    }

    fn pop_any(&mut self) -> Result<Option<Ty>, ValidationError> {
        self.budget.spend(self.offset, 1)?;
        let frame = self.frame();
        if self.values.len() == frame.height {
            if frame.unreachable {
                Ok(None)
            } else {
                Err(self.error(Fault::StackUnderflow))
            }
        } else {
            Ok(self.values.pop().expect("operand above frame height"))
        }
    }

    fn pop(&mut self, ty: Ty) -> Result<(), ValidationError> {
        if matches_type(self.pop_any()?, ty) {
            Ok(())
        } else {
            Err(self.error(Fault::TypeMismatch))
        }
    }

    fn pop_types(&mut self, types: &[Ty]) -> Result<(), ValidationError> {
        for &ty in types.iter().rev() {
            self.pop(ty)?;
        }
        Ok(())
    }

    fn push(&mut self, ty: Option<Ty>) -> Result<(), ValidationError> {
        self.budget.spend(self.offset, 1)?;
        if self.values.len() >= self.budget.limits.operands {
            return Err(self.error(Fault::LimitExceeded));
        }
        append(&mut self.values, ty, self.offset)
    }

    fn push_types(&mut self, types: &[Ty]) -> Result<(), ValidationError> {
        for &ty in types {
            self.push(Some(ty))?;
        }
        Ok(())
    }

    fn effect(&mut self, input: &[Ty], output: &[Ty]) -> Result<(), ValidationError> {
        self.pop_types(input)?;
        self.push_types(output)
    }

    fn error(&self, kind: Fault) -> ValidationError {
        error(self.offset, kind)
    }
}

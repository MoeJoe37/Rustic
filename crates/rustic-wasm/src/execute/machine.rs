use super::state::Transfer;
use super::*;
use crate::Immediate;

pub(super) trait BoundaryValue: Copy {
    fn supports(ty: ValueType) -> bool;
    fn into_slot(self) -> Slot;
    fn from_slot(slot: Slot) -> Self;
}

impl BoundaryValue for Value {
    fn supports(ty: ValueType) -> bool {
        is_numeric(ty)
    }

    fn into_slot(self) -> Slot {
        Slot::Number(self)
    }

    fn from_slot(slot: Slot) -> Self {
        slot.number()
    }
}

impl BoundaryValue for HostValue {
    fn supports(ty: ValueType) -> bool {
        Self::supports(ty)
    }

    fn into_slot(self) -> Slot {
        self
    }

    fn from_slot(slot: Slot) -> Self {
        slot
    }
}

pub(super) fn invoke<'bytes, T: BoundaryValue>(
    instance: &Executable<'_, 'bytes>,
    state: &RefCell<State<'bytes>>,
    function: usize,
    arguments: &[T],
    entry_work: usize,
    cancellation: &CancellationToken,
    dispatch: Option<Dispatch<'_, '_>>,
) -> Result<Vec<T>, ExecutionError> {
    let mut machine = Machine {
        instance,
        state,
        dispatch,
        calls: Vec::new(),
        controls: Vec::new(),
        locals: Vec::new(),
        operands: Vec::new(),
        budget: Budget {
            remaining: instance.limits.invocation_work,
            cancellation,
        },
        offset: 0,
    };
    let signature = instance.function_signature(function);
    if arguments.len() != signature.params.len() {
        return Err(error(0, Fault::ArgumentType));
    }
    machine.budget.spend(0, entry_work)?;
    machine.budget.spend(0, arguments.len())?;
    machine.budget.spend(0, signature.results.len())?;
    machine.check_values(arguments.len())?;
    if arguments
        .iter()
        .zip(&signature.params)
        .any(|(&value, &ty)| value.into_slot().value_type() != ty)
    {
        return Err(error(0, Fault::ArgumentType));
    }
    if !signature.results.iter().copied().all(T::supports) {
        return Err(error(0, Fault::ResultType));
    }
    machine.check_values(signature.results.len())?;
    let mut results = Vec::new();
    results
        .try_reserve_exact(signature.results.len())
        .map_err(|_| error(0, Fault::LimitExceeded))?;
    machine
        .operands
        .try_reserve_exact(arguments.len())
        .map_err(|_| error(0, Fault::LimitExceeded))?;
    machine
        .operands
        .extend(arguments.iter().copied().map(T::into_slot));
    if machine
        .operands
        .iter()
        .any(|value| value.host_root().is_some())
    {
        state.borrow().has_host_references.set(true);
    }
    machine.call(function)?;
    while let Some(current) = machine.calls.last_mut() {
        let pc = current.pc;
        let function = &instance.functions[current.function];
        let instruction = &function.function.instructions[pc];
        current.pc += 1;
        machine.offset = instruction.offset;
        machine.budget.spend(instruction.offset, 1)?;
        machine.step(instruction, function.jumps[pc])?;
    }
    results.extend(machine.operands.into_iter().map(T::from_slot));
    Ok(results)
}

fn exchange<T: BoundaryValue>(
    arguments: &[Slot],
    signature: &FunctionType,
    offset: usize,
    call: impl FnOnce(&[T], &mut [T]) -> Result<(), HostTrap>,
) -> Result<Vec<T>, ExecutionError> {
    if !signature
        .params
        .iter()
        .chain(&signature.results)
        .copied()
        .all(T::supports)
    {
        return Err(error(offset, Fault::UnsupportedInstruction));
    }
    let mut input = Vec::new();
    input
        .try_reserve_exact(arguments.len())
        .map_err(|_| error(offset, Fault::LimitExceeded))?;
    input.extend(arguments.iter().copied().map(T::from_slot));
    let mut output = Vec::new();
    output
        .try_reserve_exact(signature.results.len())
        .map_err(|_| error(offset, Fault::LimitExceeded))?;
    output.extend(
        signature
            .results
            .iter()
            .copied()
            .map(|ty| T::from_slot(Slot::zero(ty))),
    );
    call(&input, &mut output).map_err(|_| error(offset, Fault::HostTrap))?;
    if output
        .iter()
        .zip(&signature.results)
        .any(|(&value, &ty)| value.into_slot().value_type() != ty)
    {
        return Err(error(offset, Fault::HostResultType));
    }
    Ok(output)
}

struct Call {
    function: usize,
    pc: usize,
    local_base: usize,
    control_base: usize,
}

#[derive(Clone, Copy)]
struct Control {
    height: usize,
    results: usize,
    end: usize,
    loop_start: Option<(usize, usize)>, // Program counter and loop argument count.
}

struct Machine<'instance, 'module, 'bytes, 'state, 'token, 'dispatch, 'host> {
    instance: &'instance Executable<'module, 'bytes>,
    state: &'state RefCell<State<'bytes>>,
    dispatch: Option<Dispatch<'dispatch, 'host>>,
    calls: Vec<Call>,
    controls: Vec<Control>,
    locals: Vec<Slot>,
    operands: Vec<Slot>,
    budget: Budget<'token>,
    offset: usize,
}

impl Machine<'_, '_, '_, '_, '_, '_, '_> {
    fn step(&mut self, instruction: &Instruction, jump: Jump) -> Result<(), ExecutionError> {
        match (instruction.opcode, &instruction.immediate) {
            (Opcode::Core(0x00), _) => return Err(error(self.offset, Fault::Unreachable)),
            (Opcode::Core(0x01), _) => {}
            (opcode @ Opcode::Core(0x02..=0x04), Immediate::Block(block)) => {
                self.open(opcode, *block, jump)?;
            }
            (Opcode::Core(0x05), _) => {
                let end = self.controls.last().expect("open if").end;
                self.current().pc = end;
            }
            (Opcode::Core(0x0b), _) => {
                let index = self.controls.len() - 1;
                let control = self.controls[index];
                self.leave(index, control)?;
            }
            (Opcode::Core(0x0c), Immediate::Index(depth)) => self.branch(*depth)?,
            (Opcode::Core(0x0d), Immediate::Index(depth)) => {
                if self.pop_i32() != 0 {
                    self.branch(*depth)?;
                }
            }
            (Opcode::Core(0x0e), Immediate::BranchTable { targets, default }) => {
                let selector = self.pop_i32() as u32 as usize;
                self.branch(*targets.get(selector).unwrap_or(default))?;
            }
            (Opcode::Core(0x0f), _) => {
                let index = self.current().control_base;
                self.leave(index, self.controls[index])?;
            }
            (Opcode::Core(0x10), Immediate::Index(function)) => self.call(*function as usize)?,
            (Opcode::Core(0x12), Immediate::Index(function)) => {
                self.tail_call(*function as usize)?
            }
            (op @ Opcode::Core(0x11 | 0x13), Immediate::Indices(ty, table)) => {
                self.call_indirect(*ty, *table, op == Opcode::Core(0x13))?;
            }
            (Opcode::Core(0x1a), _) => {
                self.pop();
            }
            (Opcode::Core(0x1b | 0x1c), _) => {
                let condition = self.pop_i32();
                let otherwise = self.pop();
                let first = self.pop();
                self.push(if condition == 0 { otherwise } else { first })?;
            }
            (opcode @ Opcode::Core(0x20..=0x22), Immediate::Index(local)) => {
                let index = self.current().local_base + *local as usize;
                match opcode {
                    Opcode::Core(0x20) => self.push(self.locals[index])?,
                    Opcode::Core(0x21) => self.locals[index] = self.pop(),
                    _ => self.locals[index] = *self.operands.last().expect("typed operand"),
                }
            }
            (Opcode::Core(0x23), Immediate::Index(global)) => {
                let value = self.state.borrow().globals[*global as usize].value();
                self.push(value)?;
            }
            (Opcode::Core(0x24), Immediate::Index(global)) => {
                let value = self.pop();
                self.state.borrow_mut().globals[*global as usize].write_guest(value);
            }
            (Opcode::Core(0x25), Immediate::Index(table)) => {
                let index = self.pop().address();
                let value = self.state.borrow().tables.get(*table, index, self.offset)?;
                self.push(Slot::Ref(value))?;
            }
            (Opcode::Core(0x26), Immediate::Index(table)) => {
                let value = self.pop().reference();
                let index = self.pop().address();
                self.state
                    .borrow_mut()
                    .tables
                    .set(*table, index, value, self.offset)?;
            }
            (Opcode::Core(op @ 0x28..=0x3e), Immediate::Memory { memory, offset, .. }) => {
                let value = (op >= 0x36).then(|| self.pop().number());
                let address = self
                    .pop()
                    .address()
                    .checked_add(*offset)
                    .ok_or_else(|| error(self.offset, Fault::MemoryOutOfBounds))?;
                if let Some(result) =
                    self.state
                        .borrow_mut()
                        .access(op, *memory, address, value, self.offset)?
                {
                    self.push(Slot::Number(result))?;
                }
            }
            (Opcode::Core(0x3f), Immediate::Index(memory)) => {
                let size = self.state.borrow().size(*memory);
                self.push(Slot::Number(size))?;
            }
            (Opcode::Core(0x40), Immediate::Index(memory)) => {
                let pages = self.pop().address();
                let result =
                    self.state
                        .borrow_mut()
                        .grow(*memory, pages, self.offset, &mut self.budget)?;
                self.push(Slot::Number(result))?;
            }
            (Opcode::Core(0xd0), Immediate::HeapType(ty)) => {
                self.push(Slot::Ref(Reference::null(*ty)))?;
            }
            (Opcode::Core(0xd1), _) => {
                let is_null = self.pop().reference().is_null();
                self.push(Slot::Number(Value::I32(i32::from(is_null))))?;
            }
            (Opcode::Core(0xd2), Immediate::Index(function)) => {
                let owner = self.state.borrow().function_owner;
                self.push(Slot::Ref(FunctionAddress::reference(owner, *function)))?;
            }
            (Opcode::Core(0xd4), _) => {
                let value = self.pop().reference();
                if value.is_null() {
                    return Err(error(self.offset, Fault::NullReference));
                }
                self.push(Slot::Ref(value))?;
            }
            (Opcode::Misc(8), Immediate::Indices(data, memory)) => {
                let transfer = self.pop_transfer();
                self.state.borrow_mut().init(
                    *data,
                    *memory,
                    transfer,
                    self.offset,
                    &mut self.budget,
                )?;
            }
            (Opcode::Misc(9), Immediate::Index(data)) => self.state.borrow_mut().drop_data(*data),
            (Opcode::Misc(10), Immediate::Indices(destination, source)) => {
                let transfer = self.pop_transfer();
                self.state.borrow_mut().copy(
                    *destination,
                    *source,
                    transfer,
                    self.offset,
                    &mut self.budget,
                )?;
            }
            (Opcode::Misc(11), Immediate::Index(memory)) => {
                let transfer = self.pop_transfer();
                self.state
                    .borrow_mut()
                    .fill(*memory, transfer, self.offset, &mut self.budget)?;
            }
            (Opcode::Misc(12), Immediate::Indices(element, table)) => {
                let transfer = self.pop_transfer();
                self.state.borrow_mut().tables.init(
                    *element,
                    *table,
                    transfer,
                    self.offset,
                    &mut self.budget,
                )?;
            }
            (Opcode::Misc(13), Immediate::Index(element)) => {
                self.state.borrow_mut().tables.drop_element(*element)
            }
            (Opcode::Misc(14), Immediate::Indices(destination, source)) => {
                let transfer = self.pop_transfer();
                self.state.borrow_mut().tables.copy(
                    *destination,
                    *source,
                    transfer,
                    self.offset,
                    &mut self.budget,
                )?;
            }
            (Opcode::Misc(15), Immediate::Index(table)) => {
                let count = self.pop().address();
                let value = self.pop().reference();
                let size = self.state.borrow_mut().tables.grow(
                    *table,
                    count,
                    value,
                    self.offset,
                    &mut self.budget,
                )?;
                self.push(Slot::Number(size))?;
            }
            (Opcode::Misc(16), Immediate::Index(table)) => {
                let size = self.state.borrow().tables.size(*table);
                self.push(Slot::Number(size))?;
            }
            (Opcode::Misc(17), Immediate::Index(table)) => {
                let count = self.pop().address();
                let value = self.pop().reference();
                let index = self.pop().address();
                self.state.borrow_mut().tables.fill(
                    *table,
                    index,
                    count,
                    value,
                    &mut self.budget,
                    self.offset,
                )?;
            }
            (opcode, immediate) => {
                let result = numeric::apply(opcode, immediate, &mut self.operands)
                    .map_err(|kind| error(self.offset, kind))?;
                self.push(Slot::Number(result))?;
            }
        }
        Ok(())
    }

    fn current(&mut self) -> &mut Call {
        self.calls.last_mut().expect("open call")
    }

    fn call_indirect(&mut self, ty: u32, table: u32, tail: bool) -> Result<(), ExecutionError> {
        let index = self.pop().address();
        let state = self.state.borrow();
        let value = state.tables.get(table, index, self.offset)?;
        let Reference::Function(address) = value else {
            return Err(error(self.offset, Fault::NullReference));
        };
        let owner = state.function_owner;
        drop(state);
        let expected = self.instance.signature(ty);
        if address.owner() != owner {
            if tail {
                self.leave_for_tail_call(expected.params.len())?;
            }
            return self.call_host(HostCall::Indirect { address, expected }, None, expected);
        }
        let function = address.index() as usize;
        let callee = self.instance.function(function);
        if callee.type_index() != ty {
            let actual = self.instance.signature(callee.type_index());
            if actual.params.len() != expected.params.len()
                || actual.results.len() != expected.results.len()
            {
                return Err(error(self.offset, Fault::IndirectCallTypeMismatch));
            }
            self.budget
                .spend(self.offset, actual.params.len() + actual.results.len())?;
            if actual != expected {
                return Err(error(self.offset, Fault::IndirectCallTypeMismatch));
            }
        }
        if tail {
            self.tail_call(function)
        } else {
            self.call(function)
        }
    }

    fn tail_call(&mut self, function: usize) -> Result<(), ExecutionError> {
        let count = self.instance.function_signature(function).params.len();
        self.leave_for_tail_call(count)?;
        self.call(function)
    }

    fn leave_for_tail_call(&mut self, params: usize) -> Result<(), ExecutionError> {
        let index = self.current().control_base;
        self.keep_at(self.controls[index].height, params)?;
        self.pop_call();
        Ok(())
    }

    fn open(&mut self, opcode: Opcode, block: BlockType, jump: Jump) -> Result<(), ExecutionError> {
        let condition = opcode == Opcode::Core(0x04) && self.pop_i32() == 0;
        let (params, results) = self.instance.block_arity(block);
        let body = self.current().pc;
        self.push_control(Control {
            height: self.operands.len() - params,
            results,
            end: jump.end,
            loop_start: (opcode == Opcode::Core(0x03)).then_some((body, params)),
        })?;
        if condition {
            self.current().pc = jump.otherwise.unwrap_or(jump.end);
        }
        Ok(())
    }

    fn branch(&mut self, depth: u32) -> Result<(), ExecutionError> {
        let index = self.controls.len() - 1 - depth as usize;
        let target = self.controls[index];
        if let Some((pc, params)) = target.loop_start {
            self.keep_at(target.height, params)?;
            self.controls.truncate(index + 1);
            self.current().pc = pc;
        } else {
            self.leave(index, target)?;
        }
        Ok(())
    }

    fn leave(&mut self, index: usize, control: Control) -> Result<(), ExecutionError> {
        self.keep_at(control.height, control.results)?;
        if index == self.current().control_base {
            self.pop_call();
        } else {
            self.controls.truncate(index);
            self.current().pc = control.end + 1;
        }
        Ok(())
    }

    fn call(&mut self, function: usize) -> Result<(), ExecutionError> {
        let offset = self.offset;
        let executable = self.instance;
        let (function, prepared) = match executable.function(function) {
            Callee::Host(import) => {
                let signature = executable.signature(import.type_index);
                return self.call_host(HostCall::Import(function), Some(import), signature);
            }
            Callee::Local(index, local) => (index, local),
        };
        let signature = executable.signature(prepared.function.type_index);
        let params = signature.params.len();
        let local_base = self.locals.len();
        let height = self.operands.len() - params;
        if self.calls.len() >= self.instance.limits.calls {
            return Err(error(offset, Fault::LimitExceeded));
        }
        self.check_values(prepared.locals)?;
        self.budget.spend(offset, params)?;
        self.budget.spend(offset, prepared.locals)?;
        self.locals
            .try_reserve(params + prepared.locals)
            .map_err(|_| error(offset, Fault::LimitExceeded))?;
        self.locals.extend_from_slice(&self.operands[height..]);
        self.operands.truncate(height);
        for group in &prepared.function.locals {
            self.budget.spend(offset, 1)?;
            self.locals.resize(
                self.locals.len() + group.count as usize,
                Slot::zero(group.value_type),
            );
        }
        let control_base = self.controls.len();
        self.push_control(Control {
            height,
            results: signature.results.len(),
            end: prepared.function.instructions.len() - 1,
            loop_start: None,
        })?;
        append(
            &mut self.calls,
            Call {
                function,
                pc: 0,
                local_base,
                control_base,
            },
            offset,
        )
    }

    fn call_host(
        &mut self,
        target: HostCall<'_>,
        import: Option<&BoundFunction<'_>>,
        signature: &FunctionType,
    ) -> Result<(), ExecutionError> {
        let offset = self.offset;
        let params = signature.params.len();
        let result_count = signature.results.len();
        let additional = result_count.saturating_sub(params);
        let start = self.operands.len() - params;
        self.check_values(additional)?;
        self.budget.spend(offset, params + result_count)?;
        self.operands
            .try_reserve(additional)
            .map_err(|_| error(offset, Fault::LimitExceeded))?;
        let outcome = if let Some(callback) = import.and_then(|import| import.callback) {
            exchange::<Value>(
                &self.operands[start..],
                signature,
                offset,
                |args, results| callback(args, results, self.budget.cancellation),
            )
            .map(|results| {
                self.operands.truncate(start);
                self.operands.extend(results.into_iter().map(Slot::Number));
            })
        } else {
            match self.dispatch.as_mut() {
                Some(Dispatch::Plain(dispatch)) => {
                    let HostCall::Import(index) = target else {
                        return Err(error(offset, Fault::HostTrap));
                    };
                    exchange::<Value>(
                        &self.operands[start..],
                        signature,
                        offset,
                        |args, results| dispatch(index, args, results),
                    )
                    .map(|results| {
                        self.operands.truncate(start);
                        self.operands.extend(results.into_iter().map(Slot::Number));
                    })
                }
                Some(Dispatch::Tracing(dispatch)) => {
                    let trace = self.state.borrow().has_host_references.get();
                    let mut roots = if trace {
                        self.budget
                            .spend(offset, self.locals.len() + self.operands.len())?;
                        self.locals.iter().chain(&self.operands)
                    } else {
                        [].iter().chain(&[])
                    }
                    .filter_map(Slot::host_root);
                    let results = exchange::<HostValue>(
                        &self.operands[start..],
                        signature,
                        offset,
                        |args, results| dispatch(target, args, results, &mut roots),
                    );
                    results.map(|results| {
                        if results.iter().any(|value| value.host_root().is_some()) {
                            self.state.borrow().has_host_references.set(true);
                        }
                        self.operands.truncate(start);
                        self.operands.extend(results);
                    })
                }
                None => Err(error(offset, Fault::HostTrap)),
            }
        };
        self.budget.spend(offset, 0)?;
        outcome
    }

    fn pop_call(&mut self) {
        let call = self.calls.pop().expect("open call");
        self.locals.truncate(call.local_base);
        self.controls.truncate(call.control_base);
    }

    fn keep_at(&mut self, height: usize, count: usize) -> Result<(), ExecutionError> {
        let start = self.operands.len() - count;
        if start != height {
            self.budget.spend(self.offset, count)?;
            self.operands.copy_within(start.., height);
            self.operands.truncate(height + count);
        }
        Ok(())
    }

    fn check_values(&self, additional: usize) -> Result<(), ExecutionError> {
        if additional
            > self
                .instance
                .limits
                .values
                .saturating_sub(self.locals.len() + self.operands.len())
        {
            Err(error(self.offset, Fault::LimitExceeded))
        } else {
            Ok(())
        }
    }

    fn push_control(&mut self, frame: Control) -> Result<(), ExecutionError> {
        if self.controls.len() >= self.instance.limits.control_frames {
            return Err(error(self.offset, Fault::LimitExceeded));
        }
        append(&mut self.controls, frame, self.offset)
    }

    fn push(&mut self, value: Slot) -> Result<(), ExecutionError> {
        self.check_values(1)?;
        append(&mut self.operands, value, self.offset)
    }

    fn pop(&mut self) -> Slot {
        self.operands.pop().expect("validated stack operand")
    }

    fn pop_i32(&mut self) -> i32 {
        self.pop().i32()
    }

    fn pop_transfer(&mut self) -> Transfer {
        let size = self.pop().address();
        let source = self.pop().address();
        let destination = self.pop().address();
        Transfer {
            destination,
            source,
            size,
        }
    }
}

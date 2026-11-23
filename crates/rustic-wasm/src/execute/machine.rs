use super::state::Transfer;
use super::*;
use crate::Immediate;

pub(super) fn invoke<'bytes>(
    instance: &Executable<'_, 'bytes>,
    state: &mut State<'bytes>,
    function: usize,
    arguments: &[Value],
    entry_work: usize,
    cancellation: &CancellationToken,
) -> Result<Vec<Value>, ExecutionError> {
    let mut machine = Machine {
        instance,
        state,
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
    let signature = instance.functions[function].signature;
    if arguments.len() != signature.params.len() {
        return Err(error(0, Fault::ArgumentType));
    }
    machine.budget.spend(0, entry_work)?;
    machine.budget.spend(0, arguments.len())?;
    machine.check_values(arguments.len())?;
    if arguments
        .iter()
        .zip(&signature.params)
        .any(|(&value, &ty)| value.value_type() != ty)
    {
        return Err(error(0, Fault::ArgumentType));
    }
    machine
        .operands
        .try_reserve_exact(arguments.len())
        .map_err(|_| error(0, Fault::LimitExceeded))?;
    machine.operands.extend_from_slice(arguments);
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
    Ok(machine.operands)
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

struct Machine<'instance, 'module, 'bytes, 'state, 'token> {
    instance: &'instance Executable<'module, 'bytes>,
    state: &'state mut State<'bytes>,
    calls: Vec<Call>,
    controls: Vec<Control>,
    locals: Vec<Value>,
    operands: Vec<Value>,
    budget: Budget<'token>,
    offset: usize,
}

impl Machine<'_, '_, '_, '_, '_> {
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
                let index = self.current().control_base;
                let count = self.instance.functions[*function as usize]
                    .signature
                    .params
                    .len();
                self.keep_at(self.controls[index].height, count)?;
                self.pop_call();
                self.call(*function as usize)?;
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
                self.push(self.state.globals[*global as usize])?;
            }
            (Opcode::Core(0x24), Immediate::Index(global)) => {
                self.state.globals[*global as usize] = self.pop();
            }
            (Opcode::Core(op @ 0x28..=0x3e), Immediate::Memory { memory, offset, .. }) => {
                let value = (op >= 0x36).then(|| self.pop());
                let address = u64::from(self.pop_i32() as u32) + offset;
                if let Some(result) = self
                    .state
                    .access(op, *memory, address, value, self.offset)?
                {
                    self.push(result)?;
                }
            }
            (Opcode::Core(0x3f), Immediate::Index(memory)) => {
                self.push(Value::I32(self.state.size(*memory)))?;
            }
            (Opcode::Core(0x40), Immediate::Index(memory)) => {
                let pages = self.pop_i32() as u32;
                let result = self
                    .state
                    .grow(*memory, pages, self.offset, &mut self.budget)?;
                self.push(Value::I32(result))?;
            }
            (Opcode::Misc(8), Immediate::Indices(data, memory)) => {
                let transfer = self.pop_transfer();
                self.state
                    .init(*data, *memory, transfer, self.offset, &mut self.budget)?;
            }
            (Opcode::Misc(9), Immediate::Index(data)) => self.state.drop_data(*data),
            (Opcode::Misc(10), Immediate::Indices(destination, source)) => {
                let transfer = self.pop_transfer();
                self.state.copy(
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
                    .fill(*memory, transfer, self.offset, &mut self.budget)?;
            }
            (opcode, immediate) => {
                let (params, _) = effect(opcode).expect("prepared numeric instruction");
                let start = self.operands.len() - params.len();
                let result = numeric::evaluate(opcode, immediate, &self.operands[start..])
                    .map_err(|kind| error(self.offset, kind))?;
                self.operands.truncate(start);
                self.push(result)?;
            }
        }
        Ok(())
    }

    fn current(&mut self) -> &mut Call {
        self.calls.last_mut().expect("open call")
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
        let prepared = &self.instance.functions[function];
        let params = prepared.signature.params.len();
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
                Value::zero(group.value_type),
            );
        }
        let control_base = self.controls.len();
        self.push_control(Control {
            height,
            results: prepared.signature.results.len(),
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

    fn push(&mut self, value: Value) -> Result<(), ExecutionError> {
        self.check_values(1)?;
        append(&mut self.operands, value, self.offset)
    }

    fn pop(&mut self) -> Value {
        self.operands.pop().expect("validated stack operand")
    }

    fn pop_i32(&mut self) -> i32 {
        let Value::I32(value) = self.pop() else {
            unreachable!("validated i32 control operand");
        };
        value
    }

    fn pop_transfer(&mut self) -> Transfer {
        let size = self.pop_i32() as u32;
        let source = self.pop_i32() as u32;
        let destination = self.pop_i32() as u32;
        Transfer {
            destination,
            source,
            size,
        }
    }
}

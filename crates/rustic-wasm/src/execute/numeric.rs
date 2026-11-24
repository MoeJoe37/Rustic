use super::{Fault, Slot, Value, effect};
use crate::{Immediate, Opcode};

macro_rules! read_number {
    ($name:ident, $variant:ident, $ty:ty) => {
        fn $name(value: Value) -> $ty {
            let Value::$variant(number) = value else {
                unreachable!("validated numeric operand");
            };
            number
        }
    };
}

read_number!(i32_value, I32, i32);
read_number!(i64_value, I64, i64);
read_number!(f32_bits, F32, u32);
read_number!(f64_bits, F64, u64);

fn f32_value(value: Value) -> f32 {
    f32::from_bits(f32_bits(value))
}

fn f64_value(value: Value) -> f64 {
    f64::from_bits(f64_bits(value))
}

fn float32(value: f32) -> Value {
    Value::F32(if value.is_nan() {
        0x7fc0_0000
    } else {
        value.to_bits()
    })
}

fn float64(value: f64) -> Value {
    Value::F64(if value.is_nan() {
        0x7ff8_0000_0000_0000
    } else {
        value.to_bits()
    })
}

macro_rules! comparisons {
    ($op:expr, $a:expr, $b:expr; $( $variant:literal => $test:expr ),+ $(,)?) => {{
        let a = $a;
        let b = $b;
        Value::I32(i32::from(match $op {
            0 => a == b,
            1 => a != b,
            2 => a < b,
            $( $variant => $test(a, b), )+
            _ => unreachable!("known comparison opcode"),
        }))
    }};
}

macro_rules! int_compare {
    ($op:expr, $a:expr, $b:expr, $unsigned:ty) => {
        comparisons!($op, $a, $b;
            3 => |a, b| (a as $unsigned) < (b as $unsigned),
            4 => |a, b| a > b,
            5 => |a, b| (a as $unsigned) > (b as $unsigned),
            6 => |a, b| a <= b,
            7 => |a, b| (a as $unsigned) <= (b as $unsigned),
            8 => |a, b| a >= b,
            9 => |a, b| (a as $unsigned) >= (b as $unsigned),
        )
    };
}

macro_rules! int_ops {
    ($op:expr, $a:expr, $b:expr, $signed:ty, $unsigned:ty) => {{
        let (a, b) = ($a, $b);
        if matches!($op, 3..=6) && b == 0 {
            return Err(Fault::IntegerDivideByZero);
        }
        match $op {
            0 => a.wrapping_add(b),
            1 => a.wrapping_sub(b),
            2 => a.wrapping_mul(b),
            3 => a.checked_div(b).ok_or(Fault::IntegerOverflow)?,
            4 => ((a as $unsigned) / (b as $unsigned)) as $signed,
            5 => a.wrapping_rem(b),
            6 => ((a as $unsigned) % (b as $unsigned)) as $signed,
            7 => a & b,
            8 => a | b,
            9 => a ^ b,
            10 => a.wrapping_shl(b as u32),
            11 => a.wrapping_shr(b as u32),
            12 => (a as $unsigned).wrapping_shr(b as u32) as $signed,
            13 => a.rotate_left(b as u32),
            14 => a.rotate_right(b as u32),
            _ => unreachable!("known integer opcode"),
        }
    }};
}

macro_rules! float_ops {
    ($op:expr, $a:expr, $b:expr, $wrap:ident, $variant:ident, $float:ty, $sign:expr) => {{
        let a = $a;
        match $op {
            0 => Value::$variant(a.to_bits() & !$sign),
            1 => Value::$variant(a.to_bits() ^ $sign),
            2 => $wrap(a.ceil()),
            3 => $wrap(a.floor()),
            4 => $wrap(a.trunc()),
            5 => $wrap(a.round_ties_even()),
            6 => $wrap(a.sqrt()),
            op => {
                let b = $b;
                match op {
                    7 => $wrap(a + b),
                    8 => $wrap(a - b),
                    9 => $wrap(a * b),
                    10 => $wrap(a / b),
                    11 | 12 if a.is_nan() || b.is_nan() => $wrap(<$float>::NAN),
                    11 | 12 if a == b => Value::$variant(if op == 11 {
                        a.to_bits() | b.to_bits()
                    } else {
                        a.to_bits() & b.to_bits()
                    }),
                    11 => $wrap(a.min(b)),
                    12 => $wrap(a.max(b)),
                    13 => Value::$variant((a.to_bits() & !$sign) | (b.to_bits() & $sign)),
                    _ => unreachable!("known floating-point opcode"),
                }
            }
        }
    }};
}

pub(super) fn apply(
    op: Opcode,
    immediate: &Immediate,
    operands: &mut Vec<Slot>,
) -> Result<Value, Fault> {
    let (params, _) = effect(op).ok_or(Fault::UnsupportedInstruction)?;
    let start = operands.len() - params.len();
    let mut args = [Value::I32(0); 2];
    for (argument, &slot) in args.iter_mut().zip(&operands[start..]) {
        *argument = slot.number();
    }
    let result = evaluate(op, immediate, &args[..params.len()])?;
    operands.truncate(start);
    Ok(result)
}

fn evaluate(op: Opcode, imm: &Immediate, args: &[Value]) -> Result<Value, Fault> {
    use Value::{F32, F64, I32, I64};

    let value = match (op, imm) {
        (Opcode::Core(0x41), Immediate::I32(value)) => I32(*value),
        (Opcode::Core(0x42), Immediate::I64(value)) => I64(*value),
        (Opcode::Core(0x43), Immediate::F32(value)) => F32(*value),
        (Opcode::Core(0x44), Immediate::F64(value)) => F64(*value),
        (Opcode::Core(0x45), _) => I32(i32::from(i32_value(args[0]) == 0)),
        (Opcode::Core(code @ 0x46..=0x4f), _) => {
            int_compare!(code - 0x46, i32_value(args[0]), i32_value(args[1]), u32)
        }
        (Opcode::Core(0x50), _) => I32(i32::from(i64_value(args[0]) == 0)),
        (Opcode::Core(code @ 0x51..=0x5a), _) => {
            int_compare!(code - 0x51, i64_value(args[0]), i64_value(args[1]), u64)
        }
        (Opcode::Core(code @ 0x5b..=0x60), _) => comparisons!(
            code - 0x5b, f32_value(args[0]), f32_value(args[1]);
            3 => |a, b| a > b, 4 => |a, b| a <= b, 5 => |a, b| a >= b,
        ),
        (Opcode::Core(code @ 0x61..=0x66), _) => comparisons!(
            code - 0x61, f64_value(args[0]), f64_value(args[1]);
            3 => |a, b| a > b, 4 => |a, b| a <= b, 5 => |a, b| a >= b,
        ),
        (Opcode::Core(0x67), _) => I32(i32_value(args[0]).leading_zeros() as i32),
        (Opcode::Core(0x68), _) => I32(i32_value(args[0]).trailing_zeros() as i32),
        (Opcode::Core(0x69), _) => I32(i32_value(args[0]).count_ones() as i32),
        (Opcode::Core(code @ 0x6a..=0x78), _) => I32(int_ops!(
            code - 0x6a,
            i32_value(args[0]),
            i32_value(args[1]),
            i32,
            u32
        )),
        (Opcode::Core(0x79), _) => I64(i64::from(i64_value(args[0]).leading_zeros())),
        (Opcode::Core(0x7a), _) => I64(i64::from(i64_value(args[0]).trailing_zeros())),
        (Opcode::Core(0x7b), _) => I64(i64::from(i64_value(args[0]).count_ones())),
        (Opcode::Core(code @ 0x7c..=0x8a), _) => I64(int_ops!(
            code - 0x7c,
            i64_value(args[0]),
            i64_value(args[1]),
            i64,
            u64
        )),
        (Opcode::Core(code @ 0x8b..=0x98), _) => float_ops!(
            code - 0x8b,
            f32_value(args[0]),
            f32_value(args[1]),
            float32,
            F32,
            f32,
            1u32 << 31
        ),
        (Opcode::Core(code @ 0x99..=0xa6), _) => float_ops!(
            code - 0x99,
            f64_value(args[0]),
            f64_value(args[1]),
            float64,
            F64,
            f64,
            1u64 << 63
        ),
        (Opcode::Core(code @ (0xa8..=0xab | 0xae..=0xb1)), _) => {
            let conversion = if code <= 0xab {
                code - 0xa8
            } else {
                code - 0xae + 4
            };
            truncate(conversion, args[0], false)?
        }
        (Opcode::Misc(code @ 0..=7), _) => truncate(code as u8, args[0], true)?,
        (Opcode::Core(0xa7), _) => I32(i64_value(args[0]) as i32),
        (Opcode::Core(0xac), _) => I64(i64::from(i32_value(args[0]))),
        (Opcode::Core(0xad), _) => I64(i64::from(i32_value(args[0]) as u32)),
        (Opcode::Core(0xb2), _) => float32(i32_value(args[0]) as f32),
        (Opcode::Core(0xb3), _) => float32(i32_value(args[0]) as u32 as f32),
        (Opcode::Core(0xb4), _) => float32(i64_value(args[0]) as f32),
        (Opcode::Core(0xb5), _) => float32(i64_value(args[0]) as u64 as f32),
        (Opcode::Core(0xb6), _) => float32(f64_value(args[0]) as f32),
        (Opcode::Core(0xb7), _) => float64(f64::from(i32_value(args[0]))),
        (Opcode::Core(0xb8), _) => float64(f64::from(i32_value(args[0]) as u32)),
        (Opcode::Core(0xb9), _) => float64(i64_value(args[0]) as f64),
        (Opcode::Core(0xba), _) => float64(i64_value(args[0]) as u64 as f64),
        (Opcode::Core(0xbb), _) => float64(f64::from(f32_value(args[0]))),
        (Opcode::Core(0xbc), _) => I32(f32_bits(args[0]) as i32),
        (Opcode::Core(0xbd), _) => I64(f64_bits(args[0]) as i64),
        (Opcode::Core(0xbe), _) => F32(i32_value(args[0]) as u32),
        (Opcode::Core(0xbf), _) => F64(i64_value(args[0]) as u64),
        (Opcode::Core(0xc0), _) => I32(i32::from(i32_value(args[0]) as i8)),
        (Opcode::Core(0xc1), _) => I32(i32::from(i32_value(args[0]) as i16)),
        (Opcode::Core(0xc2), _) => I64(i64::from(i64_value(args[0]) as i8)),
        (Opcode::Core(0xc3), _) => I64(i64::from(i64_value(args[0]) as i16)),
        (Opcode::Core(0xc4), _) => I64(i64::from(i64_value(args[0]) as i32)),
        _ => unreachable!("prepared numeric instruction and immediate"),
    };
    Ok(value)
}

fn truncate(conversion: u8, value: Value, saturating: bool) -> Result<Value, Fault> {
    let float = if conversion & 2 == 0 {
        f64::from(f32_value(value))
    } else {
        f64_value(value)
    };
    let integral = float.trunc();
    if !saturating {
        let (minimum, exclusive_maximum) = match conversion {
            0 | 2 => (-(1i64 << 31) as f64, (1u64 << 31) as f64),
            1 | 3 => (0.0, (1u64 << 32) as f64),
            4 | 6 => (i64::MIN as f64, (1u64 << 63) as f64),
            _ => (0.0, 18_446_744_073_709_551_616.0), // 2^64, exactly representable.
        };
        if integral.is_nan() {
            return Err(Fault::InvalidConversionToInteger);
        }
        if integral < minimum || integral >= exclusive_maximum {
            return Err(Fault::IntegerOverflow);
        }
    }
    Ok(match conversion {
        0 | 2 => Value::I32(integral as i32),
        1 | 3 => Value::I32(integral as u32 as i32),
        4 | 6 => Value::I64(integral as i64),
        _ => Value::I64(integral as u64 as i64),
    })
}

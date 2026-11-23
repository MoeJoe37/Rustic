use crate::{Opcode, ValueType as Ty};
use Ty::{F32, F64, I32, I64};

pub(super) fn effect(op: Opcode) -> Option<(&'static [Ty], Ty)> {
    Some(match op {
        Opcode::Core(0x41) => (&[], I32),
        Opcode::Core(0x42) => (&[], I64),
        Opcode::Core(0x43) => (&[], F32),
        Opcode::Core(0x44) => (&[], F64),
        Opcode::Core(0x45 | 0x67..=0x69 | 0xc0..=0xc1) => (&[I32], I32),
        Opcode::Core(0x46..=0x4f | 0x6a..=0x78) => (&[I32; 2], I32),
        Opcode::Core(0x50 | 0xa7) => (&[I64], I32),
        Opcode::Core(0x51..=0x5a) => (&[I64; 2], I32),
        Opcode::Core(0x5b..=0x60) => (&[F32; 2], I32),
        Opcode::Core(0x61..=0x66) => (&[F64; 2], I32),
        Opcode::Core(0x79..=0x7b | 0xc2..=0xc4) => (&[I64], I64),
        Opcode::Core(0x7c..=0x8a) => (&[I64; 2], I64),
        Opcode::Core(0x8b..=0x91) => (&[F32], F32),
        Opcode::Core(0x92..=0x98) => (&[F32; 2], F32),
        Opcode::Core(0x99..=0x9f) => (&[F64], F64),
        Opcode::Core(0xa0..=0xa6) => (&[F64; 2], F64),
        Opcode::Core(0xa8..=0xa9 | 0xbc) | Opcode::Misc(0..=1) => (&[F32], I32),
        Opcode::Core(0xaa..=0xab) | Opcode::Misc(2..=3) => (&[F64], I32),
        Opcode::Core(0xac..=0xad) => (&[I32], I64),
        Opcode::Core(0xae..=0xaf) | Opcode::Misc(4..=5) => (&[F32], I64),
        Opcode::Core(0xb0..=0xb1 | 0xbd) | Opcode::Misc(6..=7) => (&[F64], I64),
        Opcode::Core(0xb2..=0xb3 | 0xbe) => (&[I32], F32),
        Opcode::Core(0xb4..=0xb5) => (&[I64], F32),
        Opcode::Core(0xb6) => (&[F64], F32),
        Opcode::Core(0xb7..=0xb8) => (&[I32], F64),
        Opcode::Core(0xb9..=0xba | 0xbf) => (&[I64], F64),
        Opcode::Core(0xbb) => (&[F32], F64),
        _ => return None,
    })
}

/// Loaded or stored value type and natural alignment exponent.
pub(super) fn memory_access(op: u8) -> (Ty, u8) {
    match op {
        0x28 | 0x36 => (I32, 2),
        0x29 | 0x37 => (I64, 3),
        0x2a | 0x38 => (F32, 2),
        0x2b | 0x39 => (F64, 3),
        0x2c..=0x2d | 0x3a => (I32, 0),
        0x2e..=0x2f | 0x3b => (I32, 1),
        0x30..=0x31 | 0x3c => (I64, 0),
        0x32..=0x33 | 0x3d => (I64, 1),
        0x34..=0x35 | 0x3e => (I64, 2),
        _ => unreachable!("decoder supplied a load or store opcode"),
    }
}

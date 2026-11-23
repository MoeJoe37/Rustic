mod common;

use common::{decode, module, short_len};
use rustic_base::CancellationToken;
use rustic_wasm::{
    ExecutionErrorKind as Fault, ExecutionLimits, NumericInstance, ValidatedModule, Value,
    validate_module,
};

fn program(types: &[u8], functions: &[(u8, &[u8], &[u8])], start: Option<u8>) -> Vec<u8> {
    let mut definitions = vec![functions.len().try_into().unwrap()];
    let mut bodies = definitions.clone();
    for (ty, locals, code) in functions {
        definitions.push(*ty);
        let body = [*locals, *code, &[0x0b]].concat();
        bodies.push(short_len(&body));
        bodies.extend(body);
    }
    let start = start.map(|index| [index]);
    let mut sections = vec![
        (1, types),
        (3, definitions.as_slice()),
        (7, b"\x01\x03run\0\0"),
    ];
    if let Some(index) = &start {
        sections.push((8, index));
    }
    sections.push((10, &bodies));
    module(&sections)
}

fn validated(bytes: &[u8]) -> ValidatedModule<'_> {
    validate_module(
        decode(bytes).unwrap(),
        Default::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}

#[test]
fn recursive_direct_calls_keep_locals_and_temporary_operands_separate() {
    let ty = &[1, 0x60, 1, 0x7f, 1, 0x7f];
    #[rustfmt::skip]
    let fibonacci = [
        0x20, 0, 0x41, 1, 0x4c, 0x04, 0x7f,
            0x20, 0,
        0x05,
            0x20, 0, 0x41, 1, 0x6b, 0x10, 0,
            0x20, 0, 0x41, 2, 0x6b, 0x10, 0,
            0x6a,
        0x0b,
    ];
    let bytes = program(ty, &[(0, &[0], &fibonacci)], None);
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let instance = NumericInstance::instantiate(&module, Default::default(), &token).unwrap();
    assert_eq!(
        instance.invoke("run", &[Value::I32(10)], &token).unwrap(),
        [Value::I32(55)]
    );
    assert_eq!(
        instance.invoke("missing", &[], &token).unwrap_err().kind,
        Fault::UnknownExport
    );
    assert_eq!(
        instance
            .invoke("run", &[Value::I64(10)], &token)
            .unwrap_err()
            .kind,
        Fault::ArgumentType
    );

    let limits = ExecutionLimits {
        calls: 4,
        ..ExecutionLimits::default()
    };
    let instance = NumericInstance::instantiate(&module, limits, &token).unwrap();
    assert_eq!(
        instance
            .invoke("run", &[Value::I32(10)], &token)
            .unwrap_err()
            .kind,
        Fault::LimitExceeded
    );
    assert_eq!(
        instance.invoke("run", &[Value::I32(3)], &token).unwrap(),
        [Value::I32(2)]
    );
}

#[test]
fn direct_tail_calls_reuse_the_call_slot_even_inside_active_blocks() {
    let ty = &[1, 0x60, 2, 0x7f, 0x7f, 1, 0x7f];
    #[rustfmt::skip]
    let sum = [
        0x20, 0, 0x45, 0x04, 0x40,
            0x20, 1, 0x0f,
        0x0b,
        0x02, 0x40,
            0x41, 8, // Dropped when the tail call replaces the frame.
            0x20, 0, 0x41, 1, 0x6b,
            0x20, 0, 0x20, 1, 0x6a,
            0x12, 0,
        0x0b, 0x00,
    ];
    let bytes = program(ty, &[(0, &[0], &sum)], None);
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let limits = ExecutionLimits {
        calls: 1,
        ..ExecutionLimits::default()
    };
    let instance = NumericInstance::instantiate(&module, limits, &token).unwrap();
    assert_eq!(
        instance
            .invoke("run", &[Value::I32(5000), Value::I32(0)], &token)
            .unwrap(),
        [Value::I32(12_502_500)],
    );
}

#[test]
fn branches_preserve_multi_value_arguments_and_choose_unsigned_table_indexes() {
    let ty = &[2, 0x60, 1, 0x7f, 2, 0x7f, 0x7e, 0x60, 0, 2, 0x7f, 0x7e];
    #[rustfmt::skip]
    let choose = [
        0x02, 1,
            0x02, 1,
                0x41, 1, 0x42, 2, 0x20, 0, 0x0e, 1, 0, 1,
            0x0b,
            0x1a, 0x1a, 0x41, 3, 0x42, 4,
        0x0b,
    ];
    let bytes = program(ty, &[(0, &[0], &choose)], None);
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let instance = NumericInstance::instantiate(&module, Default::default(), &token).unwrap();
    assert_eq!(
        instance.invoke("run", &[Value::I32(0)], &token).unwrap(),
        [Value::I32(3), Value::I64(4)]
    );
    for index in [1, -1] {
        assert_eq!(
            instance
                .invoke("run", &[Value::I32(index)], &token)
                .unwrap(),
            [Value::I32(1), Value::I64(2)]
        );
    }

    let ty = &[2, 0x60, 1, 0x7f, 1, 0x7f, 0x60, 1, 0x7f, 1, 0x7f];
    #[rustfmt::skip]
    let count = [
        0x20, 0, 0x03, 1,
            0x41, 1, 0x6b, 0x22, 0, 0x20, 0, 0x0d, 0,
        0x0b,
    ];
    let bytes = program(ty, &[(0, &[0], &count)], None);
    let module = validated(&bytes);
    let instance = NumericInstance::instantiate(&module, Default::default(), &token).unwrap();
    assert_eq!(
        instance.invoke("run", &[Value::I32(42)], &token).unwrap(),
        [Value::I32(0)]
    );
}

#[test]
fn numeric_evaluation_preserves_bits_and_traps_at_wasm_boundaries() {
    use Value::{F32, F64, I32, I64};
    type NumericCase<'a> = (u8, &'a [u8], &'a [Value], Result<Value, Fault>);
    #[rustfmt::skip]
    let cases: &[NumericCase<'_>] = &[
        (0x7f, &[0x6a], &[I32(i32::MAX), I32(1)], Ok(I32(i32::MIN))),
        (0x7e, &[0x86], &[I64(1), I64(65)], Ok(I64(2))),
        (0x7f, &[0x6d], &[I32(i32::MIN), I32(-1)], Err(Fault::IntegerOverflow)),
        (0x7e, &[0x81], &[I64(i64::MIN), I64(-1)], Ok(I64(0))),
        (0x7e, &[0x80], &[I64(-1), I64(0)], Err(Fault::IntegerDivideByZero)),
        (0x7f, &[0x49], &[I32(0), I32(-1)], Ok(I32(1))),
        (0x7c, &[0x9e], &[F64((-0.5f64).to_bits())], Ok(F64((-0.0f64).to_bits()))),
        (0x7c, &[0x9e], &[F64(2.5f64.to_bits())], Ok(F64(2.0f64.to_bits()))),
        (0x7d, &[0x96], &[F32(0), F32(0x8000_0000)], Ok(F32(0x8000_0000))),
        (0x7d, &[0x97], &[F32(0x8000_0000), F32(0)], Ok(F32(0))),
        (0x7d, &[0x92], &[F32(0), F32(0x7f80_0001)], Ok(F32(0x7fc0_0000))),
        (0x7f, &[0xbc], &[F32(0xff80_0001)], Ok(I32(0xff80_0001u32 as i32))),
        (0x7f, &[0xab], &[F64((-0.9f64).to_bits())], Ok(I32(0))),
        (0x7e, &[0xb0], &[F64(9_223_372_036_854_775_808f64.to_bits())], Err(Fault::IntegerOverflow)),
        (0x7f, &[0xaa], &[F64(f64::NAN.to_bits())], Err(Fault::InvalidConversionToInteger)),
        (0x7e, &[0xfc, 7], &[F64(f64::INFINITY.to_bits())], Ok(I64(-1))),
    ];
    let token = CancellationToken::default();
    for &(ty, operation, arguments, expected) in cases {
        let mut signature = vec![1, 0x60, arguments.len().try_into().unwrap()];
        let mut code = Vec::new();
        for (index, arg) in arguments.iter().enumerate() {
            signature.push(match arg {
                I32(_) => 0x7f,
                I64(_) => 0x7e,
                F32(_) => 0x7d,
                F64(_) => 0x7c,
            });
            code.extend([0x20, index.try_into().unwrap()]);
        }
        signature.extend([1, ty]);
        code.extend(operation);
        let bytes = program(&signature, &[(0, &[0], &code)], None);
        let module = validated(&bytes);
        let instance = NumericInstance::instantiate(&module, Default::default(), &token).unwrap();
        let result = instance.invoke("run", arguments, &token);
        if let Err(error) = &result {
            assert_eq!(bytes[error.offset], operation[0]);
        }
        assert_eq!(result.map(|values| values[0]).map_err(|e| e.kind), expected);
    }
}

#[test]
fn work_stack_limits_cancellation_and_start_traps_are_enforced() {
    let ty = &[1, 0x60, 0, 0];
    let bytes = program(ty, &[(0, &[0], &[0x03, 0x40, 0x0c, 0, 0x0b])], None);
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let limits = ExecutionLimits {
        invocation_work: 20,
        ..ExecutionLimits::default()
    };
    let instance = NumericInstance::instantiate(&module, limits, &token).unwrap();
    assert_eq!(
        instance.invoke("run", &[], &token).unwrap_err().kind,
        Fault::LimitExceeded
    );
    let limits = ExecutionLimits {
        control_frames: 1,
        ..ExecutionLimits::default()
    };
    let instance = NumericInstance::instantiate(&module, limits, &token).unwrap();
    let error = instance.invoke("run", &[], &token).unwrap_err();
    assert_eq!(error.kind, Fault::LimitExceeded);
    assert_eq!(bytes[error.offset], 0x03);
    let limits = ExecutionLimits {
        preparation_work: 2,
        ..ExecutionLimits::default()
    };
    assert_eq!(
        NumericInstance::instantiate(&module, limits, &token)
            .err()
            .unwrap()
            .kind,
        Fault::LimitExceeded
    );

    let bytes = program(ty, &[(0, &[1, 50, 0x7f], &[])], None);
    let module = validated(&bytes);
    let limits = ExecutionLimits {
        values: 49,
        ..ExecutionLimits::default()
    };
    let instance = NumericInstance::instantiate(&module, limits, &token).unwrap();
    assert_eq!(
        instance.invoke("run", &[], &token).unwrap_err().kind,
        Fault::LimitExceeded
    );
    token.cancel();
    assert_eq!(
        instance.invoke("run", &[], &token).unwrap_err().kind,
        Fault::Cancelled
    );

    let bytes = program(ty, &[(0, &[0], &[]), (0, &[0], &[0])], Some(1));
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let error = NumericInstance::instantiate(&module, Default::default(), &token)
        .err()
        .unwrap();
    assert_eq!(error.kind, Fault::Unreachable);
    assert_eq!(bytes[error.offset], 0);
}

#[test]
fn preparation_is_explicit_about_its_supported_profile() {
    let bytes = module(&[(5, &[1, 0, 1])]);
    let memory = validated(&bytes);
    let bytes = program(&[1, 0x60, 0, 0], &[(0, &[0], &[0xd0, 0x70, 0x1a])], None);
    let references = validated(&bytes);
    let token = CancellationToken::default();
    for (module, kind) in [
        (memory, Fault::UnsupportedModule),
        (references, Fault::UnsupportedInstruction),
    ] {
        assert_eq!(
            NumericInstance::instantiate(&module, Default::default(), &token)
                .err()
                .unwrap()
                .kind,
            kind
        );
    }
}

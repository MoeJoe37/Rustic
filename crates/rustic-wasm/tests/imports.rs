mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use common::{decode, module, short_len};
use rustic_base::CancellationToken;
use rustic_wasm::{
    ExecutionErrorKind as Fault, ExecutionLimits, FunctionImport, FunctionType, HostTrap, Instance,
    InstanceLimits, ValidatedModule, Value, ValueType, validate_module,
};

fn fixture() -> Vec<u8> {
    let types = &[2, 0x60, 2, 0x7f, 0x7e, 2, 0x7e, 0x7f, 0x60, 0, 0];
    let imports = b"\x02\x03sys\x08exchange\0\0\x03sys\x07started\0\x01";
    let mut exports = vec![5];
    for (name, index) in [
        (&b"host"[..], 0),
        (&b"direct"[..], 2),
        (&b"indirect"[..], 3),
        (&b"tail"[..], 4),
        (&b"indirect-tail"[..], 5),
    ] {
        exports.push(short_len(name));
        exports.extend(name);
        exports.extend([0, index]);
    }
    let mut code = vec![4];
    for op in [
        &[0x10, 0][..],
        &[0x41, 0, 0x11, 0, 0],
        &[0x12, 0],
        &[0x41, 0, 0x13, 0, 0],
    ] {
        let body = [&[0, 0x20, 0, 0x20, 1][..], op, &[0x0b]].concat();
        code.push(short_len(&body));
        code.extend(body);
    }
    module(&[
        (1, types),
        (2, imports),
        (3, &[4, 0, 0, 0, 0]),
        (4, &[1, 0x70, 0, 1]),
        (7, &exports),
        (8, &[1]),
        (9, &[1, 0, 0x41, 0, 0x0b, 1, 0]),
        (10, &code),
    ])
}

fn validated(bytes: &[u8]) -> ValidatedModule<'_> {
    validate_module(
        decode(bytes).unwrap(),
        Default::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}

fn exchange_signature() -> FunctionType {
    FunctionType {
        params: vec![ValueType::I32, ValueType::I64],
        results: vec![ValueType::I64, ValueType::I32],
    }
}

static EMPTY_SIGNATURE: FunctionType = FunctionType {
    params: Vec::new(),
    results: Vec::new(),
};

fn exchange(args: &[Value], results: &mut [Value], _: &CancellationToken) -> Result<(), HostTrap> {
    assert_eq!(results, [Value::I64(0), Value::I32(0)]);
    results.copy_from_slice(&[args[1], args[0]]);
    Ok(())
}

#[test]
fn an_import_can_be_exported_called_and_stored_in_a_table() {
    let bytes = fixture();
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let calls = AtomicUsize::new(0);
    let record = |args: &[Value], results: &mut [Value], _: &CancellationToken| {
        assert!(args.is_empty() && results.is_empty());
        calls.fetch_add(1, Ordering::Relaxed);
        Ok(())
    };
    let signature = exchange_signature();
    let bindings = [
        FunctionImport::new("sys", "started", &EMPTY_SIGNATURE, &record),
        FunctionImport::new("sys", "exchange", &signature, &exchange),
    ];
    let mut instance =
        Instance::instantiate_with_imports(&module, Default::default(), &bindings, &token).unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    for name in ["host", "direct", "indirect", "tail", "indirect-tail"] {
        assert_eq!(
            instance
                .invoke(name, &[Value::I32(5), Value::I64(-7)], &token)
                .unwrap(),
            [Value::I64(-7), Value::I32(5)],
        );
    }
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn binding_failures_precede_the_start_callback_and_have_a_source_offset() {
    let bytes = fixture();
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let signature = exchange_signature();
    let never = |_: &[Value], _: &mut [Value], _: &CancellationToken| -> Result<(), HostTrap> {
        panic!("start must not run")
    };
    for (bindings, kind) in [
        (
            vec![FunctionImport::new(
                "sys",
                "started",
                &EMPTY_SIGNATURE,
                &never,
            )],
            Fault::MissingImport,
        ),
        (
            vec![
                FunctionImport::new("sys", "exchange", &signature, &exchange),
                FunctionImport::new("sys", "started", &signature, &never),
            ],
            Fault::ImportTypeMismatch,
        ),
        (
            vec![
                FunctionImport::new("sys", "exchange", &signature, &exchange),
                FunctionImport::new("sys", "exchange", &signature, &exchange),
            ],
            Fault::DuplicateImportBinding,
        ),
    ] {
        let error =
            Instance::instantiate_with_imports(&module, Default::default(), &bindings, &token)
                .err()
                .unwrap();
        assert_eq!(error.kind, kind);
        if kind == Fault::DuplicateImportBinding {
            assert_eq!(error.offset, 0);
        } else {
            assert_eq!(&bytes[error.offset..error.offset + 4], b"\x03sys");
        }
    }
    assert_eq!(
        Instance::instantiate(&module, Default::default(), &token)
            .err()
            .unwrap()
            .kind,
        Fault::UnsupportedModule
    );

    let imported_table = validated(b"\0asm\x01\0\0\0\x02\x09\x01\x01m\x01t\x01\x70\0\0");
    let error =
        Instance::instantiate_with_imports(&imported_table, Default::default(), &[], &token)
            .err()
            .unwrap();
    assert_eq!(error.kind, Fault::UnsupportedModule);
}

#[test]
fn callbacks_observe_cancellation_and_cannot_put_mistyped_values_on_the_stack() {
    let bytes = fixture();
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let signature = exchange_signature();
    let start = |_: &[Value], _: &mut [Value], _: &CancellationToken| Ok(());
    let callback = |args: &[Value], output: &mut [Value], token: &CancellationToken| {
        match args[0] {
            Value::I32(0) => return Err(HostTrap),
            Value::I32(1) => output[0] = Value::I32(123),
            Value::I32(2) => token.cancel(),
            _ => exchange(args, output, token)?,
        }
        Ok(())
    };
    let bindings = [
        FunctionImport::new("sys", "started", &EMPTY_SIGNATURE, &start),
        FunctionImport::new("sys", "exchange", &signature, &callback),
    ];
    let mut instance =
        Instance::instantiate_with_imports(&module, Default::default(), &bindings, &token).unwrap();
    for (arg, kind) in [
        (0, Fault::HostTrap),
        (1, Fault::HostResultType),
        (2, Fault::Cancelled),
    ] {
        let call_token = CancellationToken::default();
        let error = instance
            .invoke("direct", &[Value::I32(arg), Value::I64(9)], &call_token)
            .unwrap_err();
        assert_eq!(error.kind, kind);
        assert_eq!(bytes[error.offset], 0x10);
    }
    assert_eq!(
        instance
            .invoke("direct", &[Value::I32(3), Value::I64(9)], &token)
            .unwrap(),
        [Value::I64(9), Value::I32(3)]
    );

    let calls = AtomicUsize::new(0);
    let metered = |args: &[Value], output: &mut [Value], token: &CancellationToken| {
        calls.fetch_add(1, Ordering::Relaxed);
        exchange(args, output, token)
    };
    let bindings = [
        FunctionImport::new("sys", "started", &EMPTY_SIGNATURE, &start),
        FunctionImport::new("sys", "exchange", &signature, &metered),
    ];
    let limits = InstanceLimits {
        execution: ExecutionLimits {
            invocation_work: 11,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut instance =
        Instance::instantiate_with_imports(&module, limits, &bindings, &token).unwrap();
    assert_eq!(
        instance
            .invoke("host", &[Value::I32(3), Value::I64(9)], &token)
            .unwrap_err()
            .kind,
        Fault::LimitExceeded,
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

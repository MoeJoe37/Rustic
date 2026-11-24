mod common;

use common::{decode, module, short_len};
use rustic_base::CancellationToken;
use rustic_wasm::{
    ExecutionErrorKind as Fault, ExecutionLimits, Instance, InstanceLimits, ValidatedModule, Value,
    validate_module,
};

type Function<'a> = (&'a [u8], u8, &'a [u8], &'a [u8]);

fn program(
    types: &[u8],
    state: &[(u8, &[u8])],
    elements: &[u8],
    start: Option<u8>,
    functions: &[Function<'_>],
) -> Vec<u8> {
    let mut definitions = vec![functions.len().try_into().unwrap()];
    let mut exports = vec![0];
    let mut code = definitions.clone();
    for (index, (name, ty, locals, body)) in functions.iter().enumerate() {
        definitions.push(*ty);
        if !name.is_empty() {
            exports[0] += 1;
            exports.push(short_len(name));
            exports.extend(*name);
            exports.extend([0, index.try_into().unwrap()]);
        }
        let body = [*locals, *body, &[0x0b]].concat();
        code.push(short_len(&body));
        code.extend(body);
    }
    let mut sections = vec![(1, types), (3, definitions.as_slice())];
    sections.extend_from_slice(state);
    sections.push((7, &exports));
    let start = start.map(|index| [index]);
    if let Some(index) = &start {
        sections.push((8, index));
    }
    if !elements.is_empty() {
        sections.push((9, elements));
    }
    sections.push((10, &code));
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

fn number(instance: &mut Instance<'_, '_>, name: &str, args: &[i32]) -> i32 {
    let values = args.iter().copied().map(Value::I32).collect::<Vec<_>>();
    let values = instance
        .invoke(name, &values, &CancellationToken::default())
        .unwrap();
    let [Value::I32(value)] = values.as_slice() else {
        panic!("one i32 result expected");
    };
    *value
}

#[test]
fn indirect_calls_accept_equivalent_signatures_and_distinguish_table_traps() {
    let types = &[
        4, 0x60, 0, 1, 0x7f, 0x60, 0, 1, 0x7f, 0x60, 1, 0x7f, 1, 0x7f, 0x60, 0, 1, 0x7e,
    ];
    let tables = &[2, 0x70, 0, 3, 0x40, 0, 0x70, 0, 1, 0xd2, 1, 0x0b];
    let elements = &[1, 0, 0x41, 0, 0x0b, 2, 1, 2];
    let bytes = program(
        types,
        &[(4, tables)],
        elements,
        None,
        &[
            (b"call", 2, &[0], &[0x20, 0, 0x11, 0, 0]),
            (b"", 1, &[0], &[0x41, 42]),
            (b"", 3, &[0], &[0x42, 42]),
            (b"tail", 2, &[0], &[0x41, 99, 0x20, 0, 0x13, 0, 1]),
        ],
    );
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let mut instance = Instance::instantiate(&module, Default::default(), &token).unwrap();
    assert_eq!(number(&mut instance, "call", &[0]), 42);
    for (index, kind) in [
        (1, Fault::IndirectCallTypeMismatch),
        (2, Fault::NullReference),
        (3, Fault::TableOutOfBounds),
        (-1, Fault::TableOutOfBounds),
    ] {
        let error = instance
            .invoke("call", &[Value::I32(index)], &token)
            .unwrap_err();
        assert_eq!(error.kind, kind);
        assert_eq!(bytes[error.offset], 0x11);
    }

    let limits = InstanceLimits {
        execution: ExecutionLimits {
            calls: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut instance = Instance::instantiate(&module, limits, &token).unwrap();
    assert_eq!(number(&mut instance, "tail", &[0]), 42);
    assert_eq!(
        instance
            .invoke("call", &[Value::I32(0)], &token)
            .unwrap_err()
            .kind,
        Fault::LimitExceeded
    );
}

#[test]
fn table_updates_obey_segment_lifetimes_overlap_and_allocation_limits() {
    let types = &[
        4, 0x60, 0, 1, 0x7f, 0x60, 1, 0x7f, 1, 0x7f, 0x60, 3, 0x7f, 0x7f, 0x7f, 0, 0x60, 0, 0,
    ];
    let tables = &[2, 0x70, 1, 3, 5, 0x70, 0, 1];
    let elements = &[
        3, 1, 0, 2, 0, 1, // Passive funcref segment.
        0, 0x41, 0, 0x0b, 1, 1, // Active, already dropped.
        3, 0, 1, 0, // Declarative, already dropped.
    ];
    let bytes = program(
        types,
        &[(4, tables)],
        elements,
        None,
        &[
            (b"", 0, &[0], &[0x41, 9]),
            (b"", 0, &[0], &[0x41, 7]),
            (
                b"init",
                2,
                &[0],
                &[0x20, 0, 0x20, 1, 0x20, 2, 0xfc, 12, 0, 0],
            ),
            (
                b"active",
                2,
                &[0],
                &[0x20, 0, 0x20, 1, 0x20, 2, 0xfc, 12, 1, 0],
            ),
            (
                b"decl",
                2,
                &[0],
                &[0x20, 0, 0x20, 1, 0x20, 2, 0xfc, 12, 2, 0],
            ),
            (
                b"copy",
                2,
                &[0],
                &[0x20, 0, 0x20, 1, 0x20, 2, 0xfc, 14, 0, 0],
            ),
            (
                b"cross",
                2,
                &[0],
                &[0x20, 0, 0x20, 1, 0x20, 2, 0xfc, 14, 1, 0],
            ),
            (b"fill", 2, &[0], &[0x20, 0, 0xd2, 0, 0x20, 2, 0xfc, 17, 0]),
            (b"read", 1, &[0], &[0x20, 0, 0x11, 0, 0]),
            (b"read-other", 1, &[0], &[0x20, 0, 0x11, 0, 1]),
            (b"drop", 3, &[0], &[0xfc, 13, 0]),
            (b"grow", 1, &[0], &[0xd2, 0, 0x20, 0, 0xfc, 15, 0]),
            (b"size", 0, &[0], &[0xfc, 16, 0]),
        ],
    );
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let limits = InstanceLimits {
        table_elements: 6,
        ..Default::default()
    };
    let mut instance = Instance::instantiate(&module, limits, &token).unwrap();
    let mut other = Instance::instantiate(&module, limits, &token).unwrap();
    let triplet = |a, b, c| [Value::I32(a), Value::I32(b), Value::I32(c)];
    assert_eq!(number(&mut instance, "read", &[0]), 7);
    assert_eq!(number(&mut instance, "grow", &[0]), 3);
    assert_eq!(number(&mut instance, "grow", &[1]), -1);
    assert_eq!(number(&mut instance, "grow", &[-1]), -1);
    instance.invoke("init", &triplet(0, 0, 2), &token).unwrap();
    instance.invoke("copy", &triplet(1, 0, 2), &token).unwrap();
    assert_eq!(number(&mut instance, "read", &[1]), 9);
    assert_eq!(number(&mut instance, "read", &[2]), 7);
    instance.invoke("cross", &triplet(0, 2, 1), &token).unwrap();
    assert_eq!(number(&mut instance, "read-other", &[0]), 7);
    for (name, args) in [("copy", triplet(0, 2, 2)), ("fill", triplet(2, 0, 2))] {
        assert_eq!(
            instance.invoke(name, &args, &token).unwrap_err().kind,
            Fault::TableOutOfBounds
        );
    }
    assert_eq!(number(&mut instance, "read", &[2]), 7);
    instance.invoke("fill", &triplet(2, 0, 1), &token).unwrap();
    assert_eq!(number(&mut instance, "read", &[2]), 9);
    assert_eq!(number(&mut other, "read", &[0]), 7);

    instance.invoke("drop", &[], &token).unwrap();
    instance.invoke("drop", &[], &token).unwrap();
    for name in ["init", "active", "decl"] {
        instance.invoke(name, &triplet(3, 0, 0), &token).unwrap();
        assert_eq!(
            instance
                .invoke(name, &triplet(0, 1, 0), &token)
                .unwrap_err()
                .kind,
            Fault::TableOutOfBounds
        );
        assert_eq!(
            instance
                .invoke(name, &triplet(0, 0, 1), &token)
                .unwrap_err()
                .kind,
            Fault::TableOutOfBounds
        );
    }
    assert_eq!(number(&mut instance, "grow", &[2]), 3);
    assert_eq!(number(&mut instance, "size", &[]), 5);
    assert_eq!(number(&mut instance, "read", &[4]), 9);
    assert_eq!(number(&mut instance, "grow", &[1]), -1);

    let limited_work = InstanceLimits {
        execution: ExecutionLimits {
            invocation_work: 14,
            ..limits.execution
        },
        ..limits
    };
    let mut instance = Instance::instantiate(&module, limited_work, &token).unwrap();
    assert_eq!(
        instance
            .invoke("fill", &triplet(0, 0, 3), &token)
            .unwrap_err()
            .kind,
        Fault::LimitExceeded,
    );
    assert_eq!(number(&mut instance, "read", &[0]), 7);

    let too_small = InstanceLimits {
        table_elements: 5,
        ..limits
    };
    assert_eq!(
        Instance::instantiate(&module, too_small, &token)
            .err()
            .unwrap()
            .kind,
        Fault::LimitExceeded
    );
}

#[test]
fn reference_values_flow_inside_the_instance_and_numeric_calls_check_the_boundary() {
    let types = &[
        4, 0x60, 0, 1, 0x7f, 0x60, 1, 0x70, 1, 0x70, 0x60, 0, 1, 0x70, 0x60, 0, 0,
    ];
    let tables = &[2, 0x70, 0, 1, 0x6f, 0, 1];
    let globals = &[2, 0x70, 1, 0xd0, 0x70, 0x0b, 0x7f, 1, 0x41, 0, 0x0b];
    let bytes = program(
        types,
        &[(4, tables), (6, globals)],
        &[1, 0, 0x41, 0, 0x0b, 1, 0],
        Some(5),
        &[
            (b"answer", 0, &[0], &[0x41, 42]),
            (b"ref-param", 1, &[0], &[0x20, 0, 0xd4]),
            (b"funcref", 0, &[0], &[0x23, 0, 0x10, 1, 0xd1]),
            (
                b"externref",
                0,
                &[1, 1, 0x6f],
                &[0x41, 0, 0x20, 0, 0x26, 1, 0x41, 0, 0x25, 1, 0xd1],
            ),
            (b"ref-result", 2, &[0], &[0x41, 9, 0x24, 1, 0xd2, 0]),
            (b"", 3, &[0], &[0x41, 0, 0x25, 0, 0x24, 0]),
            (b"unchanged", 0, &[0], &[0x23, 1]),
            (b"trap", 0, &[1, 1, 0x70], &[0x20, 0, 0x10, 1, 0xd1]),
        ],
    );
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let mut instance = Instance::instantiate(&module, Default::default(), &token).unwrap();
    assert_eq!(number(&mut instance, "funcref", &[]), 0);
    assert_eq!(number(&mut instance, "externref", &[]), 1);
    assert_eq!(
        instance.invoke("ref-result", &[], &token).unwrap_err().kind,
        Fault::ResultType
    );
    assert_eq!(
        instance
            .invoke("ref-param", &[Value::I32(0)], &token)
            .unwrap_err()
            .kind,
        Fault::ArgumentType,
    );
    assert_eq!(number(&mut instance, "unchanged", &[]), 0);
    let error = instance.invoke("trap", &[], &token).unwrap_err();
    assert_eq!(error.kind, Fault::NullReference);
    assert_eq!(bytes[error.offset], 0xd4);
}

#[test]
fn active_segment_bounds_and_preparation_limits_are_checked_before_start() {
    let tables = &[1, 0x70, 0, 1];
    let bad = module(&[(4, tables), (9, &[1, 4, 0x41, 2, 0x0b, 0])]);
    let token = CancellationToken::default();
    assert_eq!(
        Instance::instantiate(&validated(&bad), Default::default(), &token)
            .err()
            .unwrap()
            .kind,
        Fault::TableOutOfBounds
    );

    let large = module(&[(4, &[1, 0x70, 0, 0x80, 0x08])]);
    let module = validated(&large);
    let limits = InstanceLimits {
        execution: ExecutionLimits {
            preparation_work: 16,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        Instance::instantiate(&module, limits, &token)
            .err()
            .unwrap()
            .kind,
        Fault::LimitExceeded
    );
    token.cancel();
    assert_eq!(
        Instance::instantiate(&module, Default::default(), &token)
            .err()
            .unwrap()
            .kind,
        Fault::Cancelled
    );
}

mod common;

use common::{decode, module, short_len};
use rustic_base::CancellationToken;
use rustic_wasm::{
    ExecutionErrorKind as Fault, ExecutionLimits, Instance, InstanceLimits, ValidatedModule, Value,
    validate_module,
};

const PAGE: usize = 65_536;

fn validated(bytes: &[u8]) -> ValidatedModule<'_> {
    validate_module(
        decode(bytes).unwrap(),
        Default::default(),
        &CancellationToken::default(),
    )
    .unwrap()
}

struct Function<'a> {
    name: &'a str,
    ty: u8,
    code: &'a [u8],
}

fn program(
    types: &[u8],
    declarations: &[(u8, &[u8])],
    functions: &[Function<'_>],
    start: Option<u8>,
    data: Option<&[u8]>,
) -> Vec<u8> {
    let mut indexes = vec![functions.len().try_into().unwrap()];
    let mut body = indexes.clone();
    let mut exports = indexes.clone();
    for (index, function) in functions.iter().enumerate() {
        indexes.push(function.ty);
        exports.push(short_len(function.name.as_bytes()));
        exports.extend(function.name.bytes());
        exports.extend([0, index.try_into().unwrap()]);
        let code = [&[0], function.code, &[0x0b]].concat();
        body.push(short_len(&code));
        body.extend(code);
    }
    let mut sections = vec![(1, types), (3, indexes.as_slice())];
    sections.extend(declarations);
    sections.push((7, &exports));
    let start = start.map(|index| [index]);
    if let Some(index) = &start {
        sections.push((8, index));
    }
    let data_count = data.map(|bytes| [bytes[0]]);
    if let Some(count) = &data_count {
        sections.push((12, count));
    }
    sections.push((10, &body));
    if let Some(bytes) = data {
        sections.push((11, bytes));
    }
    module(&sections)
}

fn run(instance: &mut Instance<'_, '_>, name: &str, args: &[i32]) -> i32 {
    let args: Vec<_> = args.iter().copied().map(Value::I32).collect();
    let result = instance
        .invoke(name, &args, &CancellationToken::default())
        .unwrap();
    let [Value::I32(value)] = result.as_slice() else {
        panic!("expected one i32 result")
    };
    *value
}

#[test]
fn active_segments_and_start_run_before_exports_and_instances_are_independent() {
    let types = &[3, 0x60, 0, 1, 0x7f, 0x60, 1, 0x7f, 0, 0x60, 0, 0];
    let declarations = &[
        (5, &[1, 0, 1][..]),
        (
            6,
            &[
                2, 0x7f, 0, 0x41, 3, 0x0b, 0x7f, 1, 0x23, 0, 0x41, 4, 0x6a, 0x0b,
            ],
        ),
    ];
    let functions = &[
        Function {
            name: "get",
            ty: 0,
            code: &[0x23, 1],
        },
        // Changes made before the trap remain visible.
        Function {
            name: "set",
            ty: 1,
            code: &[0x20, 0, 0x24, 1, 0x00],
        },
        // Start adds the active segment's byte to the initialized global.
        Function {
            name: "start",
            ty: 2,
            code: &[0x23, 1, 0x23, 0, 0x2d, 0, 0, 0x6a, 0x24, 1],
        },
    ];
    let bytes = program(
        types,
        declarations,
        functions,
        Some(2),
        Some(&[1, 0, 0x23, 0, 0x0b, 1, 42]),
    );
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let mut first = Instance::instantiate(&module, Default::default(), &token).unwrap();
    let mut second = Instance::instantiate(&module, Default::default(), &token).unwrap();
    assert_eq!(run(&mut first, "get", &[]), 49);
    let error = first.invoke("set", &[Value::I32(11)], &token).unwrap_err();
    assert_eq!(error.kind, Fault::Unreachable);
    assert_eq!(run(&mut first, "get", &[]), 11);
    assert_eq!(run(&mut second, "get", &[]), 49);
}

#[test]
fn linear_memory_checks_unsigned_addresses_growth_and_little_endian_storage() {
    let types = &[
        3, 0x60, 1, 0x7f, 1, 0x7f, 0x60, 2, 0x7f, 0x7f, 0, 0x60, 0, 1, 0x7f,
    ];
    let functions = &[
        Function {
            name: "get",
            ty: 0,
            code: &[0x20, 0, 0x28, 0, 0],
        },
        Function {
            name: "set",
            ty: 1,
            code: &[0x20, 0, 0x20, 1, 0x36, 0, 0],
        },
        Function {
            name: "byte",
            ty: 0,
            code: &[0x20, 0, 0x2c, 0, 0],
        },
        Function {
            name: "size",
            ty: 2,
            code: &[0x3f, 0],
        },
        Function {
            name: "grow",
            ty: 0,
            code: &[0x20, 0, 0x40, 0],
        },
        Function {
            name: "off",
            ty: 0,
            code: &[0x20, 0, 0x28, 0, 0xff, 0xff, 0xff, 0xff, 0x0f],
        },
    ];
    let bytes = program(types, &[(5, &[1, 1, 1, 3])], functions, None, None);
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let limits = InstanceLimits {
        memory_bytes: 2 * PAGE,
        ..Default::default()
    };
    let mut instance = Instance::instantiate(&module, limits, &token).unwrap();
    assert_eq!(run(&mut instance, "size", &[]), 1);
    instance
        .invoke("set", &[Value::I32(1), Value::I32(0x1213_14ff)], &token)
        .unwrap();
    assert_eq!(run(&mut instance, "get", &[1]), 0x1213_14ff);
    assert_eq!(run(&mut instance, "byte", &[1]), -1);
    assert_eq!(run(&mut instance, "byte", &[2]), 0x14);
    let error = instance
        .invoke("get", &[Value::I32(PAGE as i32 - 3)], &token)
        .unwrap_err();
    assert_eq!(error.kind, Fault::MemoryOutOfBounds);
    assert_eq!(bytes[error.offset], 0x28);
    assert_eq!(
        instance
            .invoke("off", &[Value::I32(1)], &token)
            .unwrap_err()
            .kind,
        Fault::MemoryOutOfBounds
    );
    assert_eq!(
        instance
            .invoke("get", &[Value::I32(-1)], &token)
            .unwrap_err()
            .kind,
        Fault::MemoryOutOfBounds
    );
    assert_eq!(run(&mut instance, "grow", &[0]), 1);
    assert_eq!(run(&mut instance, "grow", &[1]), 1);
    assert_eq!(run(&mut instance, "get", &[PAGE as i32]), 0);
    assert_eq!(run(&mut instance, "grow", &[1]), -1);
    assert_eq!(run(&mut instance, "grow", &[-1]), -1);
    assert_eq!(run(&mut instance, "size", &[]), 2);

    let limited_work = InstanceLimits {
        execution: ExecutionLimits {
            invocation_work: 100,
            ..Default::default()
        },
        ..limits
    };
    let mut instance = Instance::instantiate(&module, limited_work, &token).unwrap();
    assert_eq!(
        instance
            .invoke("grow", &[Value::I32(1)], &token)
            .unwrap_err()
            .kind,
        Fault::LimitExceeded
    );
    assert_eq!(run(&mut instance, "size", &[]), 1);
}

#[test]
fn bulk_memory_checks_both_ranges_before_copy_and_drops_segments() {
    let types = &[
        3, 0x60, 3, 0x7f, 0x7f, 0x7f, 0, 0x60, 1, 0x7f, 1, 0x7f, 0x60, 0, 0,
    ];
    let functions = &[
        Function {
            name: "init",
            ty: 0,
            code: &[0x20, 0, 0x20, 1, 0x20, 2, 0xfc, 8, 0, 0],
        },
        Function {
            name: "copy",
            ty: 0,
            code: &[0x20, 0, 0x20, 1, 0x20, 2, 0xfc, 10, 0, 0],
        },
        Function {
            name: "cross",
            ty: 0,
            code: &[0x20, 0, 0x20, 1, 0x20, 2, 0xfc, 10, 1, 0],
        },
        Function {
            name: "fill",
            ty: 0,
            code: &[0x20, 0, 0x20, 1, 0x20, 2, 0xfc, 11, 0],
        },
        Function {
            name: "get",
            ty: 1,
            code: &[0x20, 0, 0x28, 0, 0],
        },
        Function {
            name: "other",
            ty: 1,
            code: &[0x20, 0, 0x28, 0x40, 1, 0],
        },
        Function {
            name: "drop",
            ty: 2,
            code: &[0xfc, 9, 0],
        },
    ];
    let bytes = program(
        types,
        &[(5, &[2, 0, 1, 0, 1])],
        functions,
        None,
        Some(&[1, 1, 4, 1, 2, 3, 4]),
    );
    let module = validated(&bytes);
    let token = CancellationToken::default();
    let mut instance = Instance::instantiate(&module, Default::default(), &token).unwrap();
    let triplet = |a, b, c| [Value::I32(a), Value::I32(b), Value::I32(c)];
    instance.invoke("init", &triplet(0, 0, 4), &token).unwrap();
    assert_eq!(run(&mut instance, "get", &[0]), 0x0403_0201);
    instance.invoke("copy", &triplet(1, 0, 3), &token).unwrap();
    assert_eq!(run(&mut instance, "get", &[0]), 0x0302_0101);
    instance.invoke("cross", &triplet(0, 0, 4), &token).unwrap();
    assert_eq!(run(&mut instance, "other", &[0]), 0x0302_0101);
    instance
        .invoke("fill", &triplet(0, 0x0105, 3), &token)
        .unwrap();
    assert_eq!(run(&mut instance, "get", &[0]), 0x0305_0505);
    assert_eq!(
        instance
            .invoke("copy", &triplet(0, PAGE as i32 - 1, 2), &token)
            .unwrap_err()
            .kind,
        Fault::MemoryOutOfBounds,
    );
    assert_eq!(run(&mut instance, "get", &[0]), 0x0305_0505);
    instance.invoke("drop", &[], &token).unwrap();
    instance.invoke("drop", &[], &token).unwrap();
    instance
        .invoke("init", &triplet(PAGE as i32, 0, 0), &token)
        .unwrap();
    for args in [
        triplet(0, 0, 1),
        triplet(0, 1, 0),
        triplet(PAGE as i32 + 1, 0, 0),
    ] {
        assert_eq!(
            instance.invoke("init", &args, &token).unwrap_err().kind,
            Fault::MemoryOutOfBounds
        );
    }
}

#[test]
fn initialization_checks_aggregate_allocation_bounds_and_segment_ends() {
    let token = CancellationToken::default();
    let bytes = module(&[(5, &[2, 0, 1, 0, 1])]);
    let two_memories = validated(&bytes);
    let limits = InstanceLimits {
        memory_bytes: PAGE,
        ..Default::default()
    };
    assert_eq!(
        Instance::instantiate(&two_memories, limits, &token)
            .err()
            .unwrap()
            .kind,
        Fault::LimitExceeded
    );
    let limits = InstanceLimits {
        execution: ExecutionLimits {
            preparation_work: PAGE,
            ..Default::default()
        },
        ..limits
    };
    assert_eq!(
        Instance::instantiate(&two_memories, limits, &token)
            .err()
            .unwrap()
            .kind,
        Fault::LimitExceeded
    );
    let bytes = module(&[
        (5, &[1, 0, 1]),
        (11, &[1, 0, 0x41, 0x80, 0x80, 0x04, 0x0b, 1, 42]),
    ]);
    let bad_data = validated(&bytes);
    assert_eq!(
        Instance::instantiate(&bad_data, Default::default(), &token)
            .err()
            .unwrap()
            .kind,
        Fault::MemoryOutOfBounds
    );
}

#[test]
fn owned_instances_outlive_the_source_and_keep_passive_data() {
    let token = CancellationToken::default();
    let mut instance = {
        let types = &[1, 0x60, 0, 1, 0x7f];
        let bytes = program(
            types,
            &[(5, &[1, 0, 1])],
            &[Function {
                name: "read",
                ty: 0,
                code: &[
                    0x41, 0, 0x41, 0, 0x41, 1, 0xfc, 8, 0, 0, 0x41, 0, 0x2d, 0, 0,
                ],
            }],
            None,
            Some(&[1, 1, 1, 42]),
        );
        Instance::instantiate_owned(validated(&bytes), Default::default(), &token).unwrap()
    };
    assert_eq!(
        instance.function_type("read").unwrap().results,
        [rustic_wasm::ValueType::I32],
    );
    assert_eq!(run(&mut instance, "read", &[]), 42);
    assert!(instance.function_type("missing").is_none());
}

#[test]
fn numeric_global_access_checks_the_index_type_and_mutability_before_writing() {
    let bytes = module(&[(6, &[2, 0x7f, 1, 0x41, 7, 0x0b, 0x7f, 0, 0x41, 9, 0x0b])]);
    let token = CancellationToken::default();
    let instance =
        Instance::instantiate_owned(validated(&bytes), Default::default(), &token).unwrap();

    assert_eq!(instance.global_value(0), Some(Value::I32(7)));
    assert_eq!(instance.global_value(1), Some(Value::I32(9)));
    assert_eq!(instance.global_value(2), None);
    assert!(instance.global_type(0).unwrap().mutable);
    assert!(!instance.global_type(1).unwrap().mutable);
    assert_eq!(instance.global_type(2), None);
    assert!(!instance.set_global_value(0, Value::I64(100)));
    assert!(!instance.set_global_value(1, Value::I32(100)));
    assert!(!instance.set_global_value(2, Value::I32(100)));
    assert_eq!(instance.global_value(0), Some(Value::I32(7)));
    assert_eq!(instance.global_value(1), Some(Value::I32(9)));
    assert!(instance.set_global_value(0, Value::I32(100)));
    assert_eq!(instance.global_value(0), Some(Value::I32(100)));
}

#[test]
fn owned_global_bindings_preserve_live_values_and_validate_types() {
    let bytes = module(&[(2, &[1, 1, b'm', 1, b'g', 3, 0x7f, 1])]);
    let token = CancellationToken::default();
    let instantiate = |globals: &[rustic_wasm::NumericGlobal]| {
        Instance::instantiate_owned_with_host(
            validated(&bytes),
            Default::default(),
            &token,
            globals,
            &[],
            &mut |_, _, _| unreachable!("module has no function imports or start"),
        )
    };
    assert_eq!(instantiate(&[]).err().unwrap().kind, Fault::MissingImport);
    let wrong = rustic_wasm::NumericGlobal::new(Value::I64(7), true);
    assert_eq!(
        instantiate(&[wrong]).err().unwrap().kind,
        Fault::ImportTypeMismatch
    );
    let cell = rustic_wasm::NumericGlobal::new(Value::I32(7), true);
    let first = instantiate(std::slice::from_ref(&cell)).unwrap();
    let second = instantiate(std::slice::from_ref(&cell)).unwrap();
    assert_eq!(first.global_value(0), Some(Value::I32(7)));
    assert!(first.set_global_value(0, Value::I32(11)));
    let reexport = second.share_numeric_global(0).unwrap();
    assert_eq!(reexport.get(), Value::I32(11));
    assert!(reexport.set(Value::I32(19)));
    assert!(!reexport.set(Value::I64(22)));
    assert_eq!(cell.get(), Value::I32(19));
    assert_eq!(first.global_value(0), Some(Value::I32(19)));
}

#[test]
fn imported_memory_preserves_segment_prefixes_and_copies_across_aliased_indices() {
    use rustic_wasm::LinearMemory;

    let memory = LinearMemory::new(1, Some(2), 2 * PAGE).unwrap();
    let token = CancellationToken::default();
    let imports = &[2, 1, b'm', 1, b'a', 2, 0, 1, 1, b'm', 1, b'b', 2, 0, 1];
    let trap = module(&[
        (2, imports),
        (
            11,
            &[
                2, 0, 0x41, 0, 0x0b, 3, 1, 2, 3, 0, 0x41, 0x80, 0x80, 0x04, 0x0b, 1, 99,
            ],
        ),
    ]);
    let bindings = &[memory.clone(), memory.clone()];
    let instance = |bytes: &[u8]| {
        Instance::instantiate_owned_with_host(
            validated(bytes),
            Default::default(),
            &token,
            &[],
            bindings,
            &mut |_, _, _| unreachable!("no imported functions"),
        )
    };
    assert_eq!(
        instance(&trap).err().unwrap().kind,
        Fault::MemoryOutOfBounds
    );
    assert_eq!(&memory.buffer().read().unwrap()[..4], &[1, 2, 3, 0]);

    let copy = module(&[
        (1, &[1, 0x60, 0, 0]),
        (2, imports),
        (3, &[1, 0]),
        (7, &[1, 4, b'c', b'o', b'p', b'y', 0, 0]),
        (
            10,
            &[1, 12, 0, 0x41, 1, 0x41, 0, 0x41, 3, 0xfc, 10, 1, 0, 0x0b],
        ),
    ]);
    instance(&copy)
        .unwrap()
        .invoke("copy", &[], &token)
        .unwrap();
    assert_eq!(&memory.buffer().read().unwrap()[..4], &[1, 1, 2, 3]);
    let old = memory.buffer();
    assert_eq!(memory.grow(2, |_| Ok::<_, ()>(())).unwrap(), None);
    assert!(old.read().is_some());
    assert_eq!(memory.grow(0, |_| Ok::<_, ()>(())).unwrap(), Some(1));
    assert!(old.read().is_none());
}

#[test]
fn memory64_bulk_operations_keep_full_width_addresses_under_the_host_budget() {
    #[rustfmt::skip]
    let types = [
        4, 0x60, 0, 0,          // () -> ()
        0x60, 1, 0x7e, 1, 0x7e, // (i64) -> i64
        0x60, 1, 0x7e, 1, 0x7f, // (i64) -> i32
        0x60, 1, 0x7e, 0,       // (i64) -> ()
    ];
    #[rustfmt::skip]
    let bulk = [
        0x42, 2, 0x41, 0, 0x41, 3, 0xfc, 8, 1, 0, // Init memory64 with passive data.
        0x42, 5, 0x41, 33, 0x42, 2, 0xfc, 11, 0,  // Fill memory64.
        0x41, 0, 0x42, 0, 0x41, 7, 0xfc, 10, 1, 0, // Copy to memory32.
        0x42, 0, 0x41, 1, 0x41, 4, 0xfc, 10, 0, 1, // Copy back to memory64.
        0x42, 1, 0x42, 0, 0x42, 4, 0xfc, 10, 0, 0, // Overlapping memory64 copy.
    ];
    let bytes = program(
        &types,
        &[(5, &[2, 5, 1, 3, 0, 1])],
        &[
            Function {
                name: "bulk",
                ty: 0,
                code: &bulk,
            },
            Function {
                name: "grow",
                ty: 1,
                code: &[0x20, 0, 0x40, 0],
            },
            Function {
                name: "load",
                ty: 2,
                code: &[0x20, 0, 0x2d, 0, 1],
            },
            Function {
                name: "wide_offset",
                ty: 2,
                code: &[0x20, 0, 0x2d, 0, 0x80, 0x80, 0x80, 0x80, 0x10],
            },
            Function {
                name: "fill",
                ty: 3,
                code: &[0x42, 0, 0x41, 0, 0x20, 0, 0xfc, 11, 0],
            },
        ],
        None,
        Some(&[2, 0, 0x42, 0, 0x0b, 2, b'A', b'B', 1, 3, b'C', b'D', b'E']),
    );
    let module = validated(&bytes);
    let limits = InstanceLimits {
        memory_bytes: 3 * PAGE,
        ..Default::default()
    };
    let token = CancellationToken::default();
    let mut instance = Instance::instantiate(&module, limits, &token).unwrap();
    let memory = instance.memory(0).unwrap();
    instance.invoke("bulk", &[], &token).unwrap();
    let old = memory.buffer();
    assert_eq!(&old.read().unwrap()[..7], b"BBCDE!!");
    for (name, arg) in [
        ("load", -1),
        ("load", 1 << 32),
        ("wide_offset", 0),
        ("fill", (1 << 32) + 1),
    ] {
        assert_eq!(
            instance
                .invoke(name, &[Value::I64(arg)], &token)
                .unwrap_err()
                .kind,
            Fault::MemoryOutOfBounds,
        );
    }
    assert_eq!(&old.read().unwrap()[..7], b"BBCDE!!");
    for (delta, previous) in [(-1, -1), (1 << 32, -1), (2, -1), (1, 1), (0, 2)] {
        assert_eq!(
            instance
                .invoke("grow", &[Value::I64(delta)], &token)
                .unwrap(),
            [Value::I64(previous)],
        );
        assert_eq!(old.read().is_some(), previous == -1);
    }
    assert_eq!(memory.pages(), 2);
    let current = memory.buffer();
    let bytes = current.read().unwrap();
    assert_eq!(&bytes[..7], b"BBCDE!!");
    assert!(bytes[PAGE..].iter().all(|&byte| byte == 0));
}

use rustic_base::CancellationToken;
use rustic_wasm::{
    BinaryLimits, DecodeLimits, ExecutionLimits, ExportIndex, FunctionImport, FunctionType,
    ImportType, Instance, InstanceLimits, InstructionLimits, Opcode, ValidatedModule,
    ValidationLimits, Value, decode_instructions, decode_module, scan_module, validate_module,
};

use crate::Target;

pub(crate) struct WasmBinaryTarget;

pub(crate) fn seeds() -> impl Iterator<Item = Vec<u8>> {
    let modules: &[&[u8]] = &[
        b"\0asm\x01\0\0\0\0\x05\x03nme\xff",
        b"\0asm\x01\0\0\0\x01\x05\x01\x60\0\x01\x7f\x03\x02\x01\0\x0a\x06\x01\x04\0\x41\x2a\x0b",
        b"\0asm\x01\0\0\0\x01\x04\x01\x60\0\0\x03\x02\x01\0\x0a\x0a\x01\x08\0\x02\x40\x41\x01\x1a\x0b\x0b",
        // One memory with an active data segment and a prior data count.
        b"\0asm\x01\0\0\0\x05\x03\x01\0\x01\x0c\x01\x01\x0b\x08\x01\0\x41\0\x0b\x02\x01\x02",
        // An exported entry point calling a second local function.
        b"\0asm\x01\0\0\0\x01\x04\x01\x60\0\0\x03\x03\x02\0\0\x07\x07\x01\x03run\0\0\x0a\x09\x02\x04\0\x10\x01\x0b\x02\0\x0b",
        // A function reference declared by an element segment.
        b"\0asm\x01\0\0\0\x01\x04\x01\x60\0\0\x03\x02\x01\0\x09\x05\x01\x03\0\x01\0\x0a\x08\x01\x06\0\xd2\0\xd1\x1a\x0b",
        // An extended constant expression reading an earlier immutable global.
        b"\0asm\x01\0\0\0\x06\x0e\x02\x7f\0\x41\x05\x0b\x7f\0\x23\0\x41\x01\x6a\x0b",
        // An exported loop which exercises the interpreter's work limit.
        b"\0asm\x01\0\0\0\x01\x04\x01\x60\0\0\x03\x02\x01\0\x07\x07\x01\x03run\0\0\x0a\x09\x01\x07\0\x03\x40\x0c\0\x0b\x0b",
        // An exported load reads bytes applied during instantiation.
        b"\0asm\x01\0\0\0\x01\x05\x01\x60\0\x01\x7f\x03\x02\x01\0\x05\x03\x01\0\x01\x07\x07\x01\x03run\0\0\x0a\x09\x01\x07\0\x41\0\x2d\0\0\x0b\x0b\x07\x01\0\x41\0\x0b\x01*",
        // An active table element supplies a matching indirect call target.
        b"\0asm\x01\0\0\0\x01\x05\x01\x60\0\x01\x7f\x03\x03\x02\0\0\x04\x04\x01\x70\0\x01\x07\x07\x01\x03run\0\0\x09\x07\x01\0\x41\0\x0b\x01\x01\x0a\x0e\x02\x07\0\x41\0\x11\0\0\x0b\x04\0\x41\x2a\x0b",
        // An exported function calls the one import supplied by the harness.
        b"\0asm\x01\0\0\0\x01\x04\x01\x60\0\0\x02\x07\x01\x01m\x01f\0\0\x03\x02\x01\0\x07\x07\x01\x03run\0\x01\x0a\x06\x01\x04\0\x10\0\x0b",
    ];
    modules
        .iter()
        .flat_map(|bytes| (0..=bytes.len()).map(move |end| bytes[..end].to_vec()))
}

impl Target for WasmBinaryTarget {
    fn name(&self) -> &'static str {
        "wasm-binary"
    }

    fn max_len(&self) -> usize {
        8192
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[
            b"\0asm\x01\0\0\0",
            &[0x80; 5],
            &[0xff; 4],
            &[0x02, 0x40],
            &[0x04, 0x7f],
            &[0x0b],
            &[0xfc, 8],
            &[0xfd],
        ]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let token = CancellationToken::default();
        let module = match scan_module(input, BinaryLimits::default(), &token) {
            Ok(module) => module,
            Err(error) if error.offset <= input.len() => return Ok(()),
            Err(error) => return Err(format!("scanner error outside input: {error}")),
        };
        for section in module.sections {
            if !input
                .get(section.offset..)
                .is_some_and(|bytes| bytes.starts_with(section.bytes))
            {
                return Err("section span outside module".to_owned());
            }
        }
        for body in module.functions {
            if !input
                .get(body.offset..)
                .is_some_and(|bytes| bytes.starts_with(body.instructions))
            {
                return Err("instruction span outside module".to_owned());
            }
            let end = body.offset + body.instructions.len();
            let instructions =
                match decode_instructions(&body, InstructionLimits::default(), &token) {
                    Ok(ops) => ops,
                    Err(error) if (body.offset..=end).contains(&error.offset) => continue,
                    Err(error) => return Err(format!("instruction error outside body: {error}")),
                };
            if instructions
                .first()
                .is_none_or(|first| first.offset != body.offset)
                || instructions
                    .last()
                    .is_none_or(|last| last.offset + 1 != end || last.opcode != Opcode::Core(0x0b))
                || instructions
                    .windows(2)
                    .any(|pair| pair[0].offset >= pair[1].offset)
            {
                return Err("decoded instruction offsets do not cover the body in order".to_owned());
            }
        }
        let decoded = match decode_module(input, DecodeLimits::default(), &token) {
            Ok(module) => module,
            Err(error) if error.offset <= input.len() => return Ok(()),
            Err(error) => return Err(format!("module decoder error outside input: {error}")),
        };
        match validate_module(decoded, ValidationLimits::default(), &token) {
            Ok(validated) => check_execution(&validated, input.len(), &token),
            Err(error) if error.offset <= input.len() => Ok(()),
            Err(error) => Err(format!("validator error outside input: {error}")),
        }
    }
}

fn check_execution(
    validated: &ValidatedModule<'_>,
    input_len: usize,
    token: &CancellationToken,
) -> Result<(), String> {
    let module = validated.module();
    let limits = InstanceLimits {
        execution: ExecutionLimits {
            preparation_work: 2 * 65_536 + 16_384,
            invocation_work: 512,
            values: 256,
            calls: 32,
            control_frames: 64,
        },
        memory_bytes: 2 * 65_536,
        table_elements: 256,
    };
    let signature = FunctionType {
        params: Vec::new(),
        results: Vec::new(),
    };
    let empty = |_: &[Value], _: &mut [Value], _: &CancellationToken| Ok(());
    let imports = [FunctionImport::new("m", "f", &signature, &empty)];
    let mut instance = match Instance::instantiate_with_imports(validated, limits, &imports, token)
    {
        Ok(instance) => instance,
        Err(error) if error.offset <= input_len => return Ok(()),
        Err(error) => return Err(format!("instantiation error outside input: {error}")),
    };
    let declarations = module.declarations();
    for export in declarations.exports.iter().take(4) {
        let ExportIndex::Function(index) = export.value.target else {
            continue;
        };
        let index = index as usize;
        let type_index = if let Some(import) = declarations.imports.get(index) {
            let ImportType::Function(ty) = import.value.ty else {
                return Err("linked a non-function import".to_owned());
            };
            ty
        } else {
            module.functions()[index - declarations.imports.len()]
                .value
                .type_index
        };
        let ty = &declarations.types[type_index as usize].value;
        if !ty.params.is_empty() {
            continue;
        }
        match instance.invoke(export.value.name, &[], token) {
            Ok(results)
                if results
                    .iter()
                    .map(|v| v.value_type())
                    .eq(ty.results.iter().copied()) => {}
            Ok(_) => return Err("executed result disagrees with the export signature".to_owned()),
            Err(error) if error.offset <= input_len => {}
            Err(error) => return Err(format!("execution error outside input: {error}")),
        }
    }
    Ok(())
}

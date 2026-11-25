//! Window WebAssembly byte compilation and module metadata.

mod global;
mod host;
mod instance;
mod memory;
mod table;

use std::{cell::RefCell, ops::Range, rc::Rc};

use rustic_base::CancellationToken;
use rustic_js::runtime::{
    Abrupt, Agent, BigInt, JsResult, NativeFn, ObjectId, OutOfMemory, PropertyDescriptor,
    PropertyKey, Value, to_int32,
};
use rustic_wasm::{
    AddressType, BinaryErrorKind, DecodeLimits, ExportIndex, ImportType, ValidatedModule,
    ValidationErrorKind, Value as WasmValue, ValueType, decode_module, validate_module,
};

use super::{
    events::{define_method, with_root},
    fetch::{settle, wasm_response_bytes},
    prototype, with_page,
};

const MODULE: &str = "WebAssembly.Module";
const COMPILE_ERROR: &str = "WebAssembly.CompileError";
const STREAM_OPTIONS: &str = "WebAssembly stream options";
const COMPILE_INPUT: &str = "WebAssembly compile input";
pub(super) const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_OPTIONS: usize = 128;
const MAX_OPTION_UNITS: usize = 4096;
const MAX_DECODER_SCRATCH: usize = 64 * 1024 * 1024;

struct Module {
    bytes: Vec<u8>,
    imports: Vec<Import>,
    exports: Vec<Export>,
    custom: Vec<Custom>,
}

struct Import {
    module: Range<usize>,
    name: Range<usize>,
    ty: ImportType,
}

struct Export {
    name: Range<usize>,
    target: ExportIndex,
}

struct Custom {
    name: Range<usize>,
    contents: Range<usize>,
}

#[derive(Default)]
struct Options {
    builtins: Vec<String>,
    strings: Option<String>,
}

impl Options {
    fn footprint(&self) -> usize {
        self.builtins.capacity() * size_of::<String>()
            + self.builtins.iter().map(String::capacity).sum::<usize>()
            + self.strings.as_ref().map_or(0, String::capacity)
    }

    fn admits(&self, module: &ValidatedModule<'_>) -> bool {
        // The current profile has no imported string constant or JS builtin
        // adapter. Feature options only affect their matching namespaces.
        let js_strings = self.builtins.iter().any(|name| name == "js-string");
        let unique = self
            .builtins
            .iter()
            .enumerate()
            .all(|(i, item)| !self.builtins[..i].contains(item));
        unique
            && module.module().declarations().imports.iter().all(|entry| {
                let module = entry.value.module;
                self.strings.as_deref() != Some(module)
                    && !(js_strings && module == "wasm:js-string")
            })
    }
}

struct Input {
    bytes: RefCell<Option<Vec<u8>>>,
    options: Options,
}

pub(super) fn install(agent: &mut Agent) -> JsResult<()> {
    let namespace = agent.new_object_with(Some(agent.intrinsics.object_prototype))?;
    tag(agent, namespace, "WebAssembly")?;
    define_method(agent, namespace, "validate", validate, 1)?;
    define_method(agent, namespace, "compile", compile, 1)?;
    define_method(agent, namespace, "compileStreaming", compile_streaming, 1)?;
    global::install(agent, namespace)?;
    memory::install(agent, namespace)?;
    table::install(agent, namespace)?;
    instance::install(agent, namespace)?;

    let module = agent.new_object_with(Some(agent.intrinsics.object_prototype))?;
    tag(agent, module, MODULE)?;
    agent.install_host_interface_on(
        namespace,
        MODULE,
        construct,
        1,
        module,
        |agent, constructor| {
            define_method(agent, constructor, "imports", imports, 1)?;
            define_method(agent, constructor, "exports", exports, 1)?;
            define_method(agent, constructor, "customSections", custom_sections, 2)
        },
    )?;

    let error = agent.intrinsics.error_prototype;
    let parent = agent
        .get(error, PropertyKey::String(agent.names.constructor))?
        .as_object();
    for (name, tag, behavior) in [
        (
            "CompileError",
            COMPILE_ERROR,
            construct_error::<0> as NativeFn,
        ),
        ("LinkError", "WebAssembly.LinkError", construct_error::<1>),
        (
            "RuntimeError",
            "WebAssembly.RuntimeError",
            construct_error::<2>,
        ),
    ] {
        let proto = agent.new_object_with(Some(error))?;
        let name = agent.string(name)?;
        for (key, value) in [
            (agent.names.name, name),
            (agent.names.message, Value::string(agent.names.empty)),
        ] {
            agent.define_property_or_throw(
                proto,
                PropertyKey::String(key),
                PropertyDescriptor::data_with(value, true, false, true),
            )?;
        }
        agent.install_host_interface_on(
            namespace,
            tag,
            behavior,
            1,
            proto,
            |agent, constructor| {
                agent.set_prototype_of(constructor, parent)?;
                Ok(())
            },
        )?;
    }
    let key = agent.heap.key("WebAssembly")?;
    agent.define_property_or_throw(
        agent.intrinsics.global,
        key,
        PropertyDescriptor::data_with(Value::object(namespace), true, false, true),
    )
}

fn tag(agent: &mut Agent, object: ObjectId, name: &str) -> JsResult<()> {
    let value = agent.string(name)?;
    agent.define_property_or_throw(
        object,
        PropertyKey::Symbol(agent.symbols.to_string_tag),
        PropertyDescriptor::data_with(value, false, false, true),
    )
}

fn arg(args: &[Value], index: usize) -> Value {
    args.get(index).copied().unwrap_or(Value::UNDEFINED)
}

fn address_type(agent: &mut Agent, descriptor: ObjectId, invalid: &str) -> JsResult<AddressType> {
    let key = agent.heap.key("address")?;
    let value = agent.get(descriptor, key)?;
    if value == Value::UNDEFINED {
        return Ok(AddressType::I32);
    }
    let name = agent.to_string(value)?;
    let name = agent.heap.string(name);
    if name.len() == 3 {
        match name.to_text().as_str() {
            "i32" => return Ok(AddressType::I32),
            "i64" => return Ok(AddressType::I64),
            _ => {}
        }
    }
    agent.type_error(invalid)
}

fn address_index(
    agent: &mut Agent,
    value: Value,
    address: AddressType,
    invalid: &str,
) -> JsResult<u64> {
    match address {
        AddressType::I32 => {
            let number = agent.to_number(value)?.trunc();
            if (0.0..=f64::from(u32::MAX)).contains(&number) {
                return Ok(number as u64);
            }
        }
        AddressType::I64 => {
            let number = agent.to_bigint(value)?;
            if !number.is_negative() && number.bit_length() <= 64 {
                return Ok(number.to_u64_wrapping());
            }
        }
    }
    agent.type_error(invalid)
}

fn is_numeric(ty: ValueType) -> bool {
    matches!(
        ty,
        ValueType::I32 | ValueType::I64 | ValueType::F32 | ValueType::F64
    )
}

fn to_wasm(agent: &mut Agent, value: Value, ty: ValueType) -> JsResult<WasmValue> {
    Ok(match ty {
        ValueType::I32 => WasmValue::I32(to_int32(agent.to_number(value)?)),
        ValueType::I64 => WasmValue::I64(agent.to_bigint64(value)?),
        ValueType::F32 => WasmValue::F32((agent.to_number(value)? as f32).to_bits()),
        ValueType::F64 => WasmValue::F64(agent.to_number(value)?.to_bits()),
        _ => unreachable!("numeric Wasm host value"),
    })
}

fn to_js(agent: &mut Agent, value: WasmValue) -> JsResult<Value> {
    match value {
        WasmValue::I32(value) => Ok(Value::number(f64::from(value))),
        WasmValue::I64(value) => agent.bigint_value(BigInt::from_i64(value)),
        WasmValue::F32(value) => Ok(Value::number(f64::from(f32::from_bits(value)))),
        WasmValue::F64(value) => Ok(Value::number(f64::from_bits(value))),
    }
}

fn options(agent: &mut Agent, value: Value) -> JsResult<Options> {
    if value.is_nullish() {
        return Ok(Options::default());
    }
    let Some(object) = value.as_object() else {
        return agent.type_error("invalid WebAssembly compile options");
    };
    let builtins_key = agent.heap.key("builtins")?;
    let value = agent.get(object, builtins_key)?;
    let mut options = Options::default();
    if value != Value::UNDEFINED {
        agent.iterate_values(value, |agent, item| {
            if options.builtins.len() == MAX_OPTIONS {
                return agent.type_error("too many WebAssembly builtins");
            }
            options.builtins.try_reserve(1).map_err(|_| OutOfMemory)?;
            options.builtins.push(option_string(agent, item)?);
            Ok(true)
        })?;
    }
    let strings_key = agent.heap.key("importedStringConstants")?;
    let strings = agent.get(object, strings_key)?;
    if !strings.is_nullish() {
        options.strings = Some(option_string(agent, strings)?);
    }
    Ok(options)
}

fn option_string(agent: &mut Agent, input: Value) -> JsResult<String> {
    let id = agent.to_string(input)?;
    let value = agent.heap.string(id);
    if value.len() > MAX_OPTION_UNITS {
        return agent.type_error("WebAssembly compile option exceeds length limit");
    }
    Ok(value.to_text())
}

fn construct_error<const KIND: u8>(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let interface = [
        COMPILE_ERROR,
        "WebAssembly.LinkError",
        "WebAssembly.RuntimeError",
    ][KIND as usize];
    let proto = match agent.new_target().or_else(|| agent.active_function()) {
        Some(target) => agent.host_prototype_from_new_target(interface, target)?,
        None => prototype(agent, interface)?,
    };
    let error = agent.new_error_object_with(proto)?;
    with_root(agent, Value::object(error), |agent| {
        let message = arg(args, 0);
        if message != Value::UNDEFINED {
            let message = Value::string(agent.to_string(message)?);
            agent.define_property_or_throw(
                error,
                PropertyKey::String(agent.names.message),
                PropertyDescriptor::data_with(message, true, false, true),
            )?;
        }
        let options = arg(args, 1);
        if let Some(options) = options.as_object() {
            let key = agent.heap.key("cause")?;
            if agent.has_property(options, key)? {
                let cause = agent.get(options, key)?;
                agent.define_property_or_throw(
                    error,
                    key,
                    PropertyDescriptor::data_with(cause, true, false, true),
                )?;
            }
        }
        Ok(Value::object(error))
    })
}

pub(super) fn compile_error(agent: &mut Agent, message: &str) -> Abrupt {
    wasm_error(agent, COMPILE_ERROR, message)
}

fn wasm_error(agent: &mut Agent, interface: &'static str, message: &str) -> Abrupt {
    let result = (|| {
        let proto = prototype(agent, interface)?;
        let error = agent.new_error_object_with(proto)?;
        let message = agent.string(message)?;
        agent.define_property_or_throw(
            error,
            PropertyKey::String(agent.names.message),
            PropertyDescriptor::data_with(message, true, false, true),
        )?;
        Ok(Value::object(error))
    })();
    match result {
        Ok(error) => Abrupt::Throw(error),
        Err(error) => error,
    }
}

enum CompileFailure {
    Binary(rustic_wasm::BinaryError),
    Validation(rustic_wasm::ValidationError),
    Options,
}

impl CompileFailure {
    fn cancelled(&self) -> bool {
        match self {
            Self::Binary(e) => e.kind == BinaryErrorKind::Cancelled,
            Self::Validation(e) => e.kind == ValidationErrorKind::Cancelled,
            Self::Options => false,
        }
    }

    fn throw(self, agent: &mut Agent) -> Abrupt {
        if self.cancelled() {
            return Abrupt::Cancelled;
        }
        match self {
            Self::Binary(e) => compile_error(agent, &e.to_string()),
            Self::Validation(e) => compile_error(agent, &e.to_string()),
            Self::Options => compile_error(
                agent,
                "unsupported WebAssembly compile options for this module",
            ),
        }
    }
}

fn checked<'a>(
    bytes: &'a [u8],
    options: &Options,
    token: &CancellationToken,
) -> Result<ValidatedModule<'a>, CompileFailure> {
    let mut limits = DecodeLimits::default();
    limits.binary.module_bytes = MAX_BYTES;
    limits.binary.functions = 32_768;
    limits.binary.local_groups = 65_536;
    limits.declaration_entries = 131_072;
    limits.code.instructions = 262_144;
    limits.code.vector_entries = 131_072;
    let decoded = decode_module(bytes, limits, token).map_err(CompileFailure::Binary)?;
    let validated =
        validate_module(decoded, Default::default(), token).map_err(CompileFailure::Validation)?;
    if !options.admits(&validated) {
        return Err(CompileFailure::Options);
    }
    Ok(validated)
}

fn validate(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let source = agent.web_resizable_buffer_source(arg(args, 0))?;
    let options = options(agent, arg(args, 1))?;
    let token = agent.cancellation().unwrap_or_default();
    let len = agent.web_buffer_bytes(source).len();
    if len > MAX_BYTES {
        return Ok(Value::FALSE);
    }
    compilation_work(agent, len, |agent| {
        match checked(&agent.web_buffer_bytes(source), &options, &token) {
            Ok(_) => Ok(Value::TRUE),
            Err(error) if error.cancelled() => Err(Abrupt::Cancelled),
            Err(_) => Ok(Value::FALSE),
        }
    })
}

fn compilation_work<T>(
    agent: &mut Agent,
    length: usize,
    work: impl FnOnce(&mut Agent) -> JsResult<T>,
) -> JsResult<T> {
    agent.spend_native_work((length as u64).div_ceil(64).max(1))?;
    // The decoder retains slot metadata for byte-sized instructions as well
    // as declarations and indices. Its vector limits bound the upper charge.
    let scratch = compilation_scratch(length);
    agent.with_host_memory(scratch, work)
}

fn compilation_scratch(length: usize) -> usize {
    (length * 128).min(MAX_DECODER_SCRATCH)
}

fn snapshot(agent: &mut Agent, source: ObjectId) -> JsResult<Vec<u8>> {
    if agent.web_buffer_bytes(source).len() > MAX_BYTES {
        return Err(compile_error(
            agent,
            "WebAssembly binary exceeds the input limit",
        ));
    }
    let bytes = agent.web_buffer_bytes(source);
    let mut stable = Vec::new();
    stable
        .try_reserve_exact(bytes.len())
        .map_err(|_| OutOfMemory)?;
    stable.extend_from_slice(&bytes);
    Ok(stable)
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("WebAssembly.Module requires new");
    };
    let source = agent.web_resizable_buffer_source(arg(args, 0))?;
    let options = options(agent, arg(args, 1))?;
    let bytes = snapshot(agent, source)?;
    let proto = agent.host_prototype_from_new_target(MODULE, target)?;
    compile_bytes(agent, bytes, &options, proto)
}

fn compile(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    promised(agent, |agent| {
        let source = agent.web_resizable_buffer_source(arg(args, 0))?;
        let options = options(agent, arg(args, 1))?;
        let bytes = snapshot(agent, source)?;
        let footprint = bytes.capacity() + options.footprint();
        let record = Input {
            bytes: RefCell::new(Some(bytes)),
            options,
        };
        let context = agent.new_host_state(
            COMPILE_INPUT,
            agent.intrinsics.object_prototype,
            record,
            footprint,
        )?;
        agent.react_to_host_value(Value::UNDEFINED, compile_job, Value::object(context))
    })
}

fn compile_job(agent: &mut Agent, _: Value, _: &[Value]) -> JsResult<Value> {
    let context = agent
        .host_capture(0)
        .as_object()
        .expect("private compile context");
    let input: Rc<Input> = agent
        .host_state(context, COMPILE_INPUT)
        .expect("compile input");
    let bytes = input
        .bytes
        .borrow_mut()
        .take()
        .expect("one compile reaction");
    let proto = prototype(agent, MODULE)?;
    compile_bytes(agent, bytes, &input.options, proto)
}

fn compile_streaming(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    promised(agent, |agent| {
        let options = options(agent, arg(args, 1))?;
        let footprint = options.footprint();
        let context = agent.new_host_state(
            STREAM_OPTIONS,
            agent.intrinsics.object_prototype,
            options,
            footprint,
        )?;
        agent.react_to_host_value(arg(args, 0), stream_job, Value::object(context))
    })
}

fn stream_job(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let context = agent
        .host_capture(0)
        .as_object()
        .expect("private stream context");
    let options: Rc<Options> = agent
        .host_state(context, STREAM_OPTIONS)
        .expect("stream options");
    let bytes = wasm_response_bytes(agent, arg(args, 0))?;
    let proto = prototype(agent, MODULE)?;
    compile_bytes(agent, bytes, &options, proto)
}

fn promised(
    agent: &mut Agent,
    work: impl FnOnce(&mut Agent) -> JsResult<Value>,
) -> JsResult<Value> {
    match work(agent) {
        Err(error @ Abrupt::Throw(_)) => {
            let promise = agent.new_host_promise()?;
            settle(agent, promise, Err(error))?;
            Ok(Value::object(promise))
        }
        other => other,
    }
}

fn compile_bytes(
    agent: &mut Agent,
    bytes: Vec<u8>,
    options: &Options,
    proto: ObjectId,
) -> JsResult<Value> {
    let allowed = with_page(agent, |_, page| {
        let verdict = page.wasm_compilation.borrow();
        for violation in &verdict.violations {
            eprintln!("rustic: CSP: {violation}");
        }
        Ok(verdict.allowed())
    })?;
    if !allowed {
        return Err(compile_error(
            agent,
            "WebAssembly compilation is blocked by Content Security Policy",
        ));
    }
    let token = agent.cancellation().unwrap_or_default();
    let (imports, exports, custom) = compilation_work(agent, bytes.len(), |agent| {
        let parsed = checked(&bytes, options, &token).map_err(|error| error.throw(agent))?;
        metadata(&bytes, &parsed, &token)
    })?;
    let footprint = bytes.capacity()
        + imports.capacity() * size_of::<Import>()
        + exports.capacity() * size_of::<Export>()
        + custom.capacity() * size_of::<Custom>();
    let state = Module {
        bytes,
        imports,
        exports,
        custom,
    };
    Ok(Value::object(
        agent.new_host_state(MODULE, proto, state, footprint)?,
    ))
}

fn range(bytes: &[u8], contents: &[u8]) -> Range<usize> {
    // Decoder views borrow the same allocation. Their addresses give the
    // original offsets, including the location of empty names and payloads.
    let first = contents.as_ptr() as usize - bytes.as_ptr() as usize;
    first..first + contents.len()
}

type Metadata = (Vec<Import>, Vec<Export>, Vec<Custom>);

fn metadata(
    bytes: &[u8],
    module: &ValidatedModule<'_>,
    token: &CancellationToken,
) -> JsResult<Metadata> {
    let module = module.module();
    let declarations = module.declarations();
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut custom = Vec::new();
    imports
        .try_reserve_exact(declarations.imports.len())
        .map_err(|_| OutOfMemory)?;
    exports
        .try_reserve_exact(declarations.exports.len())
        .map_err(|_| OutOfMemory)?;
    custom
        .try_reserve_exact(module.custom_sections().len())
        .map_err(|_| OutOfMemory)?;
    for entry in &declarations.imports {
        token.check().map_err(|_| Abrupt::Cancelled)?;
        let import = &entry.value;
        imports.push(Import {
            module: range(bytes, import.module.as_bytes()),
            name: range(bytes, import.name.as_bytes()),
            ty: import.ty,
        });
    }
    for entry in &declarations.exports {
        token.check().map_err(|_| Abrupt::Cancelled)?;
        let export = &entry.value;
        exports.push(Export {
            name: range(bytes, export.name.as_bytes()),
            target: export.target,
        });
    }
    for section in module.custom_sections() {
        token.check().map_err(|_| Abrupt::Cancelled)?;
        let name = section.name.expect("custom section name");
        custom.push(Custom {
            name: range(bytes, name.as_bytes()),
            contents: range(bytes, section.bytes),
        });
    }
    Ok((imports, exports, custom))
}

fn module(agent: &mut Agent, input: Value) -> JsResult<Rc<Module>> {
    match input
        .as_object()
        .and_then(|id| agent.host_state(id, MODULE))
    {
        Some(state) => Ok(state),
        None => agent.type_error("expected a WebAssembly.Module"),
    }
}

impl Module {
    fn text(&self, location: &Range<usize>) -> &str {
        std::str::from_utf8(&self.bytes[location.clone()]).expect("validated WebAssembly name")
    }
}

fn imports(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let module = module(agent, arg(args, 0))?;
    sequence(agent, module.imports.iter(), |agent, record| {
        descriptor(
            agent,
            &[
                ("module", module.text(&record.module)),
                ("name", module.text(&record.name)),
                (
                    "kind",
                    match record.ty {
                        ImportType::Function(_) => "function",
                        ImportType::Table(_) => "table",
                        ImportType::Memory(_) => "memory",
                        ImportType::Global(_) => "global",
                        ImportType::Tag(_) => "tag",
                    },
                ),
            ],
        )
    })
}

fn exports(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let module = module(agent, arg(args, 0))?;
    sequence(agent, module.exports.iter(), |agent, record| {
        let kind = match record.target {
            ExportIndex::Function(_) => "function",
            ExportIndex::Table(_) => "table",
            ExportIndex::Memory(_) => "memory",
            ExportIndex::Global(_) => "global",
            ExportIndex::Tag(_) => "tag",
        };
        descriptor(
            agent,
            &[("name", module.text(&record.name)), ("kind", kind)],
        )
    })
}

fn custom_sections(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let module = module(agent, arg(args, 0))?;
    if args.len() < 2 {
        return agent.type_error("customSections requires a name");
    }
    let name = agent.to_string(arg(args, 1))?;
    sequence(agent, module.custom.iter(), |agent, section| {
        if agent
            .heap
            .string(name)
            .units()
            .eq(module.text(&section.name).encode_utf16())
        {
            agent.new_web_array_buffer(
                agent.intrinsics.global,
                &module.bytes[section.contents.clone()],
            )
        } else {
            Ok(Value::UNDEFINED)
        }
    })
}

fn sequence<T>(
    agent: &mut Agent,
    entries: impl Iterator<Item = T>,
    mut value: impl FnMut(&mut Agent, T) -> JsResult<Value>,
) -> JsResult<Value> {
    let list = agent.create_array_from_list(&[])?;
    with_root(agent, Value::object(list), |agent| {
        let token = agent.cancellation().unwrap_or_default();
        let mut index = 0;
        for entry in entries {
            token.check().map_err(|_| Abrupt::Cancelled)?;
            let item = value(agent, entry)?;
            if item != Value::UNDEFINED {
                agent.define_property_or_throw(
                    list,
                    PropertyKey::Index(index),
                    PropertyDescriptor::data_with(item, true, true, true),
                )?;
                index += 1;
            }
        }
        Ok(Value::object(list))
    })
}

fn descriptor(agent: &mut Agent, fields: &[(&str, &str)]) -> JsResult<Value> {
    let object = agent.new_object_with(Some(agent.intrinsics.object_prototype))?;
    for (key, value) in fields {
        let key = agent.heap.key(key)?;
        let value = agent.string(value)?;
        agent.define_property_or_throw(
            object,
            key,
            PropertyDescriptor::data_with(value, true, true, true),
        )?;
    }
    Ok(Value::object(object))
}

//! Standalone Window tables and live wrappers over module tables.

use rustic_js::runtime::ErrorKind;
use rustic_wasm::{ExternalReference, TableType};

use super::super::events::PrivateArray;
use super::instance::{self, Runtime};
use super::*;

const TABLE: &str = "WebAssembly.Table";
const TABLE_LIMIT: u32 = 65_536;

struct Table {
    address: AddressType,
    storage: Storage,
}

enum Storage {
    Standalone { functions: bool, capacity: u32 },
    Module { index: u32, functions: bool },
}

enum Entries {
    Standalone {
        values: PrivateArray,
        functions: bool,
        capacity: u32,
    },
    Module {
        owner: Value,
        runtime: Rc<Runtime>,
        index: u32,
        functions: bool,
    },
}

pub(super) fn install(agent: &mut Agent, namespace: ObjectId) -> JsResult<()> {
    let proto = agent.new_object_with(Some(agent.intrinsics.object_prototype))?;
    tag(agent, proto, TABLE)?;
    define_method(agent, proto, "grow", grow, 1)?;
    define_method(agent, proto, "get", get, 1)?;
    define_method(agent, proto, "set", set, 1)?;
    let getter = agent.new_native_function(length, "get length", 0)?;
    let key = agent.heap.key("length")?;
    agent.define_property_or_throw(
        proto,
        key,
        PropertyDescriptor::accessor(Value::object(getter), Value::UNDEFINED, true, true),
    )?;
    agent.install_host_interface_on(namespace, TABLE, construct, 1, proto, |_, _| Ok(()))
}

fn construct(agent: &mut Agent, _: Value, args: &[Value]) -> JsResult<Value> {
    let Some(target) = agent.new_target() else {
        return agent.type_error("WebAssembly.Table requires new");
    };
    let Some(descriptor) = arg(args, 0).as_object() else {
        return agent.type_error("WebAssembly.Table requires a descriptor");
    };
    let key = agent.heap.key("element")?;
    let element = agent.get(descriptor, key)?;
    let element = agent.to_string(element)?;
    let element = agent.heap.string(element);
    let functions = match element.len() {
        7 if element.to_text() == "anyfunc" => true,
        9 if element.to_text() == "externref" => false,
        _ => return agent.type_error("unsupported WebAssembly.Table element type"),
    };
    let address = address_type(agent, descriptor, "invalid WebAssembly table address type")?;
    let key = agent.heap.key("initial")?;
    let initial = agent.get(descriptor, key)?;
    let initial = index(agent, initial, address)?;
    let key = agent.heap.key("maximum")?;
    let maximum = agent.get(descriptor, key)?;
    let maximum = if maximum == Value::UNDEFINED {
        address.max_table_elements()
    } else {
        index(agent, maximum, address)?
    };
    if initial > maximum {
        return Err(agent.throw(ErrorKind::RangeError, "invalid WebAssembly table limits"));
    }
    let value = entry(agent, functions, args.get(1).copied())?;
    if initial > u64::from(TABLE_LIMIT) {
        return Err(agent.throw(ErrorKind::RangeError, "cannot allocate WebAssembly table"));
    }
    let proto = agent.host_prototype_from_new_target(TABLE, target)?;
    let state = Table {
        address,
        storage: Storage::Standalone {
            functions,
            capacity: maximum.min(u64::from(TABLE_LIMIT)) as u32,
        },
    };
    let object = Value::object(agent.new_host_state(TABLE, proto, state, 0)?);
    with_root(agent, object, |agent| {
        let entries = PrivateArray::new(agent, &[])?;
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.wasm_links, object, entries.value())
        })?;
        append(agent, entries, initial as u32, value)?;
        Ok(object)
    })
}

pub(super) fn export(
    agent: &mut Agent,
    owner: Value,
    index: u32,
    ty: TableType,
) -> JsResult<Value> {
    let proto = prototype(agent, TABLE)?;
    let state = Table {
        address: ty.limits.address_type,
        storage: Storage::Module {
            index,
            functions: matches!(ty.element_type, ValueType::Ref { heap_type: -16, .. }),
        },
    };
    let wrapper = Value::object(agent.new_host_state(TABLE, proto, state, 0)?);
    with_root(agent, wrapper, |agent| {
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.wasm_links, wrapper, owner)
        })
    })?;
    Ok(wrapper)
}

fn state(agent: &mut Agent, this: Value) -> JsResult<(Rc<Table>, Entries)> {
    let Some(table) = this
        .as_object()
        .and_then(|id| agent.host_state::<Table>(id, TABLE))
    else {
        return agent.type_error("invalid WebAssembly.Table receiver");
    };
    let backing = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.wasm_links, this))
    })?;
    let entries = match table.storage {
        Storage::Standalone {
            functions,
            capacity,
        } => Entries::Standalone {
            values: PrivateArray::from_value(backing).expect("standalone table entries"),
            functions,
            capacity,
        },
        Storage::Module { index, functions } => Entries::Module {
            owner: backing,
            runtime: instance::find(agent, backing).expect("exported table owner"),
            index,
            functions,
        },
    };
    Ok((table, entries))
}

fn index(agent: &mut Agent, value: Value, address: AddressType) -> JsResult<u64> {
    address_index(
        agent,
        value,
        address,
        "invalid WebAssembly table index or count",
    )
}

fn size_value(agent: &mut Agent, length: u64, address: AddressType) -> JsResult<Value> {
    match address {
        AddressType::I32 => Ok(Value::number(length as f64)),
        AddressType::I64 => agent.bigint_value(BigInt::from_u64(length)),
    }
}

fn local_function(agent: &mut Agent, owner: Value, value: Option<Value>) -> JsResult<Option<u32>> {
    let value = value.unwrap_or(Value::NULL);
    if value == Value::NULL {
        return Ok(None);
    }
    let Some(record) = host::function_record(agent, value)? else {
        return agent
            .type_error("WebAssembly function table entries must be Wasm functions or null");
    };
    let (instance, function) = host::function_parts(agent, record)?;
    if instance == owner {
        return Ok(Some(function));
    }
    let record = host::instance_record(agent, owner)?;
    let imports = PrivateArray::from_value(record.get(agent, 0)?).expect("function imports");
    for index in 0..imports.len(agent)? {
        agent.spend_native_work(1)?;
        if imports.get(agent, index)? == value {
            return Ok(Some(index));
        }
    }
    agent.type_error("this WebAssembly table requires functions linked to its instance")
}

fn entry(agent: &mut Agent, functions: bool, value: Option<Value>) -> JsResult<Value> {
    let value = value.unwrap_or(if functions {
        Value::NULL
    } else {
        Value::UNDEFINED
    });
    if functions && value != Value::NULL && host::function_record(agent, value)?.is_none() {
        return agent
            .type_error("WebAssembly function table entries must be Wasm functions or null");
    }
    Ok(value)
}

fn append(agent: &mut Agent, entries: PrivateArray, count: u32, value: Value) -> JsResult<()> {
    match entries.append_repeated(agent, count, value) {
        Err(Abrupt::OutOfMemory) => Err(agent.throw(
            ErrorKind::RangeError,
            "cannot allocate WebAssembly table entries",
        )),
        result => result,
    }
}

fn check_index(agent: &mut Agent, entries: PrivateArray, index: u64) -> JsResult<u32> {
    if index >= u64::from(entries.len(agent)?) {
        return Err(agent.throw(
            ErrorKind::RangeError,
            "WebAssembly table index out of bounds",
        ));
    }
    Ok(index as u32)
}

fn length(agent: &mut Agent, this: Value, _: &[Value]) -> JsResult<Value> {
    let (table, entries) = state(agent, this)?;
    let length = match entries {
        Entries::Standalone { values, .. } => u64::from(values.len(agent)?),
        Entries::Module { runtime, index, .. } => runtime.table_len(index).expect("exported table"),
    };
    size_value(agent, length, table.address)
}

fn get(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (table, entries) = state(agent, this)?;
    let index = index(agent, arg(args, 0), table.address)?;
    match entries {
        Entries::Standalone { values, .. } => {
            let index = check_index(agent, values, index)?;
            values.get(agent, index)
        }
        Entries::Module {
            owner,
            runtime,
            index: table,
            functions,
        } => {
            if functions {
                match runtime.function_table_get(table, index) {
                    Some(Some(function)) => {
                        return instance::function(agent, owner, &runtime, function);
                    }
                    Some(None) => return Ok(Value::NULL),
                    None => {}
                }
            } else if let Some(reference) = runtime.external_table_get(table, index) {
                return Ok(reference.map_or(Value::NULL, |reference| {
                    Value::from_host_identity(reference.identity())
                }));
            }
            Err(agent.throw(
                ErrorKind::RangeError,
                "WebAssembly table index out of bounds",
            ))
        }
    }
}

fn set(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (table, entries) = state(agent, this)?;
    let index = index(agent, arg(args, 0), table.address)?;
    let value = args.get(1).copied();
    match entries {
        Entries::Standalone {
            values, functions, ..
        } => {
            let value = entry(agent, functions, value)?;
            let index = check_index(agent, values, index)?;
            values.set(agent, index, value)?;
        }
        Entries::Module {
            owner,
            runtime,
            index: table,
            functions,
        } => {
            let updated = if functions {
                let function = local_function(agent, owner, value)?;
                runtime.function_table_set(table, index, function)
            } else {
                runtime.external_table_set(table, index, external_reference(value))
            };
            if !updated {
                return Err(agent.throw(
                    ErrorKind::RangeError,
                    "WebAssembly table index out of bounds",
                ));
            }
        }
    }
    Ok(Value::UNDEFINED)
}

fn grow(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let (table, entries) = state(agent, this)?;
    let count = index(agent, arg(args, 0), table.address)?;
    let value = args.get(1).copied();
    match entries {
        Entries::Standalone {
            values,
            functions,
            capacity,
        } => {
            let value = entry(agent, functions, value)?;
            // Converting the count can run script and grow this same table.
            let old = values.len(agent)?;
            if count > u64::from(capacity - old) {
                return Err(agent.throw(ErrorKind::RangeError, "cannot grow WebAssembly table"));
            }
            let result = size_value(agent, u64::from(old), table.address)?;
            append(agent, values, count as u32, value)?;
            Ok(result)
        }
        Entries::Module {
            owner,
            runtime,
            index,
            functions,
        } => {
            let function = if functions {
                local_function(agent, owner, value)?
            } else {
                None
            };
            let Some(work) = runtime.table_growth_work(index, count) else {
                return Err(agent.throw(ErrorKind::RangeError, "cannot grow WebAssembly table"));
            };
            agent.spend_native_work(work as u64)?;
            let old = runtime.table_len(index).expect("exported table");
            let result = size_value(agent, old, table.address)?;
            let token = agent.cancellation().unwrap_or_default();
            let previous = if functions {
                runtime.grow_function_table(index, count, function, &token)
            } else {
                runtime.grow_external_table(index, count, external_reference(value), &token)
            }
            .map_err(|error| instance::execution_error(agent, error, false))?;
            if previous.is_none() {
                return Err(agent.throw(ErrorKind::RangeError, "cannot grow WebAssembly table"));
            }
            Ok(result)
        }
    }
}

fn external_reference(value: Option<Value>) -> Option<ExternalReference> {
    let value = value.unwrap_or(Value::UNDEFINED);
    (value != Value::NULL).then_some(ExternalReference::new(value.host_identity()))
}

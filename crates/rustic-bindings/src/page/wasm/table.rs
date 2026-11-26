//! Standalone Window tables and live wrappers over module tables.

use rustic_js::runtime::ErrorKind;
use rustic_wasm::{
    ExternalReference, Limits, ReferenceRoot, ReferenceTable, TableEntry, TableType,
};

use super::super::events::PrivateArray;
use super::instance::{self, owner_value, root_value};
use super::*;

const TABLE: &str = "WebAssembly.Table";
const TABLE_STORAGE: &str = "WebAssembly table storage";
const TABLE_LIMIT: u32 = 65_536;

struct Table {
    address: AddressType,
    functions: bool,
    storage: Storage,
}

enum Storage {
    Standalone { capacity: u32, maximum: Option<u64> },
    Module { index: u32 },
}

enum Entries {
    Shared(ReferenceTable),
    Standalone { values: PrivateArray, capacity: u32 },
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
    let maximum = (maximum != Value::UNDEFINED)
        .then(|| index(agent, maximum, address))
        .transpose()?;
    if maximum.is_some_and(|maximum| initial > maximum) {
        return Err(agent.throw(ErrorKind::RangeError, "invalid WebAssembly table limits"));
    }
    let value = entry(agent, functions, args.get(1).copied())?;
    if initial > u64::from(TABLE_LIMIT) {
        return Err(agent.throw(ErrorKind::RangeError, "cannot allocate WebAssembly table"));
    }
    let proto = agent.host_prototype_from_new_target(TABLE, target)?;
    let state = Table {
        address,
        functions,
        storage: Storage::Standalone {
            capacity: maximum.unwrap_or(u64::MAX).min(u64::from(TABLE_LIMIT)) as u32,
            maximum,
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
        functions: ty.element_type == reference_type(true),
        storage: Storage::Module { index },
    };
    let wrapper = Value::object(agent.new_host_state(TABLE, proto, state, 0)?);
    with_root(agent, wrapper, |agent| {
        with_page(agent, |agent, page| {
            agent.weak_map_set(page.wasm_links, wrapper, owner)
        })
    })?;
    Ok(wrapper)
}

fn find(agent: &Agent, value: Value) -> Option<Rc<Table>> {
    agent.host_state(value.as_object()?, TABLE)
}

fn reference_type(functions: bool) -> ValueType {
    ValueType::Ref {
        nullable: true,
        heap_type: if functions { -16 } else { -17 },
    }
}

pub(super) fn accepts_import(agent: &Agent, value: Value, ty: TableType) -> bool {
    find(agent, value).is_some_and(|table| ty.element_type == reference_type(table.functions))
}

pub(super) fn link(agent: &mut Agent, value: Value, ty: TableType) -> JsResult<ReferenceTable> {
    let table = find(agent, value).expect("resolved table");
    let handle = match entries(agent, value, &table)? {
        Entries::Shared(storage) => storage,
        Entries::Standalone { values, capacity } => {
            let Storage::Standalone { maximum, .. } = table.storage else {
                unreachable!("standalone table");
            };
            let length = values.len(agent)?;
            if table.address != ty.limits.address_type
                || u64::from(length) < ty.limits.min
                || ty
                    .limits
                    .max
                    .is_some_and(|max| maximum.is_none_or(|actual| actual > max))
            {
                return Err(wasm_error(
                    agent,
                    instance::LINK_ERROR,
                    "WebAssembly table type mismatch",
                ));
            }
            return promote(
                agent,
                value,
                values,
                TableType {
                    limits: Limits {
                        address_type: table.address,
                        min: u64::from(length),
                        max: maximum,
                    },
                    element_type: ty.element_type,
                },
                capacity,
            );
        }
    };
    if !handle.matches(ty) {
        return Err(wasm_error(
            agent,
            instance::LINK_ERROR,
            "WebAssembly table type mismatch",
        ));
    }
    Ok(handle)
}

fn promote(
    agent: &mut Agent,
    wrapper: Value,
    values: PrivateArray,
    ty: TableType,
    capacity: u32,
) -> JsResult<ReferenceTable> {
    let charge = ReferenceTable::storage_bytes(capacity as usize);
    agent.spend_native_work(ty.limits.min)?;
    let handle = agent.with_host_memory(charge, |agent| {
        let handle = ReferenceTable::new(ty, capacity as usize).ok_or(OutOfMemory)?;
        for index in 0..ty.limits.min as u32 {
            agent.check_native_work()?;
            let value = values.get(agent, index)?;
            let entry = shared_entry(agent, ty.element_type == reference_type(true), Some(value))?;
            assert!(handle.set(u64::from(index), entry));
        }
        Ok(handle)
    })?;
    let storage = agent.new_traced_host_state(
        TABLE_STORAGE,
        agent.intrinsics.object_prototype,
        handle.clone(),
        charge,
        Some(trace_storage),
    )?;
    with_page(agent, |agent, page| {
        agent.weak_map_set(page.wasm_links, wrapper, Value::object(storage))
    })?;
    Ok(handle)
}

fn trace_storage(state: &dyn std::any::Any, mark: &mut dyn FnMut(Value)) {
    let storage = state
        .downcast_ref::<ReferenceTable>()
        .expect("table storage");
    storage.visit_reference_roots(&mut |root: ReferenceRoot| mark(root_value(root)));
}

fn receiver(agent: &mut Agent, this: Value) -> JsResult<Rc<Table>> {
    match find(agent, this) {
        Some(table) => Ok(table),
        None => agent.type_error("invalid WebAssembly.Table receiver"),
    }
}

fn entries(agent: &mut Agent, this: Value, table: &Table) -> JsResult<Entries> {
    let backing = with_page(agent, |agent, page| {
        Ok(agent.weak_map_get(page.wasm_links, this))
    })?;
    let entries = match table.storage {
        Storage::Standalone { capacity, .. } => {
            if let Some(storage) = backing
                .as_object()
                .and_then(|id| agent.host_state::<ReferenceTable>(id, TABLE_STORAGE))
            {
                Entries::Shared((*storage).clone())
            } else {
                Entries::Standalone {
                    values: PrivateArray::from_value(backing).expect("standalone table entries"),
                    capacity,
                }
            }
        }
        Storage::Module { index } => {
            let runtime = instance::find(agent, backing).expect("exported table owner");
            Entries::Shared(
                runtime
                    .shared_table(index)
                    .expect("exported table allocation"),
            )
        }
    };
    Ok(entries)
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

fn default_entry(functions: bool, value: Option<Value>) -> Value {
    value.unwrap_or(if functions {
        Value::NULL
    } else {
        Value::UNDEFINED
    })
}

fn entry(agent: &mut Agent, functions: bool, value: Option<Value>) -> JsResult<Value> {
    let value = default_entry(functions, value);
    if functions && value != Value::NULL && host::function_record(agent, value)?.is_none() {
        return agent
            .type_error("WebAssembly function table entries must be Wasm functions or null");
    }
    Ok(value)
}

fn shared_entry(agent: &mut Agent, functions: bool, value: Option<Value>) -> JsResult<TableEntry> {
    let value = default_entry(functions, value);
    if value == Value::NULL {
        return Ok(TableEntry::Null);
    }
    if !functions {
        return Ok(TableEntry::External(ExternalReference::new(
            value.host_identity(),
        )));
    }
    Ok(TableEntry::Function(host::function_address(agent, value)?))
}

pub(super) fn read_shared(agent: &mut Agent, entry: TableEntry) -> JsResult<Value> {
    match entry {
        TableEntry::Null => Ok(Value::NULL),
        TableEntry::External(reference) => Ok(Value::from_host_identity(reference.identity())),
        TableEntry::Function(address) => {
            let owner = owner_value(address.owner().expect("host-linked function owner"));
            let runtime = instance::find(agent, owner).expect("reachable function owner");
            instance::function(agent, owner, &runtime, address.index())
        }
    }
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
    let table = receiver(agent, this)?;
    let length = match entries(agent, this, &table)? {
        Entries::Shared(storage) => storage.length(),
        Entries::Standalone { values, .. } => u64::from(values.len(agent)?),
    };
    size_value(agent, length, table.address)
}

fn get(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let table = receiver(agent, this)?;
    let index = index(agent, arg(args, 0), table.address)?;
    match entries(agent, this, &table)? {
        Entries::Shared(storage) => {
            let entry = storage.get(index).ok_or_else(|| {
                agent.throw(
                    ErrorKind::RangeError,
                    "WebAssembly table index out of bounds",
                )
            })?;
            read_shared(agent, entry)
        }
        Entries::Standalone { values, .. } => {
            let index = check_index(agent, values, index)?;
            values.get(agent, index)
        }
    }
}

fn set(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let table = receiver(agent, this)?;
    let index = index(agent, arg(args, 0), table.address)?;
    let value = args.get(1).copied();
    match entries(agent, this, &table)? {
        Entries::Shared(storage) => {
            let entry = shared_entry(agent, table.functions, value)?;
            if !storage.set(index, entry) {
                return Err(agent.throw(
                    ErrorKind::RangeError,
                    "WebAssembly table index out of bounds",
                ));
            }
        }
        Entries::Standalone { values, .. } => {
            let value = entry(agent, table.functions, value)?;
            let index = check_index(agent, values, index)?;
            values.set(agent, index, value)?;
        }
    }
    Ok(Value::UNDEFINED)
}

fn grow(agent: &mut Agent, this: Value, args: &[Value]) -> JsResult<Value> {
    let table = receiver(agent, this)?;
    let count = index(agent, arg(args, 0), table.address)?;
    let value = args.get(1).copied();
    match entries(agent, this, &table)? {
        Entries::Shared(storage) => {
            let entry = shared_entry(agent, table.functions, value)?;
            let mut result = Value::UNDEFINED;
            let previous = storage.grow(count, entry, |work| {
                agent.spend_native_work(work as u64)?;
                result = size_value(agent, storage.length(), table.address)?;
                Ok::<(), Abrupt>(())
            })?;
            if previous.is_none() {
                return Err(agent.throw(ErrorKind::RangeError, "cannot grow WebAssembly table"));
            }
            Ok(result)
        }
        Entries::Standalone { values, capacity } => {
            let value = entry(agent, table.functions, value)?;
            // Converting the count can run script and grow this same table.
            let old = values.len(agent)?;
            if count > u64::from(capacity - old) {
                return Err(agent.throw(ErrorKind::RangeError, "cannot grow WebAssembly table"));
            }
            let result = size_value(agent, u64::from(old), table.address)?;
            append(agent, values, count as u32, value)?;
            Ok(result)
        }
    }
}

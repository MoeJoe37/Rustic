//! Dynamic import request conversion and the explicit loader boundary.

use super::ModuleRequest;
use crate::runtime::{
    Abrupt, Agent, Class, EnvId, ErrorKind, Internal, Job, JsResult, JsStr, Object, ObjectId,
    PropertyKey, StringId, Value, new_intrinsic_promise, reject_promise, watch_import,
};

/// Resolve a dynamic import to its namespace and evaluation-completion
/// promise. The referrer is the module identity supplied when linking, or
/// absent for code without a module identity. The loader runs as a host job.
pub type DynamicImport = std::rc::Rc<
    dyn Fn(&mut Agent, Option<JsStr>, &ModuleRequest) -> JsResult<(ObjectId, ObjectId)>,
>;

#[derive(Default)]
pub(crate) struct ImportLoader(Option<DynamicImport>);

impl std::fmt::Debug for ImportLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportLoader")
            .field("configured", &self.0.is_some())
            .finish()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ImportJob(ModuleRequest);

impl ImportJob {
    pub(crate) fn footprint(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of_val(self.0.specifier.as_slice())
            + std::mem::size_of_val(self.0.attributes.as_slice())
            + self
                .0
                .attributes
                .iter()
                .map(|(key, value)| 2 * (key.len() + value.len()))
                .sum::<usize>()
    }
}

impl Agent {
    /// Replace the agent's dynamic import loader. Without a loader, import
    /// promises reject with a TypeError.
    pub fn set_dynamic_import_loader(&mut self, loader: Option<DynamicImport>) {
        self.vm.import_loader = ImportLoader(loader);
    }

    pub(crate) fn queue_dynamic_import(
        &mut self,
        env: EnvId,
        source: Value,
        options: Value,
    ) -> JsResult<Value> {
        let promise = new_intrinsic_promise(self)?;
        let request = match self.convert_import_request(source, options) {
            Ok(request) => ImportJob(request),
            Err(Abrupt::Throw(reason)) => {
                reject_promise(self, promise, reason);
                return Ok(Value::object(promise));
            }
            Err(other) => return Err(other),
        };
        self.heap.charge(
            std::mem::size_of::<crate::runtime::Extra>() + std::mem::size_of::<ImportJob>(),
        )?;
        let mut object = Object::new(Class::Ordinary, None);
        object.extra_mut().internal = Some(Internal::ImportRequest(Box::new(request)));
        let request = self.heap.alloc_object(object)?;
        let referrer = self
            .module_referrer(env)
            .map_or(Value::UNDEFINED, Value::string);
        self.vm.jobs.push_back(Job::Import {
            promise: Value::object(promise),
            request: Value::object(request),
            referrer,
        });
        Ok(Value::object(promise))
    }

    fn convert_import_request(&mut self, source: Value, options: Value) -> JsResult<ModuleRequest> {
        let specifier = self.to_string(source)?;
        self.heap.charge(2 * self.heap.string_len(specifier))?;
        let specifier = self.heap.string(specifier).to_units();
        let mut attributes = Vec::new();
        if options != Value::UNDEFINED {
            let Some(options) = options.as_object() else {
                return self.type_error("import options must be an object");
            };
            let with = self.heap.key("with")?;
            let value = self.get(options, with)?;
            if value != Value::UNDEFINED {
                let Some(holder) = value.as_object() else {
                    return self.type_error("import attributes must be an object");
                };
                for key in self.own_property_keys(holder)? {
                    if matches!(key, PropertyKey::Symbol(_))
                        || !self
                            .get_own_property(holder, key)?
                            .is_some_and(|d| d.enumerable == Some(true))
                    {
                        continue;
                    }
                    let value = self.get(holder, key)?;
                    let Some(value) = value.as_string() else {
                        return self.type_error("an import attribute value must be a string");
                    };
                    let name_length = match key {
                        PropertyKey::String(name) => self.heap.string_len(name),
                        PropertyKey::Index(_) => 10,
                        PropertyKey::Symbol(_) => unreachable!("symbols are excluded"),
                    };
                    self.heap.charge(
                        std::mem::size_of::<(Vec<u16>, Vec<u16>)>()
                            + 2 * (name_length + self.heap.string_len(value)),
                    )?;
                    let name = match key {
                        PropertyKey::String(name) => self.heap.string(name).to_units(),
                        PropertyKey::Index(index) => index.to_string().encode_utf16().collect(),
                        PropertyKey::Symbol(_) => unreachable!("symbols are excluded"),
                    };
                    attributes.push((name, self.heap.string(value).to_units()));
                }
            }
        }
        Ok(ModuleRequest::dynamic(specifier, attributes))
    }

    pub(crate) fn run_import_job(
        &mut self,
        promise: ObjectId,
        request: ObjectId,
        referrer: Option<StringId>,
    ) -> JsResult<()> {
        let result = if let Some(loader) = self.vm.import_loader.0.clone() {
            let referrer = referrer.map(|id| self.heap.string(id).clone());
            let Some(Internal::ImportRequest(request)) = self.heap.object(request).internal()
            else {
                unreachable!("import job owns a request");
            };
            let request = request.0.clone();
            loader(self, referrer, &request)
        } else {
            Err(self.throw(
                ErrorKind::TypeError,
                "no dynamic module loader is configured",
            ))
        };
        match result {
            Ok((namespace, completion)) => watch_import(self, completion, promise, namespace),
            Err(Abrupt::Throw(reason)) => {
                reject_promise(self, promise, reason);
                Ok(())
            }
            Err(other) => Err(other),
        }
    }
}

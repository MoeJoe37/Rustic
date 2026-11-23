use rustic_js::ast::ProgramKind;
use rustic_js::module::{ExportTarget, ImportName, ModuleRecord};
use rustic_js::{ParseError, ParseLimits, Source};

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn parse(text: &str) -> ModuleRecord {
    let source = Source::from_str_limited(text, usize::MAX).unwrap();
    ModuleRecord::parse(&source, &ParseLimits::default()).unwrap()
}

#[test]
fn requests_deduplicate_by_specifier_and_attributes_in_source_order() {
    let module = parse(
        r#"
        export * from "first";
        import "second" with {type: "a", "mode": "b"};
        import "second" with {mode: "b", type: "a"};
        import "second" with {mode: "c", type: "a"};
        import "\uD800";
        export {} from "last";
        "#,
    );
    assert_eq!(module.program().kind, ProgramKind::Module);
    let requests = module.requests();
    assert_eq!(requests.len(), 5);
    assert_eq!(requests[0].specifier(), units("first"));
    assert_eq!(requests[1].specifier(), units("second"));
    assert_eq!(
        requests[1].attributes(),
        &[(units("mode"), units("b")), (units("type"), units("a"))]
    );
    assert_eq!(requests[2].attributes()[0].1, units("c"));
    assert_eq!(requests[3].specifier(), &[0xD800]);
    assert_eq!(requests[4].specifier(), units("last"));
    assert_eq!(module.star_exports()[0].index(), 0);
}

#[test]
fn imports_and_exports_retain_binding_identity() {
    let module = parse(
        r#"
        export { local as "forwarded", ns };
        import d, {"in" as local} from "base";
        import * as ns from "other";
        export {"z" as "𐐀"} from "extra";
        export * as bundle from "other";
        export const {x: [y = 2, ...tail], ...rest} = obj;
        export default function f() {}
        "#,
    );
    let imports = module.imports();
    assert_eq!(imports.len(), 3);
    assert_eq!(imports[0].local.as_ref(), "d");
    assert_eq!(imports[0].imported, ImportName::Binding(units("default")));
    assert_eq!(imports[1].local.as_ref(), "local");
    assert_eq!(imports[1].imported, ImportName::Binding(units("in")));
    assert_eq!(imports[2].imported, ImportName::Namespace);
    let exports = module.exports();
    assert_eq!(exports[0].name, units("forwarded"));
    assert_eq!(
        exports[0].target,
        ExportTarget::Indirect {
            request: imports[1].request,
            imported: imports[1].imported.clone()
        }
    );
    assert_eq!(
        exports[1].target,
        ExportTarget::Indirect {
            request: imports[2].request,
            imported: ImportName::Namespace
        }
    );
    assert_eq!(exports[2].name, units("𐐀"));
    assert!(
        matches!(&exports[2].target, ExportTarget::Indirect { request, imported: ImportName::Binding(name) } if request.index() == 2 && name == &units("z"))
    );
    assert_eq!(
        exports[3].target,
        ExportTarget::Indirect {
            request: imports[2].request,
            imported: ImportName::Namespace
        }
    );
    for (entry, local) in exports[4..].iter().zip(["y", "tail", "rest", "f"]) {
        assert_eq!(entry.target, ExportTarget::Local(local.into()));
    }
    assert_eq!(exports[7].name, units("default"));
}

#[test]
fn default_export_without_a_name_gets_a_synthetic_binding() {
    for source in [
        "export default 42",
        "export default function() {}",
        "export default class {}",
    ] {
        assert_eq!(parse(source).exports()[0].target, ExportTarget::Default);
    }
}

#[test]
fn static_records_preserve_early_errors_and_cancellation() {
    let source = Source::from_str_limited("export {missing}", usize::MAX).unwrap();
    assert!(matches!(
        ModuleRecord::parse(&source, &ParseLimits::default()),
        Err(ParseError::Syntax(_))
    ));
    let limits = ParseLimits::default();
    limits.cancellation.cancel();
    let source = Source::from_str_limited("export default 42", usize::MAX).unwrap();
    assert!(matches!(
        ModuleRecord::parse(&source, &limits),
        Err(ParseError::Cancelled)
    ));
}

fn load(
    files: &[(&str, &str)],
    limits: &rustic_js::module::GraphLimits,
) -> Result<rustic_js::module::ModuleGraph<String>, rustic_js::module::GraphError<&'static str>> {
    use std::collections::HashSet;
    let mut read = HashSet::new();
    rustic_js::module::ModuleGraph::load(
        "entry".into(),
        limits,
        |_: &String, request| {
            if !request.attributes().is_empty() {
                return Err("attributes unsupported by test host");
            }
            String::from_utf16(request.specifier()).map_err(|_| "bad specifier")
        },
        |key| {
            assert!(read.insert(key.clone()), "loaded {key} more than once");
            let (_, text) = files
                .iter()
                .find(|(name, _)| name == key)
                .ok_or("no file")?;
            Source::from_str_limited(text, usize::MAX).map_err(|_| "too long")
        },
    )
}

#[test]
fn static_graph_resolves_cycles_and_same_binding_diamonds() {
    use rustic_js::module::{GraphLimits, ResolvedBinding, ResolvedExport};
    let limits = GraphLimits::default();
    let files = [
        ("entry", "export * from 'a'; export * from 'b';"),
        (
            "a",
            "export * from 'entry'; export {x as shared} from 'c'; export * as ns from 'c';",
        ),
        (
            "b",
            "import {x as y} from 'c'; export {y as shared}; import * as ns from 'c'; export {ns};",
        ),
        ("c", "export let x; export default 1;"),
    ];
    let graph = load(&files, &limits).unwrap();
    assert_eq!(graph.module(3).unwrap().0, "c");
    let entry = graph.module(0).unwrap().1;
    assert_eq!(graph.dependency(0, entry.star_exports()[1]), Some(2));
    assert_eq!(
        graph.exported_names(0, &limits),
        Ok(vec![units("ns"), units("shared")])
    );
    assert_eq!(
        graph.exported_names(3, &limits),
        Ok(vec![units("default"), units("x")])
    );
    assert_eq!(
        graph.resolve_export(0, &units("shared"), &limits),
        Ok(ResolvedExport::Binding {
            module: 3,
            binding: ResolvedBinding::Local("x".into())
        })
    );
    assert_eq!(
        graph.resolve_export(0, &units("ns"), &limits),
        Ok(ResolvedExport::Binding {
            module: 3,
            binding: ResolvedBinding::Namespace
        })
    );
    assert_eq!(
        graph.resolve_export(0, &units("default"), &limits),
        Ok(ResolvedExport::Missing)
    );
    assert_eq!(
        graph.resolve_export(3, &units("default"), &limits),
        Ok(ResolvedExport::Binding {
            module: 3,
            binding: ResolvedBinding::Default
        })
    );
}

#[test]
fn static_graph_validates_missing_and_ambiguous_imports() {
    use rustic_js::module::{GraphError, GraphLimits, ResolvedExport};
    let limits = GraphLimits::default();
    let mut files = [
        ("entry", "import 'm';"),
        ("m", "export * from 'a'; export * from 'b';"),
        ("a", "export let x;"),
        ("b", "export let x;"),
    ];
    let graph = load(&files, &limits).unwrap();
    assert_eq!(
        graph.resolve_export(1, &units("x"), &limits),
        Ok(ResolvedExport::Ambiguous)
    );
    files[0].1 = "import {x} from 'm';";
    assert!(
        matches!(load(&files, &limits), Err(GraphError::UnresolvedExport { importer: 0, ambiguous: true, name }) if name == units("x"))
    );
    files[0].1 = "export {missing} from 'm';";
    assert!(
        matches!(load(&files, &limits), Err(GraphError::UnresolvedExport { importer: 0, ambiguous: false, name }) if name == units("missing"))
    );
    files[0].1 = "export {x} from 'm';";
    files[1].1 = "export * from 'a'; export * from 'b'; export let x;";
    assert!(load(&files, &limits).is_ok());
}

#[test]
fn static_graph_enforces_host_policy_and_exact_budgets() {
    use rustic_js::module::{GraphError, GraphLimits};
    let files = [
        ("entry", "import {x} from 'dep';"),
        ("dep", "export let x;"),
    ];
    let size = files.iter().map(|(_, text)| text.len()).sum();
    let mut limits = GraphLimits {
        max_modules: 2,
        max_source_units: size,
        max_resolution_steps: 1,
        ..GraphLimits::default()
    };
    assert!(load(&files, &limits).is_ok());
    limits.max_modules -= 1;
    assert!(matches!(load(&files, &limits), Err(GraphError::GraphLimit)));
    limits.max_modules += 1;
    limits.max_source_units -= 1;
    assert!(matches!(load(&files, &limits), Err(GraphError::GraphLimit)));
    limits.max_source_units += 1;
    limits.max_resolution_steps = 0;
    assert!(matches!(
        load(&files, &limits),
        Err(GraphError::ResolutionLimit)
    ));
    assert!(matches!(
        load(&[("entry", "import 'dep' with {type: 'json'};")], &limits),
        Err(GraphError::Host("attributes unsupported by test host"))
    ));
    assert!(matches!(
        load(&[("entry", "import 'missing';")], &limits),
        Err(GraphError::Host("no file"))
    ));
    limits.parse.cancellation.cancel();
    assert!(matches!(load(&files, &limits), Err(GraphError::Cancelled)));
}

#[test]
fn modules_share_live_bindings_and_evaluate_each_dependency_once() {
    use rustic_js::module::GraphLimits;
    use rustic_js::runtime::{Agent, Roots, Value};

    let limits = GraphLimits::default();
    let graph = load(
        &[
            (
                "entry",
                r#"
                import { count, bump } from "middle";
                import { ns } from "middle";
                import * as again from "base";
                export { ns as library };
                export const before = count;
                export function advance() {
                    bump();
                    return eval("count") + ns.count;
                }
                export const sameNamespace = ns === again;
                export const order = globalThis.order;
                export const moduleThis = (() => this)();
                export const numeric = Reflect.ownKeys(ns).slice(0, 2).join(',');
                export const traits = Object.getPrototypeOf(ns) === null
                    && Object.prototype.toString.call(ns) === '[object Module]'
                    && !Object.isExtensible(ns) && !Reflect.set(ns, "count", 9)
                    && !Reflect.deleteProperty(ns, "count")
                    && Reflect.defineProperty(ns, "count", { value: count })
                    && !Reflect.defineProperty(ns, "count", { writable: false });
                try { count = 10; } catch (e) { globalThis.immutable = e instanceof TypeError; }
                "#,
            ),
            (
                "middle",
                r#"
                export * from "base";
                export * as ns from "base";
                globalThis.order += "m";
                "#,
            ),
            (
                "base",
                r#"
                globalThis.order = (globalThis.order || "") + "b";
                export let count = 1;
                export { count as "2", count as "10" };
                export function bump() { count++; }
                "#,
            ),
        ],
        &limits,
    )
    .unwrap();
    let mut agent = Agent::new().unwrap();
    agent.heap.set_gc_stress(true);
    let namespace = agent.evaluate_module_graph(&graph, &limits).unwrap();
    let root = agent.root(Value::object(namespace)).unwrap();

    let read = |agent: &mut Agent, name| {
        let key = agent.heap.key(name).unwrap();
        agent.get(namespace, key).unwrap()
    };
    assert_eq!(read(&mut agent, "before").as_number(), Some(1.0));
    assert_eq!(read(&mut agent, "sameNamespace"), Value::TRUE);
    assert_eq!(read(&mut agent, "moduleThis"), Value::UNDEFINED);
    assert_eq!(read(&mut agent, "traits"), Value::TRUE);
    for (name, expected) in [("order", "bm"), ("numeric", "10,2")] {
        let value = read(&mut agent, name);
        assert_eq!(
            agent.heap.string(value.as_string().unwrap()).to_text(),
            expected
        );
    }
    assert_eq!(agent.evaluate("immutable").unwrap(), Value::TRUE);
    assert_eq!(
        agent
            .evaluate("typeof before")
            .unwrap()
            .as_string()
            .map(|s| agent.heap.string(s).to_text())
            .as_deref(),
        Some("undefined")
    );

    agent.collect_garbage(Roots::default());
    let advance = read(&mut agent, "advance");
    assert_eq!(
        agent
            .call_from_host(advance, Value::UNDEFINED, &[])
            .unwrap()
            .as_number(),
        Some(4.0)
    );
    let library = read(&mut agent, "library").as_object().unwrap();
    let count = agent.heap.key("count").unwrap();
    assert_eq!(agent.get(library, count).unwrap().as_number(), Some(2.0));
    assert!(agent.unroot(root));
}

#[test]
fn cyclic_modules_hoist_functions_and_vars_but_preserve_lexical_tdz() {
    use rustic_js::module::{GraphLimits, ModuleEvalError};
    use rustic_js::runtime::{Abrupt, Agent};

    let limits = GraphLimits::default();
    let mut agent = Agent::new().unwrap();
    for (body, throws) in [
        (
            "import f, { pending } from 'entry'; export const result = f() + (pending === undefined);",
            false,
        ),
        (
            "import { lex } from 'entry'; export const result = lex;",
            true,
        ),
    ] {
        let graph = load(
            &[
                ("entry", "export { result } from 'dep'; export default function () { return 8; } export var pending = 2; export let lex = 9;"),
                ("dep", body),
            ],
            &limits,
        )
        .unwrap();
        match agent.evaluate_module_graph(&graph, &limits) {
            Ok(namespace) if !throws => {
                let key = agent.heap.key("result").unwrap();
                assert_eq!(agent.get(namespace, key).unwrap().as_number(), Some(9.0));
            }
            Err(ModuleEvalError::Runtime(Abrupt::Throw(error))) if throws => {
                let key = agent.heap.key("name").unwrap();
                let name = agent.get_v(error, key).unwrap().as_string().unwrap();
                assert_eq!(agent.heap.string(name).to_text(), "ReferenceError");
            }
            other => panic!("unexpected result {other:?}"),
        }
    }
}

#[test]
fn modules_reject_unsupported_execution_before_running_dependencies() {
    use rustic_js::module::{GraphLimits, ModuleEvalError};
    use rustic_js::runtime::Agent;

    let limits = GraphLimits::default();
    let graph = load(
        &[
            ("entry", "import 'dep'; await 1;"),
            ("dep", "globalThis.ran = true;"),
        ],
        &limits,
    )
    .unwrap();
    let mut agent = Agent::new().unwrap();
    assert!(matches!(
        agent.evaluate_module_graph(&graph, &limits),
        Err(ModuleEvalError::Compile { module: 0, .. })
    ));
    let value = agent.evaluate("typeof ran").unwrap();
    assert_eq!(
        agent.heap.string(value.as_string().unwrap()).to_text(),
        "undefined"
    );
}

#[test]
fn top_level_await_allows_siblings_and_delays_importers() {
    use rustic_js::module::GraphLimits;
    use rustic_js::runtime::{Agent, Roots, Value};

    let limits = GraphLimits::default();
    let graph = load(&[
        ("entry", "import { value } from 'async'; import 'sibling'; export const result = value; log += 'entry ';"),
        ("async", "log += 'async '; await new Promise(r => globalThis.release = r); export const value = 42;"),
        ("sibling", "log += 'sibling ';"),
    ], &limits).unwrap();
    let mut agent = Agent::new().unwrap();
    agent.heap.set_gc_stress(true);
    agent.evaluate("var log = ''").unwrap();
    let (namespace, completion) = agent.start_module_evaluation(&graph, &limits).unwrap();
    let namespace_root = agent.root(Value::object(namespace)).unwrap();
    let completion_root = agent.root(Value::object(completion)).unwrap();
    agent.collect_garbage(Roots::default());
    let log = agent.evaluate("log").unwrap();
    assert_eq!(
        agent.heap.string(log.as_string().unwrap()).to_text(),
        "async sibling "
    );
    assert_eq!(agent.promise_rejected(completion), None);
    agent.evaluate("release()").unwrap();
    agent.run_jobs().unwrap();
    assert_eq!(agent.promise_rejected(completion), Some(false));
    let result = agent.heap.key("result").unwrap();
    assert_eq!(agent.get(namespace, result), Ok(Value::number(42.0)));
    assert!(agent.unroot(namespace_root));
    assert!(agent.unroot(completion_root));
}

#[test]
fn top_level_await_rejects_cyclic_importers_without_running_their_bodies() {
    use rustic_js::module::GraphLimits;
    use rustic_js::runtime::{Agent, Value};

    let limits = GraphLimits::default();
    let graph = load(
        &[
            ("entry", "import 'a'; var never; globalThis.ran = true;"),
            ("a", "import 'b';"),
            ("b", "import 'a'; await Promise.reject(71);"),
        ],
        &limits,
    )
    .unwrap();
    let mut agent = Agent::new().unwrap();
    let (_, completion) = agent.start_module_evaluation(&graph, &limits).unwrap();
    let root = agent.root(Value::object(completion)).unwrap();
    agent.run_jobs().unwrap();
    assert_eq!(agent.promise_rejected(completion), Some(true));
    assert_eq!(agent.promise_result(completion), Some(Value::number(71.0)));
    assert_eq!(agent.evaluate("'ran' in globalThis"), Ok(Value::FALSE));
    assert!(agent.unroot(root));
}

#[test]
fn import_meta_is_lazy_and_distinct_for_each_module() {
    use rustic_js::module::GraphLimits;
    use rustic_js::runtime::{Agent, Value};

    let limits = GraphLimits::default();
    let graph = load(
        &[
            (
                "entry",
                r#"
                import { metadata } from "dep";
                export function own() { return import.meta; }
                own().label = "entry";
                export const valid = own() === import.meta
                    && metadata() !== own()
                    && Object.getPrototypeOf(own()) === null
                    && metadata().label === undefined;
            "#,
            ),
            ("dep", "export function metadata() { return import.meta; }"),
        ],
        &limits,
    )
    .unwrap();
    let mut agent = Agent::new().unwrap();
    agent.heap.set_gc_stress(true);
    let namespace = agent.evaluate_module_graph(&graph, &limits).unwrap();
    let valid = agent.heap.key("valid").unwrap();
    assert_eq!(agent.get(namespace, valid), Ok(Value::TRUE));
}

#[test]
fn import_meta_uses_host_metadata_for_each_module() {
    use rustic_js::module::GraphLimits;
    use rustic_js::runtime::{Agent, JsStr, Value};

    let limits = GraphLimits::default();
    let graph = load(
        &[
            (
                "entry",
                r#"
                import { read } from 'dep';
                import 'unused';
                export const valid = read() === 'dep'
                    && import.meta.url === 'entry'
                    && Object.getPrototypeOf(import.meta) === null
                    && Object.getOwnPropertyDescriptor(import.meta, 'url').enumerable;
            "#,
            ),
            ("dep", "export function read() { return import.meta.url; }"),
            ("unused", "export const value = 1;"),
        ],
        &limits,
    )
    .unwrap();
    let mut agent = Agent::new().unwrap();
    agent.heap.set_gc_stress(true);
    let mut visited = Vec::new();
    let namespace = agent
        .evaluate_module_graph_with_metadata(&graph, &limits, |key| {
            visited.push(key.clone());
            vec![
                (JsStr::from_text("url"), JsStr::from_text("superseded")),
                (JsStr::from_text("url"), JsStr::from_text(key)),
            ]
        })
        .unwrap();
    assert_eq!(visited, ["entry", "dep"]);
    let valid = agent.heap.key("valid").unwrap();
    assert_eq!(agent.get(namespace, valid), Ok(Value::TRUE));
}

#[test]
fn import_meta_metadata_obeys_the_graph_work_limit() {
    use rustic_js::module::{GraphError, GraphLimits, ModuleEvalError};
    use rustic_js::runtime::{Agent, JsStr};

    let graph = load(&[("entry", "import.meta;")], &GraphLimits::default()).unwrap();
    let mut agent = Agent::new().unwrap();
    let mut limits = GraphLimits {
        max_resolution_steps: 2,
        ..GraphLimits::default()
    };
    let properties = |_: &String| {
        ["first", "second"].map(|name| (JsStr::from_text(name), JsStr::from_text("value")))
    };
    assert_eq!(
        agent.evaluate_module_graph_with_metadata(&graph, &limits, properties),
        Err(ModuleEvalError::Graph(GraphError::ResolutionLimit))
    );
    // Two metadata entries plus the entry module's export-name visit.
    limits.max_resolution_steps = 3;
    assert!(
        agent
            .evaluate_module_graph_with_metadata(&graph, &limits, properties)
            .is_ok()
    );
}

#[test]
fn synthetic_modules_export_data_with_shared_identity() {
    use rustic_js::module::{GraphLimits, ModuleGraph, ModuleSource};
    use rustic_js::runtime::{Agent, Value};

    let limits = GraphLimits::default();
    let source = |text: &str| Source::from_str_limited(text, 1_024).unwrap();
    let graph = ModuleGraph::load_sources(
        "entry".to_owned(),
        &limits,
        |_, request| String::from_utf16(request.specifier()).map_err(|_| "invalid specifier"),
        |key| match key.as_str() {
            "entry" => Ok(ModuleSource::JavaScript(source(
                r#"
                import a from 'json' with { type: 'json' };
                import * as b from 'json' with { type: 'json' };
                import text from 'text' with { type: 'text' };
                export const valid = a === b.default && a.x === 42
                    && Object.keys(b).join() === 'default'
                    && text.length === 1 && text.charCodeAt(0) === 0xD800;
            "#,
            ))),
            "json" => Ok(ModuleSource::Json(source(r#"{"x":42}"#))),
            "text" => Ok(ModuleSource::Text(
                Source::from_utf16(vec![0xD800], 1).unwrap(),
            )),
            _ => Err("missing fixture"),
        },
    )
    .unwrap();
    let mut agent = Agent::new().unwrap();
    agent.heap.set_gc_stress(true);
    let namespace = agent.evaluate_module_graph(&graph, &limits).unwrap();
    let key = agent.heap.key("valid").unwrap();
    assert_eq!(agent.get(namespace, key), Ok(Value::TRUE));
}

#[test]
fn invalid_json_is_rejected_before_a_module_body_runs() {
    use rustic_js::module::{GraphLimits, ModuleEvalError, ModuleGraph, ModuleSource};
    use rustic_js::runtime::{Agent, Value};

    let limits = GraphLimits::default();
    let graph = ModuleGraph::load_sources(
        "entry",
        &limits,
        |_, _| Ok::<_, ()>("json"),
        |key| {
            let source = |text| Source::from_str_limited(text, 128).unwrap();
            Ok(if *key == "entry" {
                ModuleSource::JavaScript(source("import 'json'; globalThis.ran = true;"))
            } else {
                ModuleSource::Json(source("{ invalid }"))
            })
        },
    )
    .unwrap();
    let mut agent = Agent::new().unwrap();
    assert_eq!(
        agent.start_module_evaluation(&graph, &limits),
        Err(ModuleEvalError::InvalidJson { module: 1 })
    );
    assert_eq!(agent.evaluate("'ran' in globalThis"), Ok(Value::FALSE));
}

#[test]
fn dynamic_import_preserves_referrer_attributes_and_pending_evaluation() {
    use rustic_js::module::GraphLimits;
    use rustic_js::runtime::{Agent, JsStr, Value};
    use std::rc::Rc;

    let limits = GraphLimits::default();
    let graph = load(
        &[(
            "entry",
            "export function run() { return import('dep', { with: { z: '2', a: '1' } }) }",
        )],
        &limits,
    )
    .unwrap();
    let mut agent = Agent::new().unwrap();
    agent.heap.set_gc_stress(true);
    agent.set_dynamic_import_loader(Some(Rc::new(|agent, referrer, request| {
        assert_eq!(referrer.unwrap().to_text(), "entry");
        assert_eq!(request.specifier(), units("dep"));
        assert_eq!(
            request.attributes(),
            &[(units("a"), units("1")), (units("z"), units("2"))]
        );
        let limits = GraphLimits::default();
        let target = load(
            &[(
                "entry",
                "await new Promise(r => globalThis.release = r); export const answer = 42;",
            )],
            &limits,
        )
        .unwrap();
        Ok(agent.start_module_evaluation(&target, &limits).unwrap())
    })));
    let (namespace, _) = agent
        .start_module_evaluation_with_referrers(&graph, &limits, |key| Some(JsStr::from_text(key)))
        .unwrap();
    let key = agent.heap.key("run").unwrap();
    let function = agent.get(namespace, key).unwrap();
    let root = agent.root(function).unwrap();
    let import = agent
        .call_from_host(function, Value::UNDEFINED, &[])
        .unwrap();
    let import_root = agent.root(import).unwrap();
    let promise = import.as_object().unwrap();
    agent.run_jobs().unwrap();
    assert_eq!(agent.promise_rejected(promise), None);
    agent.evaluate("release()").unwrap();
    agent.run_jobs().unwrap();
    assert_eq!(agent.promise_rejected(promise), Some(false));
    let namespace = agent.promise_result(promise).unwrap().as_object().unwrap();
    let answer = agent.heap.key("answer").unwrap();
    assert_eq!(agent.get(namespace, answer), Ok(Value::number(42.0)));
    assert!(agent.unroot(root));
    assert!(agent.unroot(import_root));
}

#[test]
fn byte_modules_expose_read_only_shared_data() {
    use rustic_js::module::{GraphLimits, ModuleGraph, ModuleSource};
    use rustic_js::runtime::{Agent, Value};

    let limits = GraphLimits::default();
    let graph = ModuleGraph::load_sources(
        "entry",
        &limits,
        |_, _| Ok::<_, ()>("bytes"),
        |key| Ok(if *key == "entry" {
            ModuleSource::JavaScript(Source::from_str_limited(r#"
                import view from 'bytes' with { type: 'bytes' };
                import * as ns from 'bytes' with { type: 'bytes' };
                const buffer = view.buffer;
                const alias = new Uint8Array(buffer);
                const data = new DataView(buffer);
                const rejected = f => { try { f(); return false } catch (e) { return e instanceof TypeError } };
                export const valid = ns.default === view && buffer.immutable
                    && !Reflect.set(alias, '0', 1)
                    && !Reflect.defineProperty(alias, '0', { value: 1 })
                    && rejected(() => data.setUint8(0, 1))
                    && rejected(() => alias.copyWithin(0, 1))
                    && rejected(() => alias.fill(1))
                    && rejected(() => alias.reverse())
                    && rejected(() => alias.set([1]))
                    && rejected(() => alias.setFromHex('00'))
                    && rejected(() => alias.setFromBase64('AA=='))
                    && rejected(() => Atomics.store(alias, 0, 1))
                    && rejected(() => buffer.transfer())
                    && alias.toHex() === 'ff80' && alias.toBase64() === '/4A='
                    && alias[0] === 255 && alias[1] === 128;
            "#, 8_192).unwrap())
        } else {
            ModuleSource::Bytes(vec![255, 128])
        }),
    ).unwrap();
    let mut agent = Agent::new().unwrap();
    agent.heap.set_gc_stress(true);
    let namespace = agent.evaluate_module_graph(&graph, &limits).unwrap();
    let key = agent.heap.key("valid").unwrap();
    assert_eq!(agent.get(namespace, key), Ok(Value::TRUE));
}

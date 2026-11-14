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

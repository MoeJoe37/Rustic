use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use rustic_base::ManualClock;
use rustic_dom::Document;
use rustic_engine::{
    DocumentTurn, Engine, EngineDocument, LayoutGeometry, LayoutReadError, LayoutSource,
    MAX_LAYOUT_FLUSHES_PER_TURN, RenderFrame, ScriptStop, ScriptTurn, ScriptedDocument, TaskLimits,
};
use rustic_html::{DocumentMode, ParserLimits};
use rustic_js::Source;

fn document(engine: &mut Engine, limits: TaskLimits) -> ScriptedDocument<ManualClock> {
    engine
        .begin_document(ParserLimits::default())
        .unwrap()
        .finish()
        .unwrap()
        .into_scripted_with_clock(limits, ManualClock::default())
        .unwrap()
}

fn source(text: &str) -> Source {
    Source::from_str_limited(text, 4096).unwrap()
}

fn unexpected_render(_: &EngineDocument, _: RenderFrame) {
    panic!("unexpected rendering turn");
}

fn render_turn(scripts: &mut ScriptedDocument<ManualClock>) -> RenderFrame {
    match scripts
        .run_next_with_render(|_, frame| frame.timestamp())
        .unwrap()
    {
        Some(DocumentTurn::Rendered { frame, output }) => {
            assert_eq!(output, frame.timestamp());
            frame
        }
        other => panic!("expected a rendering turn, got {other:?}"),
    }
}

#[test]
fn rendering_follows_script_checkpoints_and_alternates_with_ready_work() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts
        .queue_script(source(
            "var trace = ''; Promise.resolve().then(() => trace += 'a');",
        ))
        .unwrap();
    scripts
        .queue_script(source("if (trace !== 'ab') throw 'timer order';"))
        .unwrap();
    scripts
        .schedule_script(
            Duration::ZERO,
            source(
                "if (trace !== 'a') throw 'checkpoint order';
                 Promise.resolve().then(() => trace += 'b');",
            ),
        )
        .unwrap();

    scripts.clock().advance(Duration::from_millis(3)).unwrap();
    scripts.offer_rendering_opportunity().unwrap();
    scripts.clock().advance(Duration::from_millis(4)).unwrap();
    scripts.offer_rendering_opportunity().unwrap();
    scripts.clock().advance(Duration::from_millis(1)).unwrap();

    assert_eq!(
        scripts.run_next_with_render(unexpected_render),
        Ok(Some(DocumentTurn::Script(ScriptTurn::Completed)))
    );
    let first = render_turn(&mut scripts);
    assert_eq!(first.sequence(), 1);
    assert_eq!(first.timestamp(), Duration::from_millis(7));

    scripts.offer_rendering_opportunity().unwrap();
    assert_eq!(
        scripts.run_next_with_render(unexpected_render),
        Ok(Some(DocumentTurn::Script(ScriptTurn::Completed)))
    );
    let second = render_turn(&mut scripts);
    assert_eq!(second.sequence(), 2);
    assert_eq!(second.timestamp(), Duration::from_millis(8));
    assert_eq!(scripts.accept_rendered_frame(first), Ok(false));
    assert_eq!(scripts.accept_rendered_frame(second), Ok(true));
    assert_eq!(scripts.accept_rendered_frame(second), Ok(false));
    assert_eq!(
        scripts.run_next_with_render(unexpected_render),
        Ok(Some(DocumentTurn::Script(ScriptTurn::Completed)))
    );
    assert_eq!(scripts.run_next_with_render(unexpected_render), Ok(None));
}

#[test]
fn frames_are_document_scoped_and_script_only_pumping_keeps_offers_pending() {
    let mut engine = Engine::new(2);
    let mut first = document(&mut engine, TaskLimits::default());
    let mut second = document(&mut engine, TaskLimits::default());
    first.offer_rendering_opportunity().unwrap();
    second.offer_rendering_opportunity().unwrap();

    assert_eq!(first.run_next(), Ok(None));
    let first_frame = render_turn(&mut first);
    let second_frame = render_turn(&mut second);
    assert_eq!(first_frame.sequence(), second_frame.sequence());
    assert_eq!(first_frame.timestamp(), second_frame.timestamp());
    assert_eq!(second.accept_rendered_frame(first_frame), Ok(false));
    assert_eq!(second.accept_rendered_frame(second_frame), Ok(true));
    assert_eq!(first.accept_rendered_frame(first_frame), Ok(true));
}

#[test]
fn mutations_invalidate_frame_tickets_after_animation_and_promise_work() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts
        .queue_script(source(
            "var item = document.createElement('p');
         var text = document.createTextNode('before');
         item.appendChild(text);
         document.querySelector('body').appendChild(item);
         requestAnimationFrame(() => {
             Promise.resolve().then(() => item.setAttribute('data-frame', 'ready'));
         });",
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    scripts.offer_rendering_opportunity().unwrap();
    let from_callback = render_turn(&mut scripts);
    scripts
        .queue_script(source(
            "if (item.getAttribute('data-frame') !== 'ready') throw 1;
         item.setAttribute('data-frame', 'ready');",
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.accept_rendered_frame(from_callback), Ok(true));

    for mutation in [
        "Promise.resolve().then(() => text.data = 'updated');",
        "item.setAttribute('class', 'updated');",
        "item.removeChild(text);",
    ] {
        scripts.offer_rendering_opportunity().unwrap();
        let stale = render_turn(&mut scripts);
        scripts.queue_script(source(mutation)).unwrap();
        assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
        assert_eq!(scripts.accept_rendered_frame(stale), Ok(false));
        scripts.offer_rendering_opportunity().unwrap();
        let fresh = render_turn(&mut scripts);
        assert_eq!(scripts.accept_rendered_frame(fresh), Ok(true));
    }
}

#[derive(Debug)]
struct LayoutFixture(Rc<Cell<i32>>);

impl LayoutSource for LayoutFixture {
    fn measure_layout(
        &self,
        document: &Document,
        _: DocumentMode,
    ) -> Result<LayoutGeometry, LayoutReadError> {
        let mut geometry = LayoutGeometry::default();
        for (node, _) in document.walk(document.root()).unwrap() {
            geometry.offset_widths.insert(node, self.0.get());
            geometry
                .client_rects
                .insert(node, Some([0.0, 0.0, f64::from(self.0.get()), 10.0]));
        }
        Ok(geometry)
    }
}

#[test]
fn layout_flushes_once_per_revision_with_a_shared_turn_budget() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    let width = Rc::new(Cell::new(30));
    scripts
        .set_layout_source(Box::new(LayoutFixture(width.clone())))
        .unwrap();
    let run = |scripts: &mut ScriptedDocument<ManualClock>, code: &str| {
        scripts.queue_script(source(code)).unwrap();
        assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    };
    run(
        &mut scripts,
        "
        var body = document.body;
        if (body.offsetWidth !== 30 || body.offsetWidth !== 30) throw 'first snapshot';
        Promise.resolve().then(() => {
            if (body.getBoundingClientRect().width !== 30) throw 'checkpoint snapshot';
        });
        ",
    );
    assert_eq!(scripts.take_layout_flush_count(), 1);
    scripts.offer_rendering_opportunity().unwrap();
    let stale = render_turn(&mut scripts);
    width.set(70);
    scripts.rendering_inputs_changed().unwrap();
    assert_eq!(scripts.accept_rendered_frame(stale), Ok(false));
    run(
        &mut scripts,
        &format!(
            "
            if (body.offsetWidth !== 70) throw 'applied inputs';
            for (let i = 1; i < {MAX_LAYOUT_FLUSHES_PER_TURN}; ++i) {{
                body.setAttribute('data-state', String(i));
                if (body.offsetWidth !== 70 || body.offsetWidth !== 70) throw 'dirty snapshot';
            }}
            Promise.resolve().then(() => {{
                body.setAttribute('data-state', 'from promise');
                try {{ body.offsetWidth; }} catch (error) {{ globalThis.limit = error.name; }}
            }});
            "
        ),
    );
    assert_eq!(
        scripts.take_layout_flush_count(),
        MAX_LAYOUT_FLUSHES_PER_TURN as u64
    );
    run(
        &mut scripts,
        "if (limit !== 'QuotaExceededError' || body.offsetWidth !== 70) throw 'next turn';",
    );
    assert_eq!(scripts.take_layout_flush_count(), 1);
    scripts.offer_rendering_opportunity().unwrap();
    let fresh = render_turn(&mut scripts);
    run(
        &mut scripts,
        "if (body.offsetWidth !== 70) throw 'same snapshot';",
    );
    assert_eq!(scripts.take_layout_flush_count(), 0);
    assert_eq!(scripts.accept_rendered_frame(fresh), Ok(true));
}

#[derive(Debug)]
struct AppliedResourceFixture {
    stylesheet_width: Rc<Cell<i32>>,
    image_width: Rc<Cell<i32>>,
}

impl LayoutSource for AppliedResourceFixture {
    fn measure_layout(
        &self,
        document: &Document,
        _: DocumentMode,
    ) -> Result<LayoutGeometry, LayoutReadError> {
        let mut geometry = LayoutGeometry::default();
        let stylesheet = self.stylesheet_width.get();
        let image = self.image_width.get();
        let width = stylesheet + image;
        for (node, _) in document.walk(document.root()).unwrap() {
            if !matches!(document.node(node).unwrap().kind(), rustic_dom::NodeKind::Element(_)) {
                continue;
            }
            geometry.offset_widths.insert(node, width);
            geometry
                .client_rects
                .insert(node, Some([0.0, 0.0, f64::from(width), 12.0]));
            geometry.computed_styles.insert(
                node,
                rustic_engine::ComputedStyleSnapshot {
                    longhands: vec![("width", format!("{stylesheet}px"))],
                    custom_properties: vec![(
                        "--image-width".to_owned(),
                        format!("{image}px"),
                    )],
                },
            );
        }
        Ok(geometry)
    }
}

#[test]
fn resource_completions_publish_only_after_application_and_offer_a_frame() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    let stylesheet_width = Rc::new(Cell::new(20));
    let image_width = Rc::new(Cell::new(5));
    scripts
        .set_layout_source(Box::new(AppliedResourceFixture {
            stylesheet_width: stylesheet_width.clone(),
            image_width: image_width.clone(),
        }))
        .unwrap();

    let run = |scripts: &mut ScriptedDocument<ManualClock>, expected: &str| {
        scripts
            .queue_script(source(&format!(
                r#"
                var body = document.body;
                var style = getComputedStyle(body);
                if ([body.offsetWidth, body.getBoundingClientRect().width,
                    style.width, style.getPropertyValue('--image-width')].join('|')
                    !== '{expected}') throw 'resource inputs are inconsistent';
                "#
            )))
            .unwrap();
        assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    };

    run(&mut scripts, "25|25|20px|5px");
    assert_eq!(scripts.take_layout_flush_count(), 1);
    scripts.offer_rendering_opportunity().unwrap();
    let first_frame = render_turn(&mut scripts);

    // A failed stylesheet fetch has not applied anything, so it must not
    // invalidate the frame or queue an unnecessary rendering opportunity.
    assert_eq!(
        scripts.apply_resource_completion(|| Err::<(), _>("stylesheet fetch rejected")),
        Ok(Err("stylesheet fetch rejected"))
    );
    run(&mut scripts, "25|25|20px|5px");
    assert_eq!(scripts.take_layout_flush_count(), 0);
    assert_eq!(scripts.accept_rendered_frame(first_frame), Ok(true));
    assert_eq!(scripts.run_next_with_render(unexpected_render), Ok(None));

    scripts.offer_rendering_opportunity().unwrap();
    let stale_stylesheet = render_turn(&mut scripts);
    assert_eq!(
        scripts.apply_resource_completion(|| {
            stylesheet_width.set(40);
            Ok::<(), &str>(())
        }),
        Ok(Ok(()))
    );
    assert_eq!(scripts.accept_rendered_frame(stale_stylesheet), Ok(false));
    run(&mut scripts, "45|45|40px|5px");
    assert_eq!(scripts.take_layout_flush_count(), 1);
    // The successful completion queued this frame without an explicit offer.
    let stylesheet_frame = render_turn(&mut scripts);
    assert_eq!(scripts.accept_rendered_frame(stylesheet_frame), Ok(true));

    assert_eq!(
        scripts.apply_resource_completion(|| {
            image_width.set(15);
            Ok::<(), &str>(())
        }),
        Ok(Ok(()))
    );
    run(&mut scripts, "55|55|40px|15px");
    assert_eq!(scripts.take_layout_flush_count(), 1);
    let image_frame = render_turn(&mut scripts);
    assert_eq!(scripts.accept_rendered_frame(image_frame), Ok(true));
}

#[test]
fn rendering_input_revisions_are_scoped_to_their_document() {
    let mut engine = Engine::new(2);
    let mut first = document(&mut engine, TaskLimits::default());
    let mut second = document(&mut engine, TaskLimits::default());
    let first_width = Rc::new(Cell::new(30));
    let second_width = Rc::new(Cell::new(80));
    first
        .set_layout_source(Box::new(LayoutFixture(first_width.clone())))
        .unwrap();
    second
        .set_layout_source(Box::new(LayoutFixture(second_width.clone())))
        .unwrap();

    for (scripts, width) in [(&mut first, 30), (&mut second, 80)] {
        scripts
            .queue_script(source(&format!(
                "if (document.body.offsetWidth !== {width}) throw 'initial geometry';"
            )))
            .unwrap();
        assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
        assert_eq!(scripts.take_layout_flush_count(), 1);
        scripts.offer_rendering_opportunity().unwrap();
    }
    let first_frame = render_turn(&mut first);
    let second_frame = render_turn(&mut second);

    first_width.set(50);
    first.rendering_inputs_changed().unwrap();
    assert_eq!(first.accept_rendered_frame(first_frame), Ok(false));
    assert_eq!(second.accept_rendered_frame(second_frame), Ok(true));

    first
        .queue_script(source(
            "if (document.body.getBoundingClientRect().width !== 50) throw 'stale first geometry';",
        ))
        .unwrap();
    second
        .queue_script(source(
            "if (document.body.getBoundingClientRect().width !== 80) throw 'changed second geometry';",
        ))
        .unwrap();
    assert_eq!(first.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(second.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(first.take_layout_flush_count(), 1);
    assert_eq!(second.take_layout_flush_count(), 0);

    first.offer_rendering_opportunity().unwrap();
    let fresh_first = render_turn(&mut first);
    second_width.set(90);
    second.rendering_inputs_changed().unwrap();
    assert_eq!(first.accept_rendered_frame(fresh_first), Ok(true));
}


#[derive(Debug)]
struct LiveStyleFixture;

impl LayoutSource for LiveStyleFixture {
    fn measure_layout(
        &self,
        document: &Document,
        _: DocumentMode,
    ) -> Result<LayoutGeometry, LayoutReadError> {
        let mut geometry = LayoutGeometry::default();
        for (id, _) in document.walk(document.root()).unwrap() {
            let Ok(node) = document.node(id) else {
                continue;
            };
            let rustic_dom::NodeKind::Element(element) = node.kind() else {
                continue;
            };
            let next = element.attributes().iter().any(|attribute| {
                attribute.name().local() == "data-next"
            });
            geometry.computed_styles.insert(
                id,
                rustic_engine::ComputedStyleSnapshot {
                    longhands: vec![
                        ("display", "block".to_owned()),
                        ("color", if next { "blue" } else { "red" }.to_owned()),
                    ],
                    custom_properties: if next {
                        vec![
                            ("--Case".to_owned(), "preserved".to_owned()),
                            ("--Dynamic".to_owned(), "next".to_owned()),
                        ]
                    } else {
                        vec![("--Case".to_owned(), "preserved".to_owned())]
                    },
                },
            );
        }
        Ok(geometry)
    }
}

#[test]
fn held_computed_declaration_reads_live_snapshots_after_argument_reentry() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts.set_layout_source(Box::new(LiveStyleFixture)).unwrap();
    scripts.queue_script(source(
        r#"
        const body = document.body;
        const style = getComputedStyle(body);
        if (!(style instanceof CSSStyleDeclaration)) throw 'declaration brand';
        if (style.length !== 3 || style.item(0) !== 'display' ||
            style.item(2) !== '--Case' || style.item(3) !== '') throw 'indexes';
        if (style.color !== 'red' || style.display !== 'block' ||
            style['color'] !== style.getPropertyValue('color') ||
            style.backgroundColor !== '' ||
            style.getPropertyValue('COLOR') !== 'red' ||
            style.getPropertyValue('--Case') !== 'preserved' ||
            style.getPropertyValue('--case') !== '' ||
            style.getPropertyValue('unknown-property') !== '' ||
            style.getPropertyPriority('color') !== '') throw 'initial values';
        const property = {
            toString() {
                body.setAttribute('data-next', 'yes');
                return 'color';
            }
        };
        if (style.getPropertyValue(property) !== 'blue' || style.color !== 'blue') {
            throw 'reentrant lookup';
        }
        try { style.color = 'green'; throw 'named setter allowed'; }
        catch (error) {
            if (error.name !== 'NoModificationAllowedError') throw 'wrong setter error';
        }
        if (style.color !== 'blue') throw 'named write changed style';
        Promise.resolve().then(() => {
            body.removeAttribute('data-next');
            if (style.getPropertyValue('color') !== 'red' || style.color !== 'red') {
                throw 'microtask lookup';
            }
        });
        try { getComputedStyle(body, '::before'); throw 'pseudo accepted'; }
        catch (error) {
            if (error.name !== 'TypeError') throw 'wrong pseudo error';
        }
        "#,
    )).unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.take_layout_flush_count(), 3);
}

#[test]
fn computed_style_numeric_indices_track_live_properties_and_reject_writes() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts.set_layout_source(Box::new(LiveStyleFixture)).unwrap();
    scripts.queue_script(source(
        r#"
        const body = document.body;
        const style = getComputedStyle(body);
        if (style.length !== 3 || style[0] !== 'display' ||
            style[1] !== 'color' || style[2] !== '--Case' ||
            style[3] !== undefined) throw 'initial indices';
        if (!('0' in style) || '3' in style ||
            Object.keys(style).join(',') !== '0,1,2') throw 'own keys';
        const desc = Object.getOwnPropertyDescriptor(style, '0');
        if (!desc || desc.value !== 'display' || desc.writable ||
            !desc.enumerable || !desc.configurable) throw 'index descriptor';
        if (Reflect.set(style, '0', 'other') ||
            Reflect.deleteProperty(style, '0') ||
            style[0] !== 'display') throw 'indexed mutation';
        body.setAttribute('data-next', 'yes');
        if (style.length !== 4 || style[3] !== '--Dynamic' ||
            !('3' in style) ||
            Object.keys(style).join(',') !== '0,1,2,3') {
            throw 'index did not grow';
        }
        body.removeAttribute('data-next');
        if (style.length !== 3 || '3' in style || style[3] !== undefined ||
            Object.keys(style).join(',') !== '0,1,2') {
            throw 'index did not shrink';
        }
        "#,
    )).unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.take_layout_flush_count(), 3);
}

#[test]
fn computed_style_and_geometry_share_the_dirty_flush_quota() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts.set_layout_source(Box::new(LiveStyleFixture)).unwrap();
    scripts.queue_script(source(&format!(
        r#"
        const body = document.body;
        const style = getComputedStyle(body);
        for (let i = 0; i < {MAX_LAYOUT_FLUSHES_PER_TURN}; i++) {{
            body.setAttribute('data-next', String(i));
            if (style.getPropertyValue('color') !== 'blue') throw 'style value';
            if (style[0] !== 'display' || style.length !== 4) throw 'indexed style';
            if (body.offsetWidth !== 0) throw 'geometry value';
        }}
        body.removeAttribute('data-next');
        try {{ style[0]; throw 'indexed quota missing'; }}
        catch (error) {{
            if (error.name !== 'QuotaExceededError') throw 'wrong indexed quota error';
        }}
        try {{ style.getPropertyValue('color'); throw 'quota missing'; }}
        catch (error) {{
            if (error.name !== 'QuotaExceededError') throw 'wrong quota error';
        }}
        "#
    ))).unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(
        scripts.take_layout_flush_count(),
        MAX_LAYOUT_FLUSHES_PER_TURN as u64
    );
    scripts.queue_script(source(
        "if (getComputedStyle(document.body).getPropertyValue('color') !== 'red') throw 'new turn';",
    )).unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.take_layout_flush_count(), 1);
}



#[test]
fn computed_style_mutation_methods_are_read_only() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts.set_layout_source(Box::new(LiveStyleFixture)).unwrap();
    scripts
        .queue_script(source(
            r#"
            const body = document.body;
            const computed = getComputedStyle(body);
            if (computed.cssText !== '') throw 'computed cssText';
            for (const edit of [
                () => computed.setProperty('color', 'green'),
                () => computed.removeProperty('color'),
                () => { computed.cssText = 'color: green' }
            ]) {
                try {
                    edit();
                    throw 'computed declaration was editable';
                } catch (error) {
                    if (error.name !== 'NoModificationAllowedError') throw 'wrong error';
                }
            }
            if (computed.getPropertyValue('color') !== 'red' ||
                body.getAttribute('style') !== null) throw 'mutated computed style';
            "#,
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.take_layout_flush_count(), 1);
}

#[test]
fn connected_computed_styles_require_a_renderer_but_detached_reads_are_empty() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts
        .queue_script(source(
            r#"
            const connected = getComputedStyle(document.body);
            try {
                connected.getPropertyValue('display');
                throw 'missing renderer silently returned a value';
            } catch (error) {
                if (error.name !== 'InvalidStateError') throw 'wrong error';
            }
            try {
                connected.length;
                throw 'length bypassed the renderer';
            } catch (error) {
                if (error.name !== 'InvalidStateError') throw 'wrong length error';
            }
            const detached = getComputedStyle(document.createElement('p'));
            if (detached.getPropertyValue('display') !== '' ||
                detached.length !== 0 ||
                detached.item(0) !== '') throw 'detached style not empty';
            if (document.body.offsetWidth !== 0) throw 'geometry fallback changed';
            "#,
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.take_layout_flush_count(), 0);
}

#[test]
fn a_failed_checkpoint_discards_the_rendering_offer() {
    let mut engine = Engine::new(1);
    let mut scripts = document(
        &mut engine,
        TaskLimits {
            jobs_per_checkpoint: 0,
            ..TaskLimits::default()
        },
    );
    scripts
        .queue_script(source("Promise.resolve().then(() => 0)"))
        .unwrap();
    scripts.offer_rendering_opportunity().unwrap();

    assert_eq!(
        scripts.run_next_with_render(unexpected_render),
        Err(ScriptStop::JobLimit)
    );
    assert_eq!(
        scripts.run_next_with_render(unexpected_render),
        Err(ScriptStop::JobLimit)
    );
}

#[test]
fn shutdown_during_rendering_discards_the_output_and_ticket() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts.offer_rendering_opportunity().unwrap();
    let mut issued = None;
    let result = scripts.run_next_with_render(|_, frame| {
        issued = Some(frame);
        engine.shutdown();
        "rendered"
    });

    assert_eq!(result, Err(ScriptStop::DocumentCancelled));
    assert_eq!(
        scripts.accept_rendered_frame(issued.unwrap()),
        Err(ScriptStop::DocumentCancelled)
    );
    assert_eq!(
        scripts.offer_rendering_opportunity(),
        Err(ScriptStop::DocumentCancelled)
    );
}

#[test]
fn animation_callbacks_run_before_render_with_live_cancellation_and_per_callback_jobs() {
    let mut engine = Engine::new(1);
    let mut scripts = document(&mut engine, TaskLimits::default());
    scripts
        .queue_script(source(
            r#"
        const body = document.querySelector("body");
        let trace = "";
        let cancelled;
        requestAnimationFrame(function (timestamp) {
            "use strict";
            if (timestamp !== 7.5 || this !== undefined) throw "frame arguments";
            trace += "a";
            Promise.resolve().then(() => {
                trace += "b";
                cancelAnimationFrame(cancelled);
                requestAnimationFrame(next => {
                    if (next !== 12) throw "next timestamp";
                    body.firstChild.data = trace + "e";
                });
            });
        });
        cancelled = requestAnimationFrame(() => trace += "x");
        requestAnimationFrame(timestamp => {
            if (timestamp !== 7.5 || trace !== "ab") throw "callback order";
            trace += "c";
            throw "uncaught callback";
        });
        requestAnimationFrame(() => {
            trace += "d";
            body.appendChild(document.createTextNode(trace));
        });
    "#,
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    assert_eq!(scripts.run_next(), Ok(None));
    scripts
        .clock()
        .advance(Duration::from_micros(7500))
        .unwrap();
    scripts.offer_rendering_opportunity().unwrap();
    for (expected, clock_step) in [
        ("abcd", Duration::ZERO),
        ("abcde", Duration::from_micros(4500)),
    ] {
        if clock_step != Duration::ZERO {
            scripts.clock().advance(clock_step).unwrap();
            scripts.offer_rendering_opportunity().unwrap();
        }
        let Some(DocumentTurn::Rendered { frame, output }) = scripts
            .run_next_with_render(|document, _| {
                let tree = document.parsed().document();
                tree.dump_tree(tree.root(), 4096).unwrap()
            })
            .unwrap()
        else {
            panic!("expected rendering");
        };
        assert!(output.contains(&format!("\"{expected}\"")), "{output}");
        assert_eq!(scripts.accept_rendered_frame(frame), Ok(true));
    }
    assert_eq!(scripts.take_animation_error_count(), 1);
    assert_eq!(scripts.take_animation_error_count(), 0);
}

#[test]
fn a_failed_animation_checkpoint_prevents_rendering_and_discards_callbacks() {
    let mut engine = Engine::new(1);
    let mut scripts = document(
        &mut engine,
        TaskLimits {
            jobs_per_checkpoint: 0,
            ..TaskLimits::default()
        },
    );
    scripts
        .queue_script(source(
            r#"
        requestAnimationFrame(() => { Promise.resolve().then(() => 0); });
        requestAnimationFrame(() => { throw "after the failed callback"; });
    "#,
        ))
        .unwrap();
    assert_eq!(scripts.run_next(), Ok(Some(ScriptTurn::Completed)));
    scripts.offer_rendering_opportunity().unwrap();
    assert_eq!(
        scripts.run_next_with_render(unexpected_render),
        Err(ScriptStop::JobLimit)
    );
    assert_eq!(scripts.take_animation_error_count(), 0);
    assert_eq!(
        scripts.offer_rendering_opportunity(),
        Err(ScriptStop::JobLimit)
    );
}

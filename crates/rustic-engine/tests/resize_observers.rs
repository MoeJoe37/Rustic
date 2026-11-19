//! ResizeObserver observations broadcast in rendering turns.

use rustic_base::ManualClock;
use rustic_dom::{Document, NodeKind};
use rustic_engine::{
    BoxSizes, DocumentTurn, Engine, LayoutGeometry, LayoutReadError, LayoutSource, ScriptTurn,
    ScriptedDocument, TaskLimits,
};
use rustic_html::{DocumentMode, ParserLimits};
use rustic_js::Source;

/// Elements with `data-w` get a content box that wide and 10 high, with
/// 2px padding and a 1px border.
#[derive(Debug)]
struct Sized;

impl LayoutSource for Sized {
    fn measure_layout(
        &self,
        document: &Document,
        _: DocumentMode,
    ) -> Result<LayoutGeometry, LayoutReadError> {
        let mut geometry = LayoutGeometry::default();
        for (node, _) in document.walk(document.root()).unwrap() {
            let NodeKind::Element(element) = document.node(node).unwrap().kind() else {
                continue;
            };
            let Some(width) = element
                .attributes()
                .iter()
                .find(|attribute| attribute.name().local() == "data-w")
                .and_then(|attribute| {
                    String::from_utf16(attribute.value().as_units())
                        .ok()?
                        .parse::<f64>()
                        .ok()
                })
            else {
                continue;
            };
            geometry.box_sizes.insert(
                node,
                BoxSizes {
                    content: [width, 10.0],
                    border: [width + 6.0, 16.0],
                    padding: [2.0, 2.0],
                },
            );
        }
        Ok(geometry)
    }
}

fn page(engine: &mut Engine) -> ScriptedDocument<ManualClock> {
    let mut load = engine.begin_document(ParserLimits::default()).unwrap();
    load.feed(
        br#"<!doctype html><div id="outer" data-w="40"><div id="inner" data-w="20"></div></div>"#,
    )
    .unwrap();
    let mut page = load
        .finish()
        .unwrap()
        .into_scripted_with_clock(TaskLimits::default(), ManualClock::default())
        .unwrap();
    page.set_layout_source(Box::new(Sized)).unwrap();
    page
}

fn run(page: &mut ScriptedDocument<ManualClock>, text: &str) {
    page.queue_script(Source::from_str_limited(text, 8192).unwrap())
        .unwrap();
    let turn = page.run_next();
    let console = page.take_console_messages();
    assert_eq!(turn, Ok(Some(ScriptTurn::Completed)), "{text}\n{console:?}");
}

fn render(page: &mut ScriptedDocument<ManualClock>) {
    page.offer_rendering_opportunity().unwrap();
    assert!(matches!(
        page.run_next_with_render(|_, _| ()),
        Ok(Some(DocumentTurn::Rendered { .. }))
    ));
}

#[test]
fn observations_report_once_per_size_change_in_rendering_turns() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine);
    run(
        &mut page,
        r#"
        var log = [];
        const $ = (id) => document.getElementById(id);
        var observer = new ResizeObserver(function (entries, self) {
            if (this !== observer || self !== observer) throw 'receiver';
            for (const entry of entries) {
                const r = entry.contentRect;
                log.push([entry.target.id, r.x, r.y, r.width, r.height,
                    entry.contentBoxSize[0].inlineSize, entry.borderBoxSize[0].inlineSize,
                    entry.borderBoxSize[0].blockSize, Object.isFrozen(entry.borderBoxSize)].join());
            }
        });
        observer.observe($('inner'));
        observer.observe($('outer'), { box: 'border-box' });
        requestAnimationFrame(() => log.push('frame'));
        if (log.length) throw 'observe is asynchronous';
        "#,
    );
    render(&mut page);
    run(
        &mut page,
        r#"
        const expected = ['frame', 'inner,2,2,20,10,20,26,16,true', 'outer,2,2,40,10,40,46,16,true'];
        if (log.join('|') !== expected.join('|')) throw log.join('|');
        log.length = 0;
        "#,
    );
    // No change: nothing is delivered. A change to one target reports it.
    render(&mut page);
    run(
        &mut page,
        r#"
        if (log.length) throw 'unchanged size reported';
        document.getElementById('inner').setAttribute('data-w', '25');
        "#,
    );
    render(&mut page);
    run(
        &mut page,
        r#"
        if (log.join('|') !== 'inner,2,2,25,10,25,31,16,true') throw log.join('|');
        log.length = 0;
        observer.unobserve(document.getElementById('inner'));
        document.getElementById('inner').setAttribute('data-w', '30');
        "#,
    );
    render(&mut page);
    run(
        &mut page,
        r#"
        if (log.length) throw 'unobserved target reported';
        observer.disconnect();
        document.getElementById('outer').setAttribute('data-w', '10');
        "#,
    );
    render(&mut page);
    run(
        &mut page,
        "if (log.length) throw 'disconnected observer reported';",
    );
}

#[test]
fn shallower_changes_wait_for_the_next_frame_with_a_loop_error() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine);
    run(
        &mut page,
        r#"
        var log = [];
        var errors = [];
        addEventListener('error', (event) => errors.push(event.message));
        const $ = (id) => document.getElementById(id);
        let resized = false;
        new ResizeObserver((entries) => {
            for (const entry of entries) log.push(entry.target.id + entry.contentRect.width);
        }).observe($('outer'));
        new ResizeObserver((entries) => {
            for (const entry of entries) log.push(entry.target.id + entry.contentRect.width);
            // Growing a shallower element after its depth was broadcast
            // waits for the next frame and reports a loop error.
            if (!resized) {
                resized = true;
                $('outer').setAttribute('data-w', '41');
            }
        }).observe($('inner'));
        try { new ResizeObserver(() => {}).observe(document.createTextNode('x')); throw 'text'; }
        catch (error) { if (!(error instanceof TypeError)) throw error; }
        try { new ResizeObserver(() => {}).observe($('outer'), { box: 'margin-box' }); throw 'box'; }
        catch (error) { if (!(error instanceof TypeError)) throw error; }
        try { ResizeObserver(() => {}); throw 'new'; }
        catch (error) { if (!(error instanceof TypeError)) throw error; }
        "#,
    );
    render(&mut page);
    run(
        &mut page,
        r#"
        if (log.join() !== 'outer40,inner20') throw log.join();
        if (errors.join() !== 'Uncaught Error: ResizeObserver loop completed with undelivered notifications.')
            throw errors.join();
        log.length = 0;
        "#,
    );
    render(&mut page);
    run(&mut page, "if (log.join() !== 'outer41') throw log.join();");
}

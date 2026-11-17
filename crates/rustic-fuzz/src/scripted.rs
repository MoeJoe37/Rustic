//! Deterministic oracles for the local document pump and its CPU rendering path.

use std::fmt::Write;
use std::time::Duration;

use rustic_base::ManualClock;
use rustic_css::CssParser;
use rustic_dom::NodeKind;
use rustic_engine::{
    DocumentTurn, Engine, EngineDocument, InputEvent, Modifiers, ScriptStop, ScriptTurn,
    ScriptedDocument, TaskLimits,
};
use rustic_html::ParserLimits;
use rustic_js::Source;
use rustic_layout::{BlockLayout, BoxTree, CssSize, DeviceScale, LayoutLimits};
use rustic_paint::{DisplayList, PaintLimits};
use rustic_raster::{RasterLimits, Surface, rasterize};
use rustic_style::{ComputeContext, MatchContext, document_cascade, style_tree};

use crate::Target;

pub(crate) fn seeds() -> Vec<Vec<u8>> {
    (0..=127u8)
        .map(|value| vec![value, value.rotate_left(3), !value])
        .collect()
}

fn parse(engine: &mut Engine, html: &str) -> Result<EngineDocument, String> {
    let mut load = engine
        .begin_document(ParserLimits::default())
        .map_err(|e| e.to_string())?;
    load.feed(html.as_bytes()).map_err(|e| e.to_string())?;
    load.finish().map_err(|e| e.to_string())
}

fn scripted(document: EngineDocument) -> Result<ScriptedDocument<ManualClock>, String> {
    document
        .into_scripted_with_clock(TaskLimits::default(), ManualClock::default())
        .map_err(|e| e.to_string())
}

fn run_script(document: &mut ScriptedDocument<ManualClock>, code: &str) -> Result<(), String> {
    let source = Source::from_str_limited(code, 8192).map_err(|e| e.to_string())?;
    document.queue_script(source).map_err(|e| e.to_string())?;
    run_turn(document)
}

fn run_turn(document: &mut ScriptedDocument<ManualClock>) -> Result<(), String> {
    match document.run_next() {
        Ok(Some(ScriptTurn::Completed)) => Ok(()),
        other => Err(format!("expected a completed script turn, got {other:?}")),
    }
}

pub(crate) struct EventPumpTarget;

impl Target for EventPumpTarget {
    fn name(&self) -> &'static str {
        "script-event-pump"
    }

    fn max_len(&self) -> usize {
        12
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[b"\x00\x00", b"\xff\x01\x01"]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        let mut engine = Engine::new(1);
        let mut page = scripted(parse(
            &mut engine,
            "<!doctype html><main><button type=button></button></main>",
        )?)?;
        let button = {
            let parsed = page.document();
            let dom = parsed.parsed().document();
            dom.walk(dom.root()).map_err(|e| e.to_string())?
                .map(|(id, _)| id)
                .find(|&id| matches!(dom.node(id).map(|node| node.kind()), Ok(NodeKind::Element(e)) if e.name().local() == "button"))
                .ok_or("test button")?
        };
        let mut source = String::from(
            "var trace = '';
             document.querySelector('button').addEventListener('click', event => {
                 if (event.isTrusted) trace += 'b';
                 Promise.resolve().then(() => trace += 'm');
             });
             document.querySelector('main').addEventListener('click', () => trace += 'p');",
        );
        let mut timers = Vec::new();
        for (index, &value) in input.iter().take(self.max_len()).enumerate() {
            let delay = value % 4 + 1;
            write!(
                &mut source,
                "var t = setTimeout(() => trace += '[{index}]', {delay});"
            )
            .map_err(|e| e.to_string())?;
            if value & 8 == 0 {
                timers.push((delay, index));
            } else {
                source.push_str("clearTimeout(t);");
            }
        }
        run_script(&mut page, &source)?;
        page.queue_input(
            button,
            InputEvent::Click {
                client_x: 0,
                client_y: 0,
                modifiers: Modifiers::default(),
            },
        )
        .map_err(|e| e.to_string())?;
        run_turn(&mut page)?;
        let mut expected = String::from("bpm");
        let mut elapsed = 0;
        timers.sort_unstable();
        for (delay, index) in timers {
            page.clock()
                .advance(Duration::from_millis(u64::from(delay - elapsed)))
                .map_err(|e| e.to_string())?;
            elapsed = delay;
            run_turn(&mut page)?;
            write!(&mut expected, "[{index}]").map_err(|e| e.to_string())?;
        }
        run_script(
            &mut page,
            &format!("if (trace !== '{expected}') throw trace;"),
        )?;
        if !matches!(page.run_next(), Ok(None)) || page.take_listener_error_count() != 0 {
            return Err("event pump retained a task or listener error".into());
        }
        engine.shutdown();
        if page.run_next() != Err(ScriptStop::DocumentCancelled) {
            return Err("cancelled event pump still ran".into());
        }
        Ok(())
    }
}

pub(crate) struct MutationRenderTarget;

impl Target for MutationRenderTarget {
    fn name(&self) -> &'static str {
        "script-mutation-render"
    }

    fn max_len(&self) -> usize {
        3
    }

    fn dictionary(&self) -> &'static [&'static [u8]] {
        &[b"\x00", b"\xff", b"\x04\x02\x01"]
    }

    fn check(&self, input: &[u8]) -> Result<(), String> {
        const SHELL: &str = "<!doctype html><style>body{margin:0}
          #box{display:block;width:2px;height:10px;background:black}
          .mark{display:block;width:2px;height:2px;background:white}</style>";
        let mut engine = Engine::new(2);
        let mut page = scripted(parse(
            &mut engine,
            &format!("{SHELL}<div id=box><div class=mark></div></div>"),
        )?)?;
        page.offer_rendering_opportunity()
            .map_err(|e| e.to_string())?;
        let Some(DocumentTurn::Rendered { frame: old, .. }) = page
            .run_next_with_render(|_, _| ())
            .map_err(|e| e.to_string())?
        else {
            return Err("initial frame did not render".into());
        };

        let width = input.first().map_or(3, |byte| 3 + byte % 12);
        let palette = ["red", "green", "blue", "yellow"];
        let color = palette[usize::from(input.get(1).copied().unwrap_or(0) % 4)];
        let keep_marker = input.get(2).is_some_and(|byte| byte & 1 != 0);
        let style = format!("width:{width}px;background:{color}");
        let edit = if keep_marker {
            ""
        } else {
            "box.removeChild(box.firstChild);"
        };
        run_script(
            &mut page,
            &format!(
                "var box = document.getElementById('box');
             box.setAttribute('style', '{style}');{edit}"
            ),
        )?;
        if page.accept_rendered_frame(old).map_err(|e| e.to_string())? {
            return Err("frame before script mutation remained presentable".into());
        }
        page.offer_rendering_opportunity()
            .map_err(|e| e.to_string())?;
        let Some(DocumentTurn::Rendered { frame, output }) = page
            .run_next_with_render(|doc, _| pixels(doc))
            .map_err(|e| e.to_string())?
        else {
            return Err("mutated frame did not render".into());
        };
        if !page
            .accept_rendered_frame(frame)
            .map_err(|e| e.to_string())?
        {
            return Err("frame for current document was rejected".into());
        }

        let marker = if keep_marker {
            "<div class=mark></div>"
        } else {
            ""
        };
        let fresh = parse(
            &mut engine,
            &format!("{SHELL}<div id=box style='{style}'>{marker}</div>"),
        )?;
        if output?.premultiplied() != pixels(&fresh)?.premultiplied() {
            return Err(
                "scripted frame pixels differ from a fresh rendering of the changed markup".into(),
            );
        }
        Ok(())
    }
}

fn pixels(owner: &EngineDocument) -> Result<Surface, String> {
    let document = owner.parsed().document();
    let parser = CssParser::default();
    let (cascade, _) = document_cascade(document, &parser).map_err(|e| e.to_string())?;
    let context = ComputeContext {
        viewport_width: 20.0,
        viewport_height: 12.0,
        ..ComputeContext::default()
    };
    let styles = style_tree(
        document,
        document.root(),
        &cascade,
        &parser,
        &MatchContext::html(),
        &context,
    )
    .map_err(|e| e.to_string())?;
    let cancel = owner.cancellation();
    let limits = LayoutLimits::default();
    let tree = BoxTree::build(document, document.root(), &styles, limits, cancel)
        .map_err(|e| e.to_string())?;
    let viewport = CssSize::new(20.0, 12.0).map_err(|e| e.to_string())?;
    let layout =
        BlockLayout::compute(&tree, viewport, limits, cancel).map_err(|e| e.to_string())?;
    let list = DisplayList::build(
        document,
        &tree,
        &layout,
        viewport,
        PaintLimits::default(),
        cancel,
    )
    .map_err(|e| e.to_string())?;
    rasterize(
        &list,
        DeviceScale::new(1.0).map_err(|e| e.to_string())?,
        RasterLimits::default(),
        cancel,
    )
    .map_err(|e| e.to_string())
}

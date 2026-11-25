//! Web Animations in rendering turns: `element.animate()`, playback
//! promises and events, and CSS transition events from styled frames.

use std::time::Duration;

use rustic_base::ManualClock;
use rustic_css::CssParser;
use rustic_engine::{DocumentTurn, Engine, ScriptTurn, ScriptedDocument, TaskLimits};
use rustic_html::ParserLimits;
use rustic_js::Source;
use rustic_style::{ComputeContext, MatchContext, document_cascade, style_tree_animated};

fn page(engine: &mut Engine, html: &str) -> ScriptedDocument<ManualClock> {
    let mut load = engine.begin_document(ParserLimits::default()).unwrap();
    load.feed(html.as_bytes()).unwrap();
    load.finish()
        .unwrap()
        .into_scripted_with_clock(TaskLimits::default(), ManualClock::default())
        .unwrap()
}

fn run(page: &mut ScriptedDocument<ManualClock>, text: &str) {
    page.queue_script(Source::from_str_limited(text, 16384).unwrap())
        .unwrap();
    let turn = page.run_next();
    let console = page.take_console_messages();
    assert_eq!(turn, Ok(Some(ScriptTurn::Completed)), "{text}\n{console:?}");
}

/// Advance the clock and run one rendering turn that styles the page
/// through its animation controller, as the renderer does.
fn frame(page: &mut ScriptedDocument<ManualClock>, step_ms: u64) {
    page.clock()
        .advance(Duration::from_millis(step_ms))
        .unwrap();
    page.offer_rendering_opportunity().unwrap();
    let animations = page.animations();
    let turn = page.run_next_with_render(|document, _| {
        let parser = CssParser::default();
        let document = document.parsed().document();
        let (cascade, _) = document_cascade(document, &parser).unwrap();
        style_tree_animated(
            document,
            document.root(),
            &cascade,
            &parser,
            &MatchContext::html(),
            &ComputeContext::default(),
            &Default::default(),
            &mut animations.borrow_mut(),
        )
        .unwrap();
    });
    assert!(
        matches!(turn, Ok(Some(DocumentTurn::Rendered { .. }))),
        "{turn:?}"
    );
}

#[test]
fn element_animate_plays_settles_promises_and_fires_finish() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine, "<!doctype html><div id=t></div>");
    run(
        &mut page,
        "var log = [];
         var el = document.getElementById('t');
         var a = el.animate([{ opacity: 0 }, { opacity: 1, offset: 1 }],
                            { duration: 100, fill: 'forwards', id: 'fade' });
         if (!(a instanceof Animation) || !(a.effect instanceof KeyframeEffect)) throw 'types';
         if (a.id !== 'fade' || a.playState !== 'running' || !a.pending) throw 'state ' + a.playState;
         if (a.timeline !== document.timeline) throw 'timeline';
         var frames = a.effect.getKeyframes();
         if (frames.length !== 2 || frames[0].opacity !== '0' || frames[1].computedOffset !== 1)
             throw JSON.stringify(frames);
         var timing = a.effect.getTiming();
         if (timing.duration !== 100 || timing.fill !== 'forwards' || timing.easing !== 'linear')
             throw JSON.stringify(timing);
         if (el.getAnimations().length !== 1 || el.getAnimations()[0] !== a) throw 'getAnimations';
         if (document.getAnimations().length !== 1) throw 'document.getAnimations';
         a.ready.then(x => log.push('ready:' + (x === a)));
         a.finished.then(() => log.push('finished'));
         a.onfinish = e => log.push('finish:' + e.currentTime + ':' + (e instanceof AnimationPlaybackEvent));",
    );
    frame(&mut page, 10);
    run(
        &mut page,
        "if (log.join() !== 'ready:true') throw log.join();
         if (a.pending || a.startTime !== 10) throw 'start ' + a.startTime;",
    );
    frame(&mut page, 50);
    run(
        &mut page,
        "if (a.currentTime !== 50) throw 'current ' + a.currentTime;
         var t = a.effect.getComputedTiming();
         if (t.progress !== 0.5 || t.localTime !== 50) throw JSON.stringify(t);",
    );
    frame(&mut page, 100);
    run(
        &mut page,
        "if (a.playState !== 'finished') throw a.playState;
         if (log.indexOf('finished') < 0 || log.indexOf('finish:100:true') < 0) throw log.join();",
    );
}

#[test]
fn animation_methods_and_cancel() {
    let mut engine = Engine::new(1);
    let mut page = page(&mut engine, "<!doctype html><div id=t></div>");
    run(
        &mut page,
        "var el = document.getElementById('t');
         var effect = new KeyframeEffect(el, { width: ['0px', '100px'] }, 200);
         var a = new Animation(effect, document.timeline);
         if (a.playState !== 'idle' || a.effect !== effect) throw 'idle';
         a.pause();
         if (a.playState !== 'paused' || a.currentTime !== 0) throw 'paused ' + a.currentTime;
         a.currentTime = 50;
         if (a.currentTime !== 50) throw 'seek';
         a.playbackRate = 2;
         if (a.currentTime !== 50 || a.playbackRate !== 2) throw 'rate';
         var rejected = '';
         a.finished.catch(e => rejected = e.name);
         a.cancel();
         if (a.playState !== 'idle' || a.currentTime !== null) throw 'cancel';
         try { new Animation(effect).currentTime = null; } catch (e) { throw 'null seek of idle'; }
         try { el.animate([{ offset: 2 }]); throw 'offset'; }
         catch (e) { if (!(e instanceof TypeError)) throw e; }
         try { el.animate(null, { duration: -1 }); throw 'duration'; }
         catch (e) { if (!(e instanceof TypeError)) throw e; }
         try { el.animate(null, { easing: 'bogus' }); throw 'easing'; }
         catch (e) { if (!(e instanceof TypeError)) throw e; }",
    );
    run(
        &mut page,
        "if (rejected !== 'AbortError') throw 'rejected ' + rejected;",
    );
}

#[test]
fn css_transitions_fire_events_from_styled_frames() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        "<!doctype html><style>#t { opacity: 0; transition: opacity 100ms linear }\
         #t.on { opacity: 1 }</style><div id=t></div>",
    );
    run(
        &mut page,
        "var log = [];
         var el = document.getElementById('t');
         for (var type of ['transitionrun', 'transitionstart', 'transitionend'])
             el.addEventListener(type, e => log.push(e.type + ':' + e.propertyName + ':' + e.elapsedTime
                 + ':' + (e instanceof TransitionEvent) + ':' + e.bubbles));
         document.addEventListener('transitionend', () => log.push('bubbled'));",
    );
    frame(&mut page, 10);
    run(&mut page, "el.className = 'on';");
    frame(&mut page, 10);
    run(
        &mut page,
        "var running = el.getAnimations();
         if (running.length !== 1 || !(running[0] instanceof CSSTransition)) throw 'transition object';
         if (running[0].transitionProperty !== 'opacity') throw running[0].transitionProperty;",
    );
    frame(&mut page, 50);
    frame(&mut page, 100);
    run(
        &mut page,
        "var expected = 'transitionrun:opacity:0:true:true,transitionstart:opacity:0:true:true,\
transitionend:opacity:0.1:true:true,bubbled';
         if (log.join() !== expected) throw log.join();
         if (new TransitionEvent('x', { propertyName: 'a', elapsedTime: 2 }).elapsedTime !== 2) throw 'ctor';
         if (new AnimationEvent('x', { animationName: 'n' }).animationName !== 'n') throw 'ctor2';",
    );
}

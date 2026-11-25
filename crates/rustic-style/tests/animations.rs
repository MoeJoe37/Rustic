//! CSS transitions, CSS animations and Web Animations playback through
//! [`style_tree_animated`] and the [`AnimationController`].

use std::collections::HashMap;

use rustic_css::{CssParser, PropertyId};
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::{HtmlDocumentParser, ParsedDocument};
use rustic_style::{
    AnimationController, AnimationEventKind, Cascade, ComputeContext, EffectKeyframe, Fill,
    KeyframeEffect, KeyframeValues, MatchContext, Origin, PlayStateValue, StyleSheet, TimelineRef,
    Timing, style_tree_animated,
};

fn document(html: &str) -> ParsedDocument {
    let mut parser = HtmlDocumentParser::default();
    parser.feed(html.as_bytes()).unwrap();
    parser.finish().unwrap()
}

fn find(document: &Document, wanted: &str) -> NodeId {
    let mut stack = vec![document.root()];
    while let Some(id) = stack.pop() {
        if let Ok(node) = document.node(id) {
            if let NodeKind::Element(element) = node.kind() {
                if element.attributes().iter().any(|a| {
                    a.name().local() == "id"
                        && String::from_utf16_lossy(a.value().as_units()) == wanted
                }) {
                    return id;
                }
            }
            stack.extend(document.children(id).unwrap());
        }
    }
    panic!("no #{wanted}");
}

/// Style `page` with `author` at time `now` and return `#t`'s value of
/// `property`.
fn frame(
    page: &ParsedDocument,
    author: &str,
    controller: &mut AnimationController,
    now: f64,
    property: PropertyId,
) -> String {
    let parser = CssParser::default();
    let mut cascade = Cascade::new();
    let (sheet, diagnostics) = StyleSheet::parse(&parser, author, Origin::Author).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    cascade.push(sheet);
    controller.tick(now);
    let doc = page.document();
    let target = find(doc, "t");
    let styles = style_tree_animated(
        doc,
        doc.root(),
        &cascade,
        &parser,
        &MatchContext::html(),
        &ComputeContext::default(),
        &HashMap::new(),
        controller,
    )
    .unwrap();
    let style = styles.iter().find(|s| s.node == target).unwrap();
    style.computed.get(property).to_string()
}

fn kinds(controller: &mut AnimationController) -> Vec<AnimationEventKind> {
    controller
        .take_events()
        .into_iter()
        .map(|e| e.kind)
        .collect()
}

const PAGE: &str = "<!doctype html><div id=t></div>";

#[test]
fn a_style_change_starts_a_transition_that_interpolates_and_ends() {
    let page = document(PAGE);
    let mut c = AnimationController::new();
    let from = "#t { opacity: 0; transition: opacity 1s linear }";
    let to = "#t { opacity: 1; transition: opacity 1s linear }";
    assert_eq!(frame(&page, from, &mut c, 0.0, PropertyId::Opacity), "0");
    // The change starts a transition from 0 at the current time.
    assert_eq!(frame(&page, to, &mut c, 100.0, PropertyId::Opacity), "0");
    assert!(c.wants_frames());
    assert_eq!(frame(&page, to, &mut c, 600.0, PropertyId::Opacity), "0.5");
    assert_eq!(frame(&page, to, &mut c, 1200.0, PropertyId::Opacity), "1");
    let events = kinds(&mut c);
    assert!(
        events.contains(&AnimationEventKind::TransitionRun),
        "{events:?}"
    );
    assert!(
        events.contains(&AnimationEventKind::TransitionStart),
        "{events:?}"
    );
    assert!(
        events.contains(&AnimationEventKind::TransitionEnd),
        "{events:?}"
    );
    assert!(!c.wants_frames());
}

#[test]
fn transitions_wait_for_their_delay_and_reverse_shortened() {
    let page = document(PAGE);
    let mut c = AnimationController::new();
    let a = "#t { width: 0px; transition: width 1s linear }";
    let b = "#t { width: 100px; transition: width 1s linear }";
    frame(&page, a, &mut c, 0.0, PropertyId::Width);
    frame(&page, b, &mut c, 0.0, PropertyId::Width);
    assert_eq!(frame(&page, b, &mut c, 250.0, PropertyId::Width), "25px");
    // Going back halfway through takes a quarter of the duration.
    assert_eq!(frame(&page, a, &mut c, 250.0, PropertyId::Width), "25px");
    assert_eq!(frame(&page, a, &mut c, 375.0, PropertyId::Width), "12.5px");
    assert_eq!(frame(&page, a, &mut c, 500.0, PropertyId::Width), "0px");
    let delayed = "#t { width: 50px; transition: width 1s linear 1s }";
    frame(&page, delayed, &mut c, 1000.0, PropertyId::Width);
    assert_eq!(
        frame(&page, delayed, &mut c, 1500.0, PropertyId::Width),
        "0px"
    );
    assert_eq!(
        frame(&page, delayed, &mut c, 2500.0, PropertyId::Width),
        "25px"
    );
}

#[test]
fn no_transition_without_a_duration_or_for_discrete_properties() {
    let page = document(PAGE);
    let mut c = AnimationController::new();
    frame(&page, "#t { opacity: 0 }", &mut c, 0.0, PropertyId::Opacity);
    assert_eq!(
        frame(
            &page,
            "#t { opacity: 1 }",
            &mut c,
            10.0,
            PropertyId::Opacity
        ),
        "1"
    );
    let d = "transition: float 1s";
    frame(
        &page,
        &format!("#t {{ float: left; {d} }}"),
        &mut c,
        20.0,
        PropertyId::Float,
    );
    assert_eq!(
        frame(
            &page,
            &format!("#t {{ float: right; {d} }}"),
            &mut c,
            30.0,
            PropertyId::Float
        ),
        "right"
    );
    assert!(kinds(&mut c).is_empty());
}

#[test]
fn a_css_animation_runs_iterates_and_ends() {
    let page = document(PAGE);
    let mut c = AnimationController::new();
    let css = "@keyframes grow { from { width: 0px } to { width: 100px } }\
               #t { width: 7px; animation: grow 1s linear 2 }";
    assert_eq!(frame(&page, css, &mut c, 0.0, PropertyId::Width), "0px");
    assert_eq!(frame(&page, css, &mut c, 500.0, PropertyId::Width), "50px");
    assert_eq!(frame(&page, css, &mut c, 1250.0, PropertyId::Width), "25px");
    // Without a fill the base value shows once it ends.
    assert_eq!(frame(&page, css, &mut c, 2500.0, PropertyId::Width), "7px");
    let css_events: Vec<_> = kinds(&mut c)
        .into_iter()
        .filter(|k| k.event_type().starts_with("animation"))
        .collect();
    assert_eq!(
        css_events,
        [
            AnimationEventKind::AnimationStart,
            AnimationEventKind::AnimationIteration,
            AnimationEventKind::AnimationEnd,
        ]
    );
}

#[test]
fn animation_fill_direction_and_missing_keyframes() {
    let page = document(PAGE);
    let mut c = AnimationController::new();
    let css = "@keyframes k { to { width: 100px } }\
               #t { width: 20px; animation: k 1s linear alternate-reverse forwards }";
    // A missing `from` keyframe uses the base value; alternate-reverse
    // starts at the end.
    assert_eq!(frame(&page, css, &mut c, 0.0, PropertyId::Width), "100px");
    assert_eq!(frame(&page, css, &mut c, 500.0, PropertyId::Width), "60px");
    // Filling forwards keeps the final value (the start, when reversed).
    assert_eq!(frame(&page, css, &mut c, 5000.0, PropertyId::Width), "20px");
    let unknown = "#t { width: 3px; animation: nothing 1s }";
    let mut c = AnimationController::new();
    assert_eq!(frame(&page, unknown, &mut c, 0.0, PropertyId::Width), "3px");
}

#[test]
fn display_none_cancels_animations_and_paused_ones_hold() {
    let page = document(PAGE);
    let mut c = AnimationController::new();
    let run = "@keyframes k { from { width: 0px } to { width: 100px } }\
               #t { animation: k 1s linear }";
    frame(&page, run, &mut c, 0.0, PropertyId::Width);
    assert_eq!(frame(&page, run, &mut c, 300.0, PropertyId::Width), "30px");
    let paused = "@keyframes k { from { width: 0px } to { width: 100px } }\
                  #t { animation: k 1s linear paused }";
    // Pausing takes effect at the style change and then holds.
    assert_eq!(
        frame(&page, paused, &mut c, 800.0, PropertyId::Width),
        "80px"
    );
    assert_eq!(
        frame(&page, paused, &mut c, 850.0, PropertyId::Width),
        "80px"
    );
    let hidden = "@keyframes k { from { width: 0px } to { width: 100px } }\
                  #t { display: none; animation: k 1s linear }";
    assert_eq!(
        frame(&page, hidden, &mut c, 900.0, PropertyId::Width),
        "auto"
    );
    assert!(kinds(&mut c).contains(&AnimationEventKind::AnimationCancel));
}

#[test]
fn script_animations_follow_web_animations_timing() {
    let mut c = AnimationController::new();
    c.tick(0.0);
    let keyframes = vec![
        EffectKeyframe {
            offset: Some(0.0),
            ..EffectKeyframe::default()
        },
        EffectKeyframe {
            offset: Some(1.0),
            ..EffectKeyframe::default()
        },
    ];
    let timing = Timing {
        duration: 1000.0,
        fill: Fill::Forwards,
        ..Timing::default()
    };
    let effect = KeyframeEffect::new(None, keyframes, timing);
    let id = c.create(Some(effect), TimelineRef::Document);
    c.play(id, true).unwrap();
    assert_eq!(c.play_state(id), Some(PlayStateValue::Running));
    assert!(c.get(id).unwrap().pending());
    c.tick(100.0);
    assert!(!c.get(id).unwrap().pending());
    c.tick(600.0);
    assert_eq!(c.current_time(id), Some(500.0));
    c.pause(id).unwrap();
    c.tick(700.0);
    c.tick(900.0);
    assert_eq!(c.current_time(id), Some(600.0));
    assert_eq!(c.play_state(id), Some(PlayStateValue::Paused));
    c.set_playback_rate(id, 2.0).unwrap();
    c.play(id, true).unwrap();
    c.tick(1000.0);
    c.tick(1200.0);
    assert_eq!(c.current_time(id), Some(1000.0));
    assert_eq!(c.play_state(id), Some(PlayStateValue::Finished));
    let events = kinds(&mut c);
    assert!(events.contains(&AnimationEventKind::Ready), "{events:?}");
    assert!(events.contains(&AnimationEventKind::Finish), "{events:?}");
    c.reverse(id).unwrap();
    c.tick(1300.0);
    c.tick(1400.0);
    assert_eq!(c.current_time(id), Some(800.0));
    c.cancel(id);
    assert_eq!(c.current_time(id), None);
    assert_eq!(c.play_state(id), Some(PlayStateValue::Idle));
    assert!(kinds(&mut c).contains(&AnimationEventKind::Cancel));
    let _ = KeyframeValues::Declarations(Vec::new());
}

#[test]
fn timing_phases_and_iterations() {
    let timing = Timing {
        delay: 100.0,
        duration: 200.0,
        iterations: 2.5,
        fill: Fill::Both,
        ..Timing::default()
    };
    assert_eq!(timing.active_duration(), 500.0);
    assert_eq!(timing.end_time(), 600.0);
    let before = timing.compute(Some(0.0), false);
    assert_eq!(before.progress, Some(0.0));
    let middle = timing.compute(Some(350.0), false);
    assert_eq!(middle.current_iteration, Some(1.0));
    assert!((middle.progress.unwrap() - 0.25).abs() < 1e-9);
    let after = timing.compute(Some(1000.0), false);
    assert_eq!(after.current_iteration, Some(2.0));
    assert!((after.progress.unwrap() - 0.5).abs() < 1e-9);
}

//! The read-only CSS Object Model: `document.styleSheets` and rule objects.

use rustic_base::ManualClock;
use rustic_engine::{Engine, ScriptTurn, ScriptedDocument, TaskLimits};
use rustic_html::ParserLimits;
use rustic_js::Source;

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

#[test]
fn style_sheets_expose_scope_media_and_style_rules() {
    let mut engine = Engine::new(1);
    let mut page = page(
        &mut engine,
        "<!doctype html><style media='screen, print'>
           @import url(a.css) layer(base) supports(display: grid) print;
           p.x > a { color: red !important; width: 1px }
           @scope (.card) to (.content) { width: 2px; img { height: 3px } }
           @scope { p { color: blue } }
           @media (min-width: 10px) { p { margin: 0 } }
           @layer one, two;
           @keyframes spin { from { opacity: 0 } 50%, to { opacity: 1 } }
         </style><style type=text/plain>p {}</style><link rel=stylesheet href=x.css>",
    );
    run(
        &mut page,
        "var sheets = document.styleSheets;
         if (!(sheets instanceof StyleSheetList) || sheets.length !== 2) throw 'sheets ' + sheets.length;
         var sheet = sheets[0];
         if (!(sheet instanceof CSSStyleSheet) || sheet.ownerNode !== document.querySelector('style'))
             throw 'owner';
         if (document.styleSheets[0] !== sheet) throw 'identity';
         if (sheet.media.mediaText !== 'screen, print' || sheet.media.length !== 2) throw sheet.media.mediaText;
         var rules = sheet.cssRules;
         if (rules.length !== 7) throw 'rules ' + rules.length;
         var imp = rules[0];
         if (!(imp instanceof CSSImportRule) || imp.href !== 'a.css' || imp.layerName !== 'base'
             || imp.supportsText !== 'display: grid' || imp.media.mediaText !== 'print' || imp.type !== 3)
             throw 'import ' + [imp.href, imp.layerName, imp.supportsText, imp.media.mediaText];
         var style = rules[1];
         if (!(style instanceof CSSStyleRule) || style.selectorText !== 'p.x > a' || style.type !== 1)
             throw 'style ' + style.selectorText;
         if (style.style.getPropertyValue('color') !== 'red' || style.style.getPropertyPriority('color') !== 'important'
             || style.style.length !== 2 || style.style.item(1) !== 'width')
             throw 'declarations ' + style.style.cssText;
         if (style.parentStyleSheet !== sheet || style.parentRule !== null) throw 'parents';
         var scope = rules[2];
         if (!(scope instanceof CSSScopeRule) || !(scope instanceof CSSGroupingRule)) throw 'scope type';
         if (scope.start !== '.card' || scope.end !== '.content') throw 'scope ' + scope.start + ' ' + scope.end;
         if (scope.cssRules.length !== 2 || !(scope.cssRules[0] instanceof CSSNestedDeclarations)
             || scope.cssRules[0].style.getPropertyValue('width') !== '2px')
             throw 'scoped declarations';
         if (scope.cssRules[1].selectorText !== 'img' || scope.cssRules[1].parentRule !== scope) throw 'scoped rule';
         if (rules[3].start !== null || rules[3].end !== null) throw 'implicit scope';
         var media = rules[4];
         if (!(media instanceof CSSMediaRule) || media.conditionText !== '(min-width: 10px)'
             || media.media.mediaText !== '(min-width: 10px)' || media.cssRules.length !== 1) throw 'media';
         if (!(rules[5] instanceof CSSLayerStatementRule) || rules[5].nameList.join() !== 'one,two') throw 'layers';
         var frames = rules[6];
         if (!(frames instanceof CSSKeyframesRule) || frames.name !== 'spin' || frames.cssRules.length !== 2
             || frames.cssRules[1].keyText !== '50%, to') throw 'keyframes';
         if (sheets[1].cssRules.length !== 0) throw 'linked sheet';
         document.querySelector('style').textContent = 'a { b: c }';
         if (document.styleSheets[0] === sheet || document.styleSheets[0].cssRules.length !== 1) throw 'rebuild';",
    );
}

//! a11y.semantics: roles, names, states and pruning on real parsed HTML
//! (HTML-AAM implicit roles, ARIA author roles, AccName 1.2).

use rustic_a11y::{
    AxId, Checked, ControlState, LiveState, Rect, Rendering, Role, TreeLimits, build, wire,
};
use rustic_base::CancellationToken;
use rustic_dom::{Document, NodeId, NodeKind};
use rustic_html::{HtmlDocumentParser, ParsedDocument, ParserLimits};

fn parse(html: &str) -> ParsedDocument {
    let mut parser =
        HtmlDocumentParser::new(ParserLimits::default(), CancellationToken::default()).unwrap();
    parser.feed(html.as_bytes()).unwrap();
    parser.finish().unwrap()
}

fn dump(html: &str) -> String {
    let page = parse(html);
    build(
        page.document(),
        None,
        &LiveState::default(),
        TreeLimits::default(),
    )
    .dump(1 << 20)
    .unwrap()
}

fn by_id(document: &Document, id: &str) -> NodeId {
    document
        .walk(document.root())
        .unwrap()
        .map(|(node, _)| node)
        .find(|&node| match document.node(node).unwrap().kind() {
            NodeKind::Element(element) => element.attributes().iter().any(|a| {
                a.name().local() == "id" && String::from_utf16_lossy(a.value().as_units()) == id
            }),
            _ => false,
        })
        .unwrap()
}

#[test]
fn landmarks_headings_lists_and_links() {
    let tree = dump(
        "<!doctype html><title>Shop</title><header><nav><ul><li><a href=/>Home</a>\
         <li><a href=/cart>Cart <b>(2)</b></a></ul></nav></header>\
         <main><h1>Products</h1><h3 aria-level=2>Fruit</h3><p>Fresh <em>today</em>.</p>\
         <section>plain</section><section aria-label=Offers>deals</section>\
         <article><header>byline</header></article></main><footer>(c)</footer>",
    );
    assert_eq!(
        tree,
        "document \"Shop\"\n\
         \x20 banner\n\
         \x20   navigation\n\
         \x20     list\n\
         \x20       listitem\n\
         \x20         link \"Home\" url=\"/\" focusable\n\
         \x20           text \"Home\"\n\
         \x20       listitem\n\
         \x20         link \"Cart (2)\" url=\"/cart\" focusable\n\
         \x20           text \"Cart\"\n\
         \x20           text \"(2)\"\n\
         \x20 main\n\
         \x20   heading \"Products\" level=1\n\
         \x20     text \"Products\"\n\
         \x20   heading \"Fruit\" level=2\n\
         \x20     text \"Fruit\"\n\
         \x20   paragraph\n\
         \x20     text \"Fresh\"\n\
         \x20     emphasis\n\
         \x20       text \"today\"\n\
         \x20     text \".\"\n\
         \x20   text \"plain\"\n\
         \x20   region \"Offers\"\n\
         \x20     text \"deals\"\n\
         \x20   article\n\
         \x20     text \"byline\"\n\
         \x20 contentinfo\n\
         \x20   text \"(c)\"\n"
    );
}

#[test]
fn form_controls_get_labels_values_and_states() {
    let tree = dump(
        r#"<!doctype html><form>
        <label for=n>Name</label><input id=n required placeholder="Jane">
        <label>Email <input type=email value=a@b.c></label>
        <input aria-label="Search terms" type=search>
        <input type=checkbox checked id=c><label for=c>Agree</label>
        <input type=radio disabled title="Option A">
        <input type=password value=secret aria-labelledby=pw-l><span id=pw-l>Password</span>
        <textarea placeholder=Notes>
hello</textarea>
        <select><option>One<option selected>Two</select>
        <input type=range min=0 max=10 value=3 aria-label=Volume>
        <input type=submit><input type=reset value=Clear><input type=image alt=Go>
        <button>Send <img alt=now src=x.png></button>
        <fieldset disabled><legend>Group</legend><input aria-label=Inner></fieldset>
        <input type=hidden value=x>
        </form>"#,
    );
    let expected = [
        "form",
        "  label",
        "    text \"Name\"",
        "  textbox \"Name\" value=\"\" placeholder=\"Jane\" focusable required editable",
        "  label",
        "    text \"Email\"",
        "    textbox \"Email\" value=\"a@b.c\" focusable editable",
        "  searchbox \"Search terms\" value=\"\" focusable editable",
        "  checkbox \"Agree\" focusable checked=true",
        "  label",
        "    text \"Agree\"",
        "  radio \"Option A\" disabled checked=false",
        "  textbox \"Password\" value=\"\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\" focusable editable protected",
        "  text \"Password\"",
        "  textbox \"Notes\" value=\"hello\" placeholder=\"Notes\" focusable editable multiline",
        "  combobox value=\"Two\" focusable",
        "    option \"One\" selected=false",
        "      text \"One\"",
        "    option \"Two\" selected=true",
        "      text \"Two\"",
        "  slider \"Volume\" value=\"3\" focusable range=0..10 now=3",
        "  button \"Submit\" focusable",
        "  button \"Clear\" focusable",
        "  button \"Go\" focusable",
        "  button \"Send now\" focusable",
        "    text \"Send\"",
        "    img \"now\"",
        "  group \"Group\" disabled",
        "    legend",
        "      text \"Group\"",
        "    textbox \"Inner\" value=\"\" disabled editable",
    ];
    let body: Vec<&str> = tree.lines().skip(1).map(|l| &l[2..]).collect();
    assert_eq!(body, expected, "\n{tree}");
}

#[test]
fn aria_roles_states_hiding_and_presentation() {
    let tree = dump(
        r#"<!doctype html>
        <div role=button tabindex=0 aria-pressed=true>Bold</div>
        <div role="bogus tab" aria-selected=true>First</div>
        <div role=checkbox aria-checked=mixed aria-label=All></div>
        <div role=dialog aria-modal=true aria-labelledby=t><h2 id=t>Confirm</h2></div>
        <div aria-hidden=true><button>Hidden</button></div>
        <p hidden>gone</p>
        <ul role=presentation><li role=none>flat</li></ul>
        <button role=presentation>Still a button</button>
        <img src=a.png alt="">
        <img src=b.png alt="">
        <div role=alert>Saved</div>
        <div aria-live=polite id=live>Count 3</div>
        <div role=slider aria-valuemin=1 aria-valuemax=5 aria-valuenow=2 aria-valuetext=Two aria-label=Rating tabindex=0></div>
        <details open><summary>More</summary>body</details>
        <span title=Tip>hover</span>
        <div tabindex=-1 aria-describedby=d1 aria-label=Box></div><p id=d1>described</p>
        "#,
    );
    let body: Vec<&str> = tree.lines().skip(1).map(|l| &l[2..]).collect();
    let expected = [
        "button \"Bold\" focusable pressed=true",
        "  text \"Bold\"",
        "tab \"First\" selected=true",
        "  text \"First\"",
        "checkbox \"All\" checked=mixed",
        "dialog \"Confirm\" modal",
        "  heading \"Confirm\" level=2",
        "    text \"Confirm\"",
        "text \"flat\"",
        "button \"Still a button\" focusable",
        "  text \"Still a button\"",
        "alert live=assertive",
        "  text \"Saved\"",
        "generic live=polite",
        "  text \"Count 3\"",
        "slider \"Rating\" value=\"Two\" focusable range=1..5 now=2",
        "details \"More\" expanded=true",
        "  summary \"More\" focusable expanded=true",
        "    text \"More\"",
        "  text \"body\"",
        "text \"hover\"",
        "generic \"Box\" description=\"described\" focusable",
        "paragraph",
        "  text \"described\"",
    ];
    assert_eq!(body, expected, "\n{tree}");
}

#[test]
fn tables_name_from_caption_and_headers_by_scope() {
    let tree = dump(
        "<!doctype html><table><caption>Prices</caption><thead><tr><th>Item<th>Cost</thead>\
         <tbody><tr><th>Apple<td>1</tbody></table>",
    );
    let body: Vec<&str> = tree.lines().skip(1).map(|l| &l[2..]).collect();
    assert_eq!(
        body,
        [
            "table \"Prices\"",
            "  caption",
            "    text \"Prices\"",
            "  rowgroup",
            "    row \"Item Cost\"",
            "      columnheader \"Item\"",
            "        text \"Item\"",
            "      columnheader \"Cost\"",
            "        text \"Cost\"",
            "  rowgroup",
            "    row \"Apple 1\"",
            "      rowheader \"Apple\"",
            "        text \"Apple\"",
            "      cell \"1\"",
            "        text \"1\"",
        ],
        "\n{tree}"
    );
}

#[test]
fn accname_edge_cases() {
    // Labelledby chains do not recurse, embedded controls contribute their
    // values, hidden referenced content still names, and cycles end.
    let tree = dump(
        r#"<!doctype html>
        <span id=a aria-labelledby=b>A-label</span><span id=b>B</span>
        <button aria-labelledby=a>x</button>
        <label>Flash <input type=checkbox id=f> every <input value=5 size=2> minutes</label>
        <span id=h hidden>Secret name</span><button aria-labelledby=h>y</button>
        <div id=c1 aria-labelledby=c2>one</div><div id=c2 aria-labelledby=c1 role=button>two</div>
        <a href=#><img src=i.png alt="Logo"> home</a>
        "#,
    );
    let body: Vec<&str> = tree.lines().skip(1).map(|l| &l[2..]).collect();
    assert!(body.contains(&"button \"A-label\" focusable"), "\n{tree}");
    assert!(
        body.contains(&"  checkbox \"Flash every 5 minutes\" focusable checked=false"),
        "\n{tree}"
    );
    assert!(
        body.contains(&"button \"Secret name\" focusable"),
        "\n{tree}"
    );
    assert!(body.contains(&"button \"one\""), "\n{tree}");
    assert!(
        body.contains(&"link \"Logo home\" url=\"#\" focusable"),
        "\n{tree}"
    );
}

#[test]
fn live_state_rendering_bounds_and_focus() {
    let page = parse(
        "<!doctype html><input id=t value=old><input id=c type=checkbox>\
         <div id=gone>none</div><div id=vis>seen</div>",
    );
    let doc = page.document();
    let (t, c, gone, vis) = (
        by_id(doc, "t"),
        by_id(doc, "c"),
        by_id(doc, "gone"),
        by_id(doc, "vis"),
    );
    let mut live = LiveState {
        focused: Some(t),
        ..LiveState::default()
    };
    live.controls.insert(
        t,
        ControlState {
            value: Some("typed".into()),
            invalid: true,
            selection: Some((1, 4)),
            ..ControlState::default()
        },
    );
    live.controls.insert(
        c,
        ControlState {
            checked: Some(Checked::Mixed),
            ..ControlState::default()
        },
    );
    let mut rendering = Rendering::new();
    let rect = |x: f64| Rect {
        x,
        y: 0.0,
        width: 10.0,
        height: 5.0,
    };
    // Everything renders except #gone (display: none).
    for (node, _) in doc.walk(doc.root()).unwrap() {
        if node != gone && doc.node(node).unwrap().parent() != Some(gone) {
            rendering.mark_rendered(node);
        }
    }
    rendering.add_fragment(t, rect(0.0));
    rendering.add_fragment(vis, rect(20.0));
    rendering.add_fragment(vis, rect(40.0));
    let tree = build(doc, Some(&rendering), &live, TreeLimits::default());
    assert_eq!(tree.focus, AxId::of(t));
    let text = tree.dump(1 << 16).unwrap();
    assert_eq!(
        text,
        "document\n\
         \x20 textbox value=\"typed\" focusable focused invalid editable selection=1..4 @0,0 10x5\n\
         \x20 checkbox focusable checked=mixed\n\
         \x20 text \"seen\"\n"
    );
    // A div with nothing to say is pruned, but its bounds were recorded.
    assert_eq!(rendering.bounds(vis).unwrap().width, 30.0);
    // The encoded tree survives the trip to the browser process.
    let decoded = wire::decode(&wire::encode(&tree), TreeLimits::default()).unwrap();
    assert_eq!(decoded, tree);
    assert_eq!(tree.get(AxId::of(t)).unwrap().role, Role::TextInput);
    assert!(tree.get(AxId::of(t)).unwrap().actions().set_value);
}

#[test]
fn limits_truncate_instead_of_failing() {
    let html = format!("<!doctype html>{}", "<p>x</p>".repeat(500));
    let page = parse(&html);
    let limits = TreeLimits {
        max_nodes: 100,
        ..TreeLimits::default()
    };
    let tree = build(page.document(), None, &LiveState::default(), limits);
    assert!(tree.truncated);
    assert!(tree.nodes.len() <= 100);
    let deep = format!("<!doctype html>{}", "<div role=group>".repeat(300));
    let mut parser_limits = ParserLimits::default();
    parser_limits.document.max_depth = 1000;
    let mut parser = HtmlDocumentParser::new(parser_limits, CancellationToken::default()).unwrap();
    parser.feed(deep.as_bytes()).unwrap();
    let page = parser.finish().unwrap();
    let tree = build(
        page.document(),
        None,
        &LiveState::default(),
        TreeLimits::default(),
    );
    assert!(tree.truncated);
    let long = format!("<!doctype html><button>{}</button>", "y".repeat(40_000));
    let page = parse(&long);
    let tree = build(
        page.document(),
        None,
        &LiveState::default(),
        TreeLimits::default(),
    );
    assert_eq!(tree.nodes[1].name.len(), 16 * 1024);
    // Page text cannot forge dump lines.
    let sneaky = dump("<!doctype html><button aria-label='a\"\u{85}b'>x</button>");
    assert!(sneaky.contains("button \"a\\\"\\u{85}b\""), "{sneaky}");
    assert_eq!(sneaky.lines().count(), 3, "{sneaky}");
}

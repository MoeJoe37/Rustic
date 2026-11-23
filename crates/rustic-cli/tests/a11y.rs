//! `rustic dump-a11y` (a11y.semantics, ADR 0069): the accessibility tree
//! of a laid-out page, through the sandboxed renderer like every document
//! command, with layout bounds, hidden content left out and script state.

use std::process::Command;

fn page(name: &str, html: &str) -> (std::path::PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("rustic-a11y-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let page = dir.join("page.html");
    std::fs::write(&page, html).unwrap();
    let shown = page.to_string_lossy().into_owned();
    (dir, shown)
}

fn dump(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_rustic"))
        .arg("dump-a11y")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn dumps_roles_names_bounds_and_leaves_out_hidden_content() {
    let (dir, path) = page(
        "layout",
        "<!doctype html><title>T</title><body style=margin:0><h1 style='margin:0;\
         font-size:20px;line-height:30px'>Hi</h1><a href=x>go</a>\
         <div style=display:none>gone</div><div style=visibility:hidden>unseen</div>",
    );
    let tree = dump(&[&path]);
    let lines: Vec<&str> = tree.lines().collect();
    assert!(lines[0].starts_with("document \"T\" @0,0 1280x"), "{tree}");
    assert!(
        lines[1].starts_with("  heading \"Hi\" level=1 @0,0 1280x30"),
        "{tree}"
    );
    assert!(
        lines[3].starts_with("  link \"go\" url=\"x\" focusable @0,30 "),
        "{tree}"
    );
    assert!(!tree.contains("gone") && !tree.contains("unseen"), "{tree}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reflects_focus_values_and_selection_scripts_left() {
    let (dir, path) = page(
        "script",
        "<!doctype html><label for=n>Name</label><input id=n>\
         <input type=checkbox id=c aria-label=Agree>\
         <script>const n = document.getElementById('n'); n.value = 'Ann'; n.focus();\
         n.setSelectionRange(1, 3); document.getElementById('c').checked = true;</script>",
    );
    let tree = dump(&["--run-scripts", &path]);
    assert!(
        tree.contains("textbox \"Name\" value=\"Ann\" focusable focused editable selection=1..3"),
        "{tree}"
    );
    assert!(
        tree.contains("checkbox \"Agree\" focusable checked=true"),
        "{tree}"
    );
    // Without scripts the DOM's own state shows.
    let plain = dump(&[&path]);
    assert!(
        plain.contains("textbox \"Name\" value=\"\" focusable editable\n"),
        "{plain}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

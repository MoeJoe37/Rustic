//! Build a small tree through the storage API. No HTML parser is involved.

use rustic_dom::{Document, DocumentLimits, DocumentType, DomString, QualifiedName, SVG_NAMESPACE};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut document = Document::new(DocumentLimits::default())?;
    let root = document.root();
    let doctype = document.create_doctype(DocumentType {
        name: "html".into(),
        public_id: DomString::default(),
        system_id: DomString::default(),
    })?;
    document.append_child(root, doctype)?;
    let html = document.create_element(QualifiedName::html("html")?)?;
    document.append_child(root, html)?;
    let body = document.create_element(QualifiedName::html("body")?)?;
    document.append_child(html, body)?;
    document.set_attribute(
        body,
        QualifiedName::from_parsed(None, None, "id")?,
        "rustic-demo",
    )?;
    let text = document.create_text("Hello, Rustic — مرحباً")?;
    document.append_child(body, text)?;
    let svg = document.create_element(QualifiedName::from_parsed(
        Some(SVG_NAMESPACE),
        None,
        "svg",
    )?)?;
    document.append_child(body, svg)?;
    print!("{}", document.dump_tree(root, 64 * 1024)?);
    Ok(())
}

use rustic_html::HtmlDocumentParser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = "<!doctype html><title>Rustic &amp; trees</title><body><p>Hello <span>Rust 🙂</span><p>Another paragraph<div>Recovered block</div><!-- inert local fixture -->";
    let mut parser = HtmlDocumentParser::default();
    for chunk in source.as_bytes().chunks(7) {
        parser.feed(chunk)?;
    }
    let parsed = parser.finish()?;
    println!("mode: {:?}", parsed.mode());
    print!(
        "{}",
        parsed
            .document()
            .dump_tree(parsed.document().root(), 16 * 1024)?
    );
    println!("tree recoveries: {}", parsed.tree_diagnostics().len());
    Ok(())
}

use rustic_html::HtmlTokenizer;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = "<DIV id='sample'>Hello, Rustic 🙂<br /></DIV>".as_bytes();
    let mut tokenizer = HtmlTokenizer::default();
    // Deliberately split tags and a multibyte character across feeds.
    for chunk in bytes.chunks(3) {
        for token in tokenizer.feed(chunk)? {
            println!("{token:?}");
        }
    }
    for token in tokenizer.finish()? {
        println!("{token:?}");
    }
    for diagnostic in tokenizer.diagnostics() {
        eprintln!("{diagnostic:?}");
    }
    Ok(())
}

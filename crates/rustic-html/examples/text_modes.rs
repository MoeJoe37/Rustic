use rustic_html::{HtmlTokenizer, Token, TokenizerMode};

fn drain(tokenizer: &mut HtmlTokenizer) -> Result<(), rustic_html::HtmlError> {
    while let Some(token) = tokenizer.next_token()? {
        // This known HTML-only fixture chooses modes explicitly. A real tree
        // builder must use namespace, insertion mode and scripting context too.
        if let Token::StartTag(tag) = &token {
            let mode = match tag.name() {
                "title" => Some(TokenizerMode::Rcdata),
                "style" => Some(TokenizerMode::Rawtext),
                "script" => Some(TokenizerMode::ScriptData),
                _ => None,
            };
            if let Some(mode) = mode {
                tokenizer.set_text_mode(mode)?;
            }
        }
        println!("{token:?}");
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = "<title>Rustic 🙂 &amp; <b>literal</title><style>a::before {content:'<';}</style><script><!--<script>literal</script>--></script>";
    let mut tokenizer = HtmlTokenizer::default();
    for bytes in source.as_bytes().chunks(4) {
        tokenizer.write(bytes)?;
        drain(&mut tokenizer)?;
    }
    tokenizer.end()?;
    drain(&mut tokenizer)?;
    Ok(())
}

//! URL parsing demonstration; it performs no networking or document navigation.

use rustic_url::Url;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = Url::parse("https://example.test/docs/guide.html", None)?;
    let target = Url::parse("../assets/logo.svg?theme=dark#icon", Some(&base))?;
    println!("URL: {}", target.serialize_without_credentials());
    println!("Origin: {}", target.origin().serialize());
    Ok(())
}

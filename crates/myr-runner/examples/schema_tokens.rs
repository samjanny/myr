//! Count cl100k_base reference tokens of JSON values, compactly serialized, for
//! deterministic cost-pilot measurements. Usage: schema_tokens <file> <json-pointer>...
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: schema_tokens <file> <pointer>...")?;
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let tokenizer = myr_runner::accounting::ReferenceTokenizer::new()?;
    for pointer in args {
        let text = value
            .pointer(&pointer)
            .ok_or("pointer not found")?
            .to_string();
        println!("{pointer}\t{}\t{}", text.len(), tokenizer.count(&text));
    }
    Ok(())
}

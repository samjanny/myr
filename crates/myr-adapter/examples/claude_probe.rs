fn main() -> Result<(), Box<dyn std::error::Error>> {
    let executable = std::env::args_os()
        .nth(1)
        .ok_or("usage: claude_probe <claude-executable>")?;
    let result = myr_adapter::claude_code::probe(
        std::path::Path::new(&executable),
        std::time::Duration::from_secs(30),
    )?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

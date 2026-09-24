fn main() -> Result<(), Box<dyn std::error::Error>> {
    let destination = std::env::args_os()
        .nth(1)
        .ok_or("usage: pilot <new-output-directory>")?;
    let report = myr_runner::pilot::run(std::path::Path::new(&destination))?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

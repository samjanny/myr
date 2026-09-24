//! Explicit maintenance tool: changing the checked-in vectors requires review.
#[path = "../tests/support/mod.rs"]
mod support;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut vectors = Vec::new();
    for object in support::objects() {
        let object = myr_wire::normalize(&object)?;
        let (reference, bytes) = myr_wire::identify(&object)?;
        vectors.push(serde_json::json!({"object": object, "cbor_hex": hex::encode(bytes), "cid": reference.cid.to_string()}));
    }
    println!("{}", serde_json::to_string_pretty(&vectors)?);
    Ok(())
}

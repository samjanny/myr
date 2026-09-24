use myr_core::Object;

#[test]
fn checked_in_vectors_cover_every_kind_and_match_cids() {
    let vectors: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../../test-vectors/mw0-v0.json")).unwrap();
    let mut kinds = std::collections::BTreeSet::new();
    for v in vectors {
        let object: Object = serde_json::from_value(v["object"].clone()).unwrap();
        let bytes = hex::decode(v["cbor_hex"].as_str().unwrap()).unwrap();
        let (reference, encoded) = myr_wire::identify(&object).unwrap();
        assert_eq!(encoded, bytes);
        assert_eq!(reference.cid.to_string(), v["cid"].as_str().unwrap());
        assert_eq!(myr_wire::decode(&bytes).unwrap(), object);
        kinds.insert(object.kind() as u8);
    }
    assert_eq!(kinds, (1..=10).collect());
}

#[test]
fn claim_vector_matches_hand_constructed_rfc8949_layout() {
    // Independently assembled field order and CBOR type/length bytes. The three
    // references are raw byte strings, not JSON hex text or arrays of integers.
    let expected = format!(
        "a30000010202a30082095820{}01f502a200820b5820{}0182005820{}",
        "04".repeat(32),
        "01".repeat(32),
        "02".repeat(32)
    );
    let vectors: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../../../test-vectors/mw0-v0.json")).unwrap();
    assert_eq!(vectors[1]["cbor_hex"], expected);
}

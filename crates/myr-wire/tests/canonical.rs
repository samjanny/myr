use myr_core::*;
use myr_wire::*;
mod support;
use support::*;

#[test]
fn every_message_kind_roundtrips_with_raw_cids() {
    for o in objects() {
        let bytes = encode(&o).unwrap();
        assert_eq!(decode(&bytes).unwrap(), normalize(&o).unwrap());
        assert_eq!(encode(&decode(&bytes).unwrap()).unwrap(), bytes);
        assert!(!bytes.windows(3).any(|w| w == b"b3:"));
    }
}
#[test]
fn sets_normalize_before_sorting_and_reject_duplicates() {
    let mut a = task();
    let mut b = task();
    if let Object::Task(t) = &mut b {
        t.capabilities.reverse();
    }
    assert_eq!(identify(&a).unwrap(), identify(&b).unwrap());
    if let Object::Task(t) = &mut a {
        t.capabilities = vec!["café".into(), "cafe\u{301}".into()];
    }
    assert!(encode(&a).is_err());
}
#[test]
fn ordered_arguments_keep_order_and_decimal_normalizes() {
    let mut a = Object::Atom(Atom {
        predicate: r(Kind::PredicateDef, 1),
        arguments: vec![Argument::Integer(1), Argument::Integer(2)],
    });
    let cid = identify(&a).unwrap().0;
    if let Object::Atom(atom) = &mut a {
        atom.arguments.reverse();
    }
    assert_ne!(cid, identify(&a).unwrap().0);
    let mut zero = Decimal {
        mantissa: 0,
        exponent: i64::MIN,
    };
    zero.normalize().unwrap();
    assert_eq!(zero.exponent, 0);
    let mut overflow = Decimal {
        mantissa: 10,
        exponent: i64::MAX,
    };
    assert!(overflow.normalize().is_err());
}
#[test]
fn artifact_bytes_are_opaque_and_domain_separated() {
    assert_ne!(artifact_cid(b"a\r\n"), artifact_cid(b"a\n"));
    assert_ne!(artifact_cid(b""), message_cid(b""));
    assert_ne!(
        artifact_cid("café".as_bytes()),
        artifact_cid("cafe\u{301}".as_bytes())
    );
}
#[test]
fn malformed_and_noncanonical_wire_is_rejected() {
    let valid = encode(&objects()[1]).unwrap();
    let mut trailing = valid.clone();
    trailing.push(0);
    assert!(decode(&trailing).is_err());
    let mut long_version = valid.clone();
    long_version.splice(2..3, [0x18, 0]);
    assert!(decode(&long_version).is_err());
    let mut indefinite = valid.clone();
    indefinite[0] = 0xbf;
    indefinite.push(0xff);
    assert!(decode(&indefinite).is_err());
    for length in 0..valid.len() {
        assert!(decode(&valid[..length]).is_err());
    }
    // Payload contains an unknown key, duplicate key, or null required field.
    for bytes in [
        "a30000010702a10000",
        "a30000010702a40001000302582000000000000000000000000000000000000000000000000000000000000000000200",
        "a30000010702a30001010302f6",
    ] {
        assert!(decode(&hex::decode(bytes).unwrap()).is_err());
    }
}
#[test]
fn provider_identity_is_not_in_claim_content() {
    let claim = objects().remove(1);
    let json = render(&claim).unwrap();
    assert!(!json.contains("provider"));
    assert!(!json.contains("agent"));
    let parsed: Object = serde_json::from_str(&json).unwrap();
    assert_eq!(identify(&claim).unwrap().0, identify(&parsed).unwrap().0);
}
#[test]
fn portable_paths_reject_traversal_and_windows_aliases() {
    for path in [
        "../x", "/x", "a\\b", "C:x", "a//b", "a/./b", "NUL.txt", "aux", "COM1.rs", "x.", "x ",
        "x:y", "x\0y",
    ] {
        assert!(validate_relative_path(path).is_err(), "{path}");
    }
    for path in ["src/lib.rs", "tests/cache.rs", "résumé.txt"] {
        validate_relative_path(path).unwrap();
    }
}
#[test]
fn unknown_json_fields_cannot_enter_semantic_objects() {
    let mut value = serde_json::to_value(objects().remove(1)).unwrap();
    value["payload"]["agent"] = "forged".into();
    assert!(serde_json::from_value::<Object>(value).is_err());
}

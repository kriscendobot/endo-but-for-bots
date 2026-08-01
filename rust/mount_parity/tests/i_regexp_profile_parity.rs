//! Rust-side ownership check for the fixed @endo/regexp contract.
//!
//! This runner deliberately consumes the same asset as the JS package without
//! introducing a provisional Rust regexp engine. The `endor` backend must
//! replace this envelope check with the same acceptance and match assertions;
//! its engine direction is xsre / the #600 Rust port, not Rust `regex`.

use mount_parity::regexp_contract_dir;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ValidityCase {
    source: String,
    accepted: bool,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MatchCase {
    source: String,
    text: String,
    matches: bool,
}

#[derive(Debug, Deserialize)]
struct ProfileCases {
    profile: String,
    validity: Vec<ValidityCase>,
    matches: Vec<MatchCase>,
    contains: Vec<MatchCase>,
}

#[test]
fn i_regexp_profile_corpus_is_present_and_well_formed() {
    let path = regexp_contract_dir().join("i-regexp-profile-cases.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let cases: ProfileCases = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));

    assert_eq!(cases.profile, "endo-i-regexp-v1");
    assert!(cases.validity.len() >= 10, "validity corpus must not be skeletal");
    assert!(cases.matches.len() >= 8, "match corpus must not be skeletal");
    assert!(!cases.contains.is_empty(), "contains mode needs a contract case");

    for case in &cases.validity {
        assert!(!case.source.is_empty() || case.accepted, "empty source is RFC-valid");
        if case.accepted {
            assert!(case.reason.is_none(), "accepted source has no diagnostic");
        } else {
            assert!(
                matches!(
                    case.reason.as_deref(),
                    Some("syntax")
                        | Some("unicode-property")
                        | Some("ambiguous-repetition")
                        | Some("resource-limit")
                ),
                "rejected source has a profile diagnostic"
            );
        }
    }
    for case in cases.matches.iter().chain(&cases.contains) {
        assert!(!case.source.is_empty(), "match cases use an explicit source");
        let _ = (&case.text, case.matches);
    }
}

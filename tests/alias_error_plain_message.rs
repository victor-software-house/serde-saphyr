#![cfg(feature = "deserialize")]
//! Tests that the deprecated `msg` field of `Error::AliasError` holds the plain message of the
//! error it wraps, the text that error's `Display` writes, for an alias replayed inside the
//! replay of another alias.

use serde::Deserialize;
use serde_saphyr::Error;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Port {
    port: u16,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Copy {
    copy: Port,
}

const NESTED_ALIASES: &str = "value: &v eighty\nbase: &b\n  port: *v\ncopy: *b\n";

#[test]
#[allow(deprecated)] // `msg` is the field under test.
fn alias_inside_an_alias_keeps_the_plain_message_of_the_failing_value() {
    for outer in [
        serde_saphyr::from_str::<Copy>(NESTED_ALIASES).unwrap_err(),
        serde_saphyr::from_reader::<_, Copy>(NESTED_ALIASES.as_bytes()).unwrap_err(),
    ] {
        let Error::AliasError { msg, error, .. } = outer.without_snippet() else {
            panic!("expected an alias error, got {outer:?}");
        };
        assert!(matches!(
            error.as_ref(),
            Error::InvalidScalar { ty: "u16", .. }
        ));
        assert_eq!(msg, "invalid u16 at line 1, column 11");
        assert_eq!(*msg, error.to_string());
        assert_eq!(
            outer.without_snippet().to_string(),
            "invalid u16 at line 1, column 11 (defined at line 3, column 3) (used at line 4, column 7)"
        );
    }
}

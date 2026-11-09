#![cfg(feature = "deserialize")]
//! Tests that `deserialize_str` maps the `&str` visitor's owned-string error to
//! `CannotBorrowTransformedString` by matching the error variant, and leaves every
//! other visitor error as the visitor returned it.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer};
use serde_saphyr::{Error, TransformReason};

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BorrowedName<'a> {
    name: &'a str,
}

#[track_caller]
fn assert_parser_returned_owned(err: &Error, line: u64, column: u64) {
    assert!(
        matches!(
            err.without_snippet(),
            Error::CannotBorrowTransformedString {
                reason: TransformReason::ParserReturnedOwned,
                ..
            }
        ),
        "expected CannotBorrowTransformedString, got {err:?}"
    );
    let location = err.location().expect("the error keeps the scalar location");
    assert_eq!((location.line(), location.column()), (line, column));
}

#[test]
fn escaped_scalar_into_borrowed_str_field_reports_cannot_borrow() {
    let err = serde_saphyr::from_str::<BorrowedName<'_>>("name: \"a\\tb\"\n").unwrap_err();
    assert_parser_returned_owned(&err, 1, 7);
}

// The location of a block scalar is its first content line, below the `>` header, as in upstream 2.1.1.
#[test]
fn folded_scalar_into_borrowed_str_field_reports_cannot_borrow() {
    let err = serde_saphyr::from_str::<BorrowedName<'_>>("name: >\n  one\n  two\n").unwrap_err();
    assert_parser_returned_owned(&err, 2, 3);
}

#[test]
fn binary_scalar_into_borrowed_str_field_reports_cannot_borrow() {
    let err = serde_saphyr::from_str::<BorrowedName<'_>>("name: !!binary SGVsbG8=\n").unwrap_err();
    assert_parser_returned_owned(&err, 1, 16);
}

#[derive(Debug)]
struct BorrowedTextOnly;

impl<'de> Deserialize<'de> for BorrowedTextOnly {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(BorrowedTextOnlyVisitor)
    }
}

struct BorrowedTextOnlyVisitor;

impl<'de> Visitor<'de> for BorrowedTextOnlyVisitor {
    type Value = BorrowedTextOnly;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("borrowed text only")
    }

    fn visit_borrowed_str<E>(self, _value: &'de str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(BorrowedTextOnly)
    }
}

#[test]
fn other_type_errors_from_an_owned_string_stay_as_returned() {
    let err = serde_saphyr::from_str::<BorrowedTextOnly>("\"a\\tb\"\n").unwrap_err();
    assert!(
        matches!(
            err.without_snippet(),
            Error::SerdeInvalidType { expected, .. } if expected == "borrowed text only"
        ),
        "expected SerdeInvalidType, got {err:?}"
    );
}

//! Tests for [serde_saphyr::scalars]: each rule answers as [from_str_with_options] did before
//! the rules were made public.
//!
//! `fixtures/public_scalars.json` records what `from_str_with_options` returned for every row
//! under every option set, from the commit below the located reader and the scalar rules. Both
//! the deserializer and [Rules] are checked against that record, so a change to a rule that the
//! deserializer shares fails here instead of moving both sides together.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Debug;

use serde::Deserialize;
use serde_json::{Number, Value};
use serde_saphyr::granit_parser::ScalarStyle;
use serde_saphyr::located::{self, Node};
use serde_saphyr::scalars::{Inferred, Rules, Scalar};
use serde_saphyr::{
    Error, Location, NonFiniteFloatPolicy, Options, from_str_with_options, options,
};

/// The YAML text of every row whose answer is pinned against the deserializer.
const ROWS: &[&str] = &[
    "1.10",
    "010",
    "0x10",
    "0o17",
    "0b101",
    "1e3",
    "+1",
    "1_000",
    ".inf",
    ".nan",
    "inf",
    "True",
    "no",
    "~",
    "null",
    "",
    "'null'",
    "\"8\"",
    "!!str 1",
    "!!int \"5\"",
    "!override 0",
    "-7",
    "-0x10",
    "1e999",
    "-.inf",
    "!!binary aGVsbG8=",
    "!!float 1",
    "!!bool yes",
    "!!null x",
    "!!int 8080",
    "!!int ~",
    "!!binary aGk=",
    "!!binary \"!!!\"",
    "!!str ~",
    "'~'",
];

fn option_sets() -> Vec<(&'static str, Options)> {
    vec![
        ("default", options! { with_snippet: false }),
        (
            "strict_booleans",
            options! { with_snippet: false, strict_booleans: true },
        ),
        (
            "legacy_octal_numbers",
            options! { with_snippet: false, legacy_octal_numbers: true },
        ),
        (
            "leading_zero_numbers_as_strings",
            options! { with_snippet: false, leading_zero_numbers_as_strings: true },
        ),
        (
            "underscored_numbers_as_strings",
            options! { with_snippet: false, underscored_numbers_as_strings: true },
        ),
        (
            "reject non-finite",
            options! { with_snippet: false, non_finite_float_policy: NonFiniteFloatPolicy::Reject },
        ),
        (
            "pass non-finite through",
            options! {
                with_snippet: false,
                non_finite_float_policy: NonFiniteFloatPolicy::PassThrough,
            },
        ),
        (
            "ignore the binary tag",
            options! { with_snippet: false, ignore_binary_tag_for_string: true },
        ),
    ]
}

fn read_scalar(text: &str) -> (String, ScalarStyle, Option<String>, Location) {
    match located::read(text, Options::default()).expect("the row reads") {
        Node::Scalar(scalar) => (
            scalar.value,
            scalar.style,
            scalar.tag,
            scalar.locations.defined_location,
        ),
        other => panic!("{text:?} is not a scalar: {other:?}"),
    }
}

/// What one read gave: the `Debug` text of the value, or the `Display` text of the error.
#[derive(Debug, PartialEq, Eq, Deserialize)]
enum Answer {
    #[serde(rename = "ok")]
    Read(String),
    #[serde(rename = "error")]
    Refused(String),
}

fn answer<T: Debug>(result: Result<T, Error>) -> Answer {
    match result {
        Ok(value) => Answer::Read(format!("{value:?}")),
        Err(error) => Answer::Refused(error.to_string()),
    }
}

/// One row under one option set: the reads `from_str_with_options` gave, by type name, and the
/// reads where [Rules] words its answer differently by design.
#[derive(Deserialize)]
struct Recorded {
    text: String,
    options: String,
    reads: BTreeMap<String, Answer>,
    #[serde(default)]
    rules: BTreeMap<String, Answer>,
}

impl Recorded {
    fn deserializer(&self, read: &str) -> &Answer {
        &self.reads[read]
    }

    fn rules(&self, read: &str) -> &Answer {
        self.rules
            .get(read)
            .unwrap_or_else(|| self.deserializer(read))
    }
}

fn recorded() -> Vec<Recorded> {
    serde_json::from_str(include_str!("fixtures/public_scalars.json"))
        .expect("the fixture is valid")
}

/// Every row under every option set, with its record.
fn each_row(mut check: impl FnMut(&str, &str, &Options, &Recorded)) {
    let recorded = recorded();
    for (name, options) in option_sets() {
        for text in ROWS {
            let row = recorded
                .iter()
                .find(|row| row.text == *text && row.options == name)
                .unwrap_or_else(|| panic!("no record of {text:?} under {name}"));
            check(text, name, &options, row);
        }
    }
}

fn to_json(inferred: Inferred<'_>) -> Value {
    match inferred {
        Inferred::Null => Value::Null,
        Inferred::Bool(value) => Value::Bool(value),
        Inferred::Signed(value) => Value::Number(value.into()),
        Inferred::Unsigned(value) => Value::Number(value.into()),
        // serde_json's own visitor turns a non-finite float into null.
        Inferred::Float(value) => Number::from_f64(value).map_or(Value::Null, Value::Number),
        Inferred::String(value) => Value::String(value.into_owned()),
    }
}

#[test]
fn the_deserializer_answers_as_recorded_for_every_row_and_option_set() {
    each_row(|text, name, options, row| {
        let reads = [
            (
                "typeless",
                answer(from_str_with_options::<Value>(text, options.clone())),
            ),
            (
                "string",
                answer(from_str_with_options::<String>(text, options.clone())),
            ),
            (
                "bool",
                answer(from_str_with_options::<bool>(text, options.clone())),
            ),
            (
                "i64",
                answer(from_str_with_options::<i64>(text, options.clone())),
            ),
            (
                "u64",
                answer(from_str_with_options::<u64>(text, options.clone())),
            ),
            (
                "f64",
                answer(from_str_with_options::<f64>(text, options.clone())),
            ),
        ];
        for (read, got) in reads {
            assert_eq!(
                &got,
                row.deserializer(read),
                "{text:?} as {read} under {name}"
            );
        }
    });
}

/// The rules answer as the record says for every read they offer. The empty row is the one place
/// the wording differs by design, under every option set: the deserializer reads an empty input
/// as a document with no node, so a typed read of it fails with "unexpected end of input", while
/// the rules answer for a scalar that exists and is empty, and fail with the error of the type
/// they were asked for. The fixture records the rules' wording under `rules`.
#[test]
fn rules_answer_as_recorded_for_every_row_and_option_set() {
    each_row(|text, name, options, row| {
        let rules = Rules::new(options);
        let (value, style, tag, location) = read_scalar(text);
        let scalar = Scalar {
            value: &value,
            style,
            tag: tag.as_deref(),
        };
        let reads = [
            (
                "typeless",
                answer(rules.infer(&scalar, location).map(to_json)),
            ),
            ("string", answer(rules.string(&scalar, location))),
            ("bool", answer(rules.boolean(&scalar, location))),
            ("i64", answer(rules.signed::<i64>(&scalar, location))),
            ("u64", answer(rules.unsigned::<u64>(&scalar, location))),
            ("f64", answer(rules.float64(&scalar, location))),
        ];
        for (read, got) in reads {
            assert_eq!(&got, row.rules(read), "{text:?} as {read} under {name}");
        }
    });
}

#[test]
fn string_borrows_unchanged_text_and_owns_decoded_binary() {
    let rules = Rules::new(&Options::default());
    let read = |text: &str, check: &dyn Fn(Cow<'_, str>)| {
        let (value, style, tag, location) = read_scalar(text);
        let scalar = Scalar {
            value: &value,
            style,
            tag: tag.as_deref(),
        };
        check(
            rules
                .string(&scalar, location)
                .expect("the row reads as text"),
        );
    };
    read("plain", &|text| {
        assert!(matches!(text, Cow::Borrowed("plain")))
    });
    read("'~'", &|text| assert!(matches!(text, Cow::Borrowed("~"))));
    read("!!str ~", &|text| {
        assert!(matches!(text, Cow::Borrowed("~")))
    });
    read("!!binary aGk=", &|text| {
        assert!(matches!(text, Cow::Owned(ref owned) if owned == "hi"));
    });
}

#[test]
fn infer_answers_the_pinned_rows() {
    let rules = Rules::new(&options! {
        non_finite_float_policy: NonFiniteFloatPolicy::AsString,
    });
    let infer = |text: &str| {
        let (value, style, tag, location) = read_scalar(text);
        let scalar = Scalar {
            value: &value,
            style,
            tag: tag.as_deref(),
        };
        rules.infer(&scalar, location).map(to_json)
    };
    let string = |text: &str| Value::String(text.to_owned());
    assert_eq!(infer("1.10").unwrap(), serde_json::json!(1.1));
    assert_eq!(infer("010").unwrap(), serde_json::json!(10.0));
    assert_eq!(infer("0x10").unwrap(), serde_json::json!(16));
    assert_eq!(infer("0o17").unwrap(), serde_json::json!(15));
    assert_eq!(infer("0b101").unwrap(), serde_json::json!(5));
    assert_eq!(infer("1e3").unwrap(), serde_json::json!(1000.0));
    assert_eq!(infer("+1").unwrap(), serde_json::json!(1));
    assert_eq!(infer("1_000").unwrap(), serde_json::json!(1000));
    assert_eq!(infer(".inf").unwrap(), string(".inf"));
    assert_eq!(infer(".nan").unwrap(), string(".nan"));
    assert_eq!(infer("inf").unwrap(), string("inf"));
    assert_eq!(infer("True").unwrap(), Value::Bool(true));
    assert_eq!(infer("no").unwrap(), Value::Bool(false));
    assert_eq!(infer("~").unwrap(), Value::Null);
    assert_eq!(infer("null").unwrap(), Value::Null);
    assert_eq!(infer("").unwrap(), Value::Null);
    assert_eq!(infer("'null'").unwrap(), string("null"));
    assert_eq!(infer("\"8\"").unwrap(), string("8"));
    assert_eq!(infer("!!str 1").unwrap(), string("1"));
    assert_eq!(infer("!!int \"5\"").unwrap(), serde_json::json!(5));
    assert_eq!(infer("!override 0").unwrap(), serde_json::json!(0));
}

macro_rules! typed_rows {
    ($($test:ident: $method:ident::<$ty:ty>),* $(,)?) => {$(
        #[test]
        fn $test() {
            for (name, options) in option_sets() {
                let rules = Rules::new(&options);
                for text in ROWS {
                    let (value, style, tag, location) = read_scalar(text);
                    let scalar = Scalar { value: &value, style, tag: tag.as_deref() };
                    let rule = rules.$method::<$ty>(&scalar, location);
                    let deserialized = from_str_with_options::<$ty>(text, options.clone());
                    match (rule, deserialized) {
                        (Ok(rule), Ok(deserialized)) => assert_eq!(
                            format!("{rule:?}"),
                            format!("{deserialized:?}"),
                            "{text:?} as {} under {name}",
                            stringify!($ty),
                        ),
                        (Err(_), Err(_)) => {}
                        (rule, deserialized) => panic!(
                            "{text:?} as {} under {name}: {rule:?} against {deserialized:?}",
                            stringify!($ty),
                        ),
                    }
                }
            }
        }
    )*};
}

macro_rules! float_rows {
    ($($test:ident: $method:ident -> $ty:ty),* $(,)?) => {$(
        #[test]
        fn $test() {
            for (name, options) in option_sets() {
                let rules = Rules::new(&options);
                for text in ROWS {
                    let (value, style, tag, location) = read_scalar(text);
                    let scalar = Scalar { value: &value, style, tag: tag.as_deref() };
                    let rule = rules.$method(&scalar, location);
                    let deserialized = from_str_with_options::<$ty>(text, options.clone());
                    match (rule, deserialized) {
                        (Ok(rule), Ok(deserialized)) => assert_eq!(
                            format!("{rule:?}"),
                            format!("{deserialized:?}"),
                            "{text:?} as {} under {name}",
                            stringify!($ty),
                        ),
                        (Err(_), Err(_)) => {}
                        (rule, deserialized) => panic!(
                            "{text:?} as {} under {name}: {rule:?} against {deserialized:?}",
                            stringify!($ty),
                        ),
                    }
                }
            }
        }
    )*};
}

typed_rows! {
    signed_i8_matches_the_deserializer: signed::<i8>,
    signed_i16_matches_the_deserializer: signed::<i16>,
    signed_i32_matches_the_deserializer: signed::<i32>,
    signed_i128_matches_the_deserializer: signed::<i128>,
    unsigned_u8_matches_the_deserializer: unsigned::<u8>,
    unsigned_u16_matches_the_deserializer: unsigned::<u16>,
    unsigned_u32_matches_the_deserializer: unsigned::<u32>,
    unsigned_u128_matches_the_deserializer: unsigned::<u128>,
}

float_rows! {
    float32_matches_the_deserializer: float32 -> f32,
}

#[test]
fn typed_answers_for_the_pinned_rows() {
    let rules = Rules::new(&Options::default());
    let answers = |text: &str| {
        let (value, style, tag, location) = read_scalar(text);
        let scalar = Scalar {
            value: &value,
            style,
            tag: tag.as_deref(),
        };
        (
            rules.signed::<i64>(&scalar, location).ok(),
            rules.unsigned::<u64>(&scalar, location).ok(),
            rules.float64(&scalar, location).ok(),
            rules.boolean(&scalar, location).ok(),
        )
    };
    assert_eq!(answers("1.10"), (None, None, Some(1.1), None));
    assert_eq!(answers("010"), (None, None, Some(10.0), None));
    assert_eq!(answers("0x10"), (Some(16), Some(16), None, None));
    assert_eq!(answers("0o17"), (Some(15), Some(15), None, None));
    assert_eq!(answers("0b101"), (Some(5), Some(5), None, None));
    assert_eq!(answers("1e3"), (None, None, Some(1000.0), None));
    assert_eq!(answers("+1"), (Some(1), Some(1), Some(1.0), None));
    assert_eq!(answers("1_000"), (Some(1000), Some(1000), None, None));
    assert_eq!(answers("True"), (None, None, None, Some(true)));
    assert_eq!(answers("no"), (None, None, None, Some(false)));
    assert_eq!(answers("!!int \"5\""), (Some(5), Some(5), None, None));
    assert_eq!(answers("!override 0"), (Some(0), Some(0), Some(0.0), None));
    assert_eq!(answers("!!str 1"), (None, None, None, None));
    assert_eq!(answers("inf"), (None, None, None, None));
    assert_eq!(answers(".inf").2, Some(f64::INFINITY));
    assert!(answers(".nan").2.is_some_and(f64::is_nan));
}

#[test]
fn is_null_agrees_with_the_recorded_typeless_null() {
    each_row(|text, name, options, row| {
        if name != "default" {
            return;
        }
        let rules = Rules::new(options);
        let (value, style, tag, _) = read_scalar(text);
        let scalar = Scalar {
            value: &value,
            style,
            tag: tag.as_deref(),
        };
        let null = *row.deserializer("typeless") == Answer::Read("Null".to_owned());
        assert_eq!(rules.is_null(&scalar), null, "{text:?}");
    });
    let rules = Rules::new(&Options::default());
    let is_null = |text: &str| {
        let (value, style, tag, _) = read_scalar(text);
        rules.is_null(&Scalar {
            value: &value,
            style,
            tag: tag.as_deref(),
        })
    };
    assert!(is_null("~"));
    assert!(is_null("null"));
    assert!(is_null(""));
    assert!(!is_null("'null'"));
    assert!(!is_null("!!str"));
    assert!(!is_null("no"));
}

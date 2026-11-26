//! The YAML 1.2 scalar rules the deserializer applies, for scalars that something other than the
//! deserializer holds, such as the nodes [`located::read`](crate::located::read) returns.
//!
//! [`Rules`] answers the questions the deserializer asks of a scalar's processed text, style and
//! tag: whether it is null, which boolean, integer or float it spells at a width the caller
//! names, and what [`deserialize_any`](serde_core::Deserializer::deserialize_any) would visit.
//! The deserializer calls the same functions, so an answer here is the answer
//! [`from_str_with_options`](crate::from_str_with_options) acts on.
//!
//! The rules read these [`Options`]: `strict_booleans`, `legacy_octal_numbers`,
//! `leading_zero_numbers_as_strings`, `underscored_numbers_as_strings`,
//! `ignore_binary_tag_for_string`, `non_finite_float_policy` (and the older
//! `reject_non_finite_typeless_float`), and `angle_conversions`. Property interpolation is a
//! step of the deserializer that comes before the rules and is not applied here.
//!
//! # Example
//!
//! ```
//! use serde_saphyr::granit_parser::ScalarStyle;
//! use serde_saphyr::scalars::{Inferred, Rules, Scalar};
//! use serde_saphyr::{Location, Options};
//!
//! let rules = Rules::new(&Options::default());
//! let scalar = Scalar { value: "0x10", style: ScalarStyle::Plain, tag: None };
//! assert_eq!(rules.infer(&scalar, Location::UNKNOWN)?, Inferred::Unsigned(16));
//! assert_eq!(rules.signed::<i8>(&scalar, Location::UNKNOWN)?, 16);
//!
//! let quoted = Scalar { value: "8", style: ScalarStyle::DoubleQuoted, tag: None };
//! assert_eq!(rules.infer(&quoted, Location::UNKNOWN)?, Inferred::String("8".into()));
//! # Ok::<(), serde_saphyr::Error>(())
//! ```

use std::borrow::Cow;
use std::fmt;

use granit_parser::ScalarStyle;

use super::base64::decode_base64_yaml;
use super::cfg::Cfg;
use super::options::NonFiniteFloatPolicy;
use super::tags::SfTag;
use super::{Error, Options};
use crate::location::Location;
use crate::parse_scalars::{
    leading_zero_decimal, parse_int_signed, parse_int_unsigned, parse_yaml11_bool,
    parse_yaml12_bool, parse_yaml12_float, scalar_is_null, try_parse_float_incl_overflow,
    yaml_trim,
};

/// A scalar as the parser delivers it, which is what the rules read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scalar<'a> {
    /// The processed text: escapes, folding and chomping applied.
    pub value: &'a str,
    /// How the scalar is written. Only a plain scalar can be null or a number.
    pub style: ScalarStyle,
    /// The tag in the spelling [`Tagged`](crate::Tagged) gives: `!override` for a local tag, the
    /// resolved handle and suffix otherwise, such as `tag:yaml.org,2002:str` for `!!str`.
    pub tag: Option<&'a str>,
}

impl Scalar<'_> {
    fn sf_tag(&self) -> SfTag {
        SfTag::from_spelling(self.tag)
    }
}

/// What [`deserialize_any`](serde_core::Deserializer::deserialize_any) visits for a scalar.
#[derive(Clone, Debug, PartialEq)]
pub enum Inferred<'a> {
    /// A null: `visit_unit`.
    Null,
    /// A boolean: `visit_bool`.
    Bool(bool),
    /// An integer whose text starts with a minus sign, plain or tagged `!!int`: `visit_i64`. An
    /// integer without a minus sign is [`Inferred::Unsigned`], tagged `!!int` or not.
    Signed(i64),
    /// An integer written without a minus sign: `visit_u64`.
    Unsigned(u64),
    /// A float: `visit_f64`.
    Float(f64),
    /// Text: `visit_borrowed_str` or `visit_string`. It holds the canonical `.nan`, `.inf` or
    /// `-.inf` for a non-finite float that [`NonFiniteFloatPolicy::AsString`] turns into text,
    /// and the decoded text of a `!!binary` scalar.
    String(Cow<'a, str>),
}

/// The scalar rules under one set of [`Options`].
#[derive(Clone, Copy)]
pub struct Rules {
    cfg: Cfg,
}

impl fmt::Debug for Rules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rules").finish_non_exhaustive()
    }
}

impl Rules {
    /// The rules that [`from_str_with_options`](crate::from_str_with_options) applies under
    /// `options`.
    #[must_use]
    pub fn new(options: &Options) -> Self {
        Self {
            cfg: Cfg::from_options(options),
        }
    }

    /// Whether the scalar is null: an empty, `~`, `null`, `Null` or `NULL` plain scalar, or one
    /// tagged `!!null`. A scalar with a string tag, such as `!!str`, or a quoted one is not.
    #[must_use]
    pub fn is_null(&self, scalar: &Scalar<'_>) -> bool {
        scalar_is_null(&scalar.sf_tag(), scalar.value, &scalar.style)
    }

    /// The boolean the scalar spells: `true` and `false` in the three YAML 1.2 Core spellings
    /// under `strict_booleans`, and the YAML 1.1 spellings, such as `yes` and `off`, otherwise.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidScalar`] or, under `strict_booleans`, [`Error::InvalidBooleanStrict`]
    /// at `location` when the text is not a boolean, and [`Error::InvalidScalar`] when the tag is
    /// not compatible with a boolean.
    pub fn boolean(&self, scalar: &Scalar<'_>, location: Location) -> Result<bool, Error> {
        boolean(&self.cfg, scalar.value, scalar.sf_tag(), location)
    }

    /// The integer the scalar spells, at the width `T` names: decimal, `0x`, `0o` and `0b`, with
    /// `_` between digits and an optional sign, and `0` before the digits under
    /// `legacy_octal_numbers`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidScalar`] at `location` when the text is not an integer, does not
    /// fit `T`, or carries a tag that is not compatible with an integer.
    pub fn signed<T: TryFrom<i128>>(
        &self,
        scalar: &Scalar<'_>,
        location: Location,
    ) -> Result<T, Error> {
        signed(
            &self.cfg,
            scalar.value,
            scalar.sf_tag(),
            std::any::type_name::<T>(),
            location,
        )
    }

    /// The unsigned integer the scalar spells, at the width `T` names, in the spellings of
    /// [`Self::signed`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidScalar`] at `location` when the text is not an unsigned integer,
    /// does not fit `T`, or carries a tag that is not compatible with an integer.
    pub fn unsigned<T: TryFrom<u128>>(
        &self,
        scalar: &Scalar<'_>,
        location: Location,
    ) -> Result<T, Error> {
        unsigned(
            &self.cfg,
            scalar.value,
            scalar.sf_tag(),
            std::any::type_name::<T>(),
            location,
        )
    }

    /// The 32-bit float the scalar spells: a decimal or exponent number, `.inf`, `-.inf` and
    /// `.nan`. Rust's own `inf` and `nan` are text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidScalar`] at `location` when the text is not a finite float or one
    /// of the YAML spellings of infinity and not-a-number, or when the tag is not compatible
    /// with a float.
    pub fn float32(&self, scalar: &Scalar<'_>, location: Location) -> Result<f32, Error> {
        float32(&self.cfg, scalar.value, scalar.sf_tag(), location)
    }

    /// The 64-bit float the scalar spells, in the spellings of [`Self::float32`].
    ///
    /// # Errors
    ///
    /// The errors of [`Self::float32`].
    pub fn float64(&self, scalar: &Scalar<'_>, location: Location) -> Result<f64, Error> {
        float64(&self.cfg, scalar.value, scalar.sf_tag(), location)
    }

    /// What `deserialize_any` visits for the scalar: an explicit tag picks null, boolean, integer
    /// or float; a plain scalar without one is null, then a boolean, an integer, a float, and
    /// text otherwise; any other scalar is text.
    ///
    /// # Errors
    ///
    /// Returns the errors `deserialize_any` returns at `location`: [`Error::InvalidScalar`] for
    /// text that does not fit its explicit tag, [`Error::NonFiniteFloat`] for a non-finite float
    /// under a policy that rejects it, [`Error::TaggedScalarCannotDeserializeIntoString`] for a
    /// tag that has no text form, and the errors of a `!!binary` scalar that is not base64 or
    /// not UTF-8.
    pub fn infer<'a>(
        &self,
        scalar: &Scalar<'a>,
        location: Location,
    ) -> Result<Inferred<'a>, Error> {
        infer(
            &self.cfg,
            Cow::Borrowed(scalar.value),
            scalar.value,
            scalar.sf_tag(),
            scalar.style,
            location,
        )
    }
}

/// Check an explicit tag against the tag a scalar target expects.
///
/// `tag` is the node's resolved tag, `expected` is the target's scalar tag, `ty` names the target
/// in diagnostics, and `location` identifies the node. Returns an error for incompatible tags or
/// disabled robotics conversions.
pub(super) fn validate_tag(
    cfg: &Cfg,
    tag: SfTag,
    expected: SfTag,
    ty: &'static str,
    location: Location,
) -> Result<(), Error> {
    let compatible = tag == expected
        || match tag {
            SfTag::None
            | SfTag::Other
            | SfTag::Include
            | SfTag::Merge
            | SfTag::Value
            | SfTag::NonSpecific => true,
            tag if tag.is_robotics() => {
                expected == SfTag::Float && cfg!(feature = "robotics") && cfg.angle_conversions
            }
            _ => false,
        };
    if compatible {
        Ok(())
    } else {
        Err(Error::InvalidScalar { ty, location })
    }
}

/// Parse a boolean literal according to `strict_booleans`.
pub(super) fn boolean(
    cfg: &Cfg,
    value: &str,
    tag: SfTag,
    location: Location,
) -> Result<bool, Error> {
    validate_tag(cfg, tag, SfTag::Bool, "boolean", location)?;
    if cfg.strict_booleans {
        parse_yaml12_bool(value).ok_or(Error::InvalidBooleanStrict { location })
    } else {
        parse_yaml11_bool(value).ok_or(Error::InvalidScalar {
            ty: "boolean",
            location,
        })
    }
}

/// Parse a signed integer of the type `ty` names.
pub(super) fn signed<T: TryFrom<i128>>(
    cfg: &Cfg,
    value: &str,
    tag: SfTag,
    ty: &'static str,
    location: Location,
) -> Result<T, Error> {
    validate_tag(cfg, tag, SfTag::Int, ty, location)?;
    parse_int_signed(value, ty, location, cfg.legacy_octal_numbers)
}

/// Parse an unsigned integer of the type `ty` names.
pub(super) fn unsigned<T: TryFrom<u128>>(
    cfg: &Cfg,
    value: &str,
    tag: SfTag,
    ty: &'static str,
    location: Location,
) -> Result<T, Error> {
    validate_tag(cfg, tag, SfTag::Int, ty, location)?;
    parse_int_unsigned(value, ty, location, cfg.legacy_octal_numbers)
}

/// Parse a 32-bit float (supports YAML 1.2 `+.inf`, `-.inf`, `.nan`).
pub(super) fn float32(
    cfg: &Cfg,
    value: &str,
    tag: SfTag,
    location: Location,
) -> Result<f32, Error> {
    validate_tag(cfg, tag, SfTag::Float, "floating point", location)?;
    parse_yaml12_float(value, location, tag, cfg.angle_conversions)
}

/// Parse a 64-bit float (supports YAML 1.2 `+.inf`, `-.inf`, `.nan`).
pub(super) fn float64(
    cfg: &Cfg,
    value: &str,
    tag: SfTag,
    location: Location,
) -> Result<f64, Error> {
    validate_tag(cfg, tag, SfTag::Float, "floating point", location)?;
    parse_yaml12_float(value, location, tag, cfg.angle_conversions)
}

/// Whether a scalar is read as text: it is not plain, or its tag is one that reads as text.
fn reads_as_text(tag: SfTag, style: ScalarStyle) -> bool {
    !matches!(style, ScalarStyle::Plain)
        || !tag.can_parse_into_string()
        || tag == SfTag::Binary
        || tag.forces_string()
}

/// Check that the tag allows the scalar to be read as text.
fn check_text_tag(cfg: &Cfg, tag: SfTag, location: Location) -> Result<(), Error> {
    if tag == SfTag::Binary && !cfg.ignore_binary_tag_for_string {
        return Ok(());
    }
    if !(tag.can_parse_into_string() || cfg.ignore_binary_tag_for_string && tag == SfTag::Binary) {
        return Err(Error::TaggedScalarCannotDeserializeIntoString { location });
    }
    Ok(())
}

/// Read a scalar as text, decoding `!!binary` into UTF-8 text unless the tag is ignored.
///
/// Errors if the tag is incompatible with strings or if the binary payload is not valid UTF-8.
pub(super) fn string<'a>(
    cfg: &Cfg,
    value: Cow<'a, str>,
    tag: SfTag,
    location: Location,
) -> Result<Cow<'a, str>, Error> {
    check_text_tag(cfg, tag, location)?;
    if tag == SfTag::Binary && !cfg.ignore_binary_tag_for_string {
        let data = decode_base64_yaml(&value).map_err(|err| err.with_location(location))?;
        let text = String::from_utf8(data).map_err(|_| Error::BinaryNotUtf8 { location })?;
        return Ok(Cow::Owned(text));
    }
    Ok(value)
}

/// What a caller must do with a scalar once [`precheck`] has looked at it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Plan {
    /// The scalar is an implicit null and nothing more is read from it.
    Null,
    /// Read the scalar and call [`infer`].
    Read,
}

/// The part of [`infer`] that decides, from the scalar as first seen, whether it is read at all:
/// whether an explicit type or text tag is compatible, and whether an untagged scalar is null.
///
/// The deserializer calls this before it consumes the scalar, so a scalar that is not read is
/// not interpolated a second time.
pub(super) fn precheck(
    cfg: &Cfg,
    value: &str,
    tag: SfTag,
    style: ScalarStyle,
    location: Location,
) -> Result<Plan, Error> {
    match tag {
        SfTag::Null | SfTag::Bool | SfTag::Int => return Ok(Plan::Read),
        SfTag::Float | SfTag::Time | SfTag::Degrees | SfTag::Radians => {
            validate_tag(cfg, tag, SfTag::Float, "floating point", location)?;
            return Ok(Plan::Read);
        }
        _ => {}
    }
    // String tags (!!str and !) preserve null-like text (null, ~, empty).
    if scalar_is_null(&tag, value, &style) {
        return Ok(Plan::Null);
    }
    if reads_as_text(tag, style) {
        check_text_tag(cfg, tag, location)?;
    }
    Ok(Plan::Read)
}

/// Deliver a float to a typeless reader, applying the configured non-finite policy.
fn typeless_float<'a>(
    cfg: &Cfg,
    value: f64,
    raw: &str,
    location: Location,
) -> Result<Inferred<'a>, Error> {
    if value.is_finite() {
        return Ok(Inferred::Float(value));
    }
    match cfg.non_finite_float_policy {
        NonFiniteFloatPolicy::PassThrough => Ok(Inferred::Float(value)),
        // Cfg resolves the compatibility default; an unconfigured policy otherwise
        // has the same behavior as Reject.
        NonFiniteFloatPolicy::Default | NonFiniteFloatPolicy::Reject => {
            Err(Error::NonFiniteFloat {
                value: raw.to_owned(),
                location,
            })
        }
        NonFiniteFloatPolicy::AsString => {
            let canonical = if value.is_nan() {
                ".nan"
            } else if value.is_sign_negative() {
                "-.inf"
            } else {
                ".inf"
            };
            Ok(Inferred::String(Cow::Owned(canonical.to_owned())))
        }
    }
}

/// What `deserialize_any` visits for a scalar whose processed value is `value`.
///
/// `raw` is the text as written, which a non-finite float error reports when interpolation has
/// changed `value`.
///
/// Explicit scalar types take precedence over implicit null and string inference, including for
/// quoted and block scalars buffered by Serde's flatten support. Plain, untagged scalars are
/// inferred (null-like, bool, int, float) before falling back to string. Quoted scalars without a
/// core type tag remain strings.
pub(super) fn infer<'a>(
    cfg: &Cfg,
    value: Cow<'a, str>,
    raw: &str,
    tag: SfTag,
    style: ScalarStyle,
    location: Location,
) -> Result<Inferred<'a>, Error> {
    if precheck(cfg, &value, tag, style, location)? == Plan::Null {
        return Ok(Inferred::Null);
    }
    match tag {
        SfTag::Null => return Ok(Inferred::Null),
        SfTag::Bool => return boolean(cfg, &value, tag, location).map(Inferred::Bool),
        SfTag::Int if yaml_trim(&value).starts_with('-') => {
            return signed(cfg, &value, tag, "i64", location).map(Inferred::Signed);
        }
        SfTag::Int => return unsigned(cfg, &value, tag, "u64", location).map(Inferred::Unsigned),
        SfTag::Float | SfTag::Time | SfTag::Degrees | SfTag::Radians => {
            let parsed =
                try_parse_float_incl_overflow(&value, location, tag, cfg.angle_conversions).ok_or(
                    Error::InvalidScalar {
                        ty: "floating point",
                        location,
                    },
                )?;
            return typeless_float(cfg, parsed, raw, location);
        }
        _ => {}
    }
    if reads_as_text(tag, style) {
        return string(cfg, value, tag, location).map(Inferred::String);
    }

    // Try booleans.
    if cfg.strict_booleans {
        if let Some(b) = parse_yaml12_bool(&value) {
            return Ok(Inferred::Bool(b));
        }
    } else if let Some(b) = parse_yaml11_bool(&value) {
        return Ok(Inferred::Bool(b));
    }

    let t = yaml_trim(&value);
    // Preserve selected spellings before either numeric parser can consume them. Explicit
    // numeric tags have already dispatched above.
    if (cfg.leading_zero_numbers_as_strings && leading_zero_decimal(t))
        || (cfg.underscored_numbers_as_strings && t.contains('_'))
    {
        return Ok(Inferred::String(value));
    }

    // Try integers: signed if leading '-', else unsigned, using 64-bit values.
    if t.starts_with('-') {
        if (!leading_zero_decimal(t) || cfg.legacy_octal_numbers)
            && let Ok(v) = parse_int_signed::<i64>(t, "i64", location, cfg.legacy_octal_numbers)
        {
            return Ok(Inferred::Signed(v));
        }
    } else if let Ok(v) = parse_int_unsigned::<u64>(t, "u64", location, cfg.legacy_octal_numbers) {
        return Ok(Inferred::Unsigned(v));
    }

    // Try float per YAML 1.2 forms, treating an f64-overflowing literal (e.g. `1e999`) as
    // non-finite too, so both shapes are handled consistently.
    if let Some(v) = try_parse_float_incl_overflow(&value, location, tag, cfg.angle_conversions) {
        return typeless_float(cfg, v, raw, location);
    }

    // Fallback: treat as string as-is, preserving zero-copy borrowing.
    Ok(Inferred::String(value))
}

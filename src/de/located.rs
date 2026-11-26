//! Read one YAML document as located nodes.
//!
//! [`read`] runs the pipeline [`from_str_with_options`](crate::from_str_with_options) uses, so
//! the budget, the alias replay limits, the recursive alias refusal, the duplicate key policy and
//! the single document check apply to it unchanged. It stops short of Serde: it returns the
//! document as a tree of [`Node`] values that remember where each node was written.

use std::cell::RefCell;
use std::collections::HashMap;

use granit_parser::ScalarStyle;

use super::Error;
use super::api::StrSnippetContext;
use super::cfg::Cfg;
use super::events::{
    Ev, Events, NodeOrigin, ReplayEvents, attach_alias_locations_if_missing, eof_with_loc,
};
use super::key_nodes::{
    KeyFingerprint, capture_node_with_legacy_octal, is_merge_key, pending_entries_from_live_events,
};
use super::options::{DuplicateKeyPolicy, MergeKeyPolicy, Options};
use super::with_deserializer::{enforce_single_document_and_finish, run_with_document_scope};
use crate::live_events::LiveEvents;
use crate::location::{Location, Locations};
use crate::span::Span;

/// A scalar node.
///
/// The text is the processed value: escapes, folding and chomping are applied, as
/// [`from_str`](crate::from_str) gives it to a `String`. Property interpolation is not applied.
///
/// A missing value, such as `b:` in `a: 1\nb:\n`, reads as the plain scalar `~`, as the parser
/// gives it, with an empty span. The struct is non-exhaustive so that a field can join it later:
/// read it through its fields or its accessors.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScalarNode {
    /// Where the node is written and where an alias used it.
    pub locations: Locations,
    /// The tag in the spelling [`Tagged`](crate::Tagged) gives: `!override` for a local tag, the
    /// resolved handle and suffix otherwise, such as `tag:yaml.org,2002:str` for `!!str`.
    pub tag: Option<String>,
    /// The processed scalar text.
    pub value: String,
    /// How the scalar is written.
    pub style: ScalarStyle,
}

/// A sequence node.
///
/// The struct is non-exhaustive so that a field can join it later: read it through its fields or
/// its accessors.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SequenceNode {
    /// Where the node is written and where an alias used it.
    pub locations: Locations,
    /// The tag, in the spelling of [`ScalarNode::tag()`].
    pub tag: Option<String>,
    /// The items in source order.
    pub items: Vec<Node>,
}

/// A mapping node.
///
/// The struct is non-exhaustive so that a field can join it later: read it through its fields or
/// its accessors.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MappingNode {
    /// Where the node is written and where an alias used it.
    pub locations: Locations,
    /// The tag, in the spelling of [`ScalarNode::tag()`].
    pub tag: Option<String>,
    /// The (key, value) pairs in source order, after the duplicate key policy.
    pub entries: Vec<(Node, Node)>,
}

impl ScalarNode {
    /// Where the node is written and where an alias used it.
    #[must_use]
    pub fn locations(&self) -> Locations {
        self.locations
    }

    /// The tag in the spelling [`Tagged`](crate::Tagged) gives: `!override` for a local tag, the
    /// resolved handle and suffix otherwise, such as `tag:yaml.org,2002:str` for `!!str`.
    #[must_use]
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// The processed scalar text.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// How the scalar is written.
    #[must_use]
    pub fn style(&self) -> ScalarStyle {
        self.style
    }
}

impl SequenceNode {
    /// Where the node is written and where an alias used it.
    #[must_use]
    pub fn locations(&self) -> Locations {
        self.locations
    }

    /// The tag, in the spelling of [`ScalarNode::tag()`].
    #[must_use]
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// The items in source order.
    #[must_use]
    pub fn items(&self) -> &[Node] {
        &self.items
    }
}

impl MappingNode {
    /// Where the node is written and where an alias used it.
    #[must_use]
    pub fn locations(&self) -> Locations {
        self.locations
    }

    /// The tag, in the spelling of [`ScalarNode::tag()`].
    #[must_use]
    pub fn tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// The (key, value) pairs in source order, after the duplicate key policy.
    #[must_use]
    pub fn entries(&self) -> &[(Node, Node)] {
        &self.entries
    }
}

/// One YAML node with the locations it was written at.
///
/// In every variant, [`Locations::defined_location`] is where the node is written. Its span
/// covers the whole node: the quotes of a quoted scalar, the block indicator of a block scalar
/// and, for a collection, everything up to the end of its last child or its closing bracket. A
/// node never includes its tag or anchor. [`Locations::reference_location`] is the outermost
/// alias that replayed the node, and every node beneath a replayed node inherits it. For a node
/// no alias replayed it equals the defined location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// A scalar.
    Scalar(ScalarNode),
    /// A sequence.
    Sequence(SequenceNode),
    /// A mapping.
    Mapping(MappingNode),
}

impl Node {
    /// The locations of the node.
    #[must_use]
    pub fn locations(&self) -> Locations {
        match self {
            Node::Scalar(node) => node.locations,
            Node::Sequence(node) => node.locations,
            Node::Mapping(node) => node.locations,
        }
    }

    /// The tag of the node, in the spelling of [`ScalarNode::tag()`].
    #[must_use]
    pub fn tag(&self) -> Option<&str> {
        match self {
            Node::Scalar(node) => node.tag.as_deref(),
            Node::Sequence(node) => node.tag.as_deref(),
            Node::Mapping(node) => node.tag.as_deref(),
        }
    }
}

/// Read one YAML document as located nodes.
///
/// The document is read through the same pipeline as
/// [`from_str_with_options`](crate::from_str_with_options): the [`Budget`](crate::Budget), the
/// alias replay limits, the recursive alias refusal and the refusal of a second document apply
/// unchanged, and an input the deserializer refuses is refused with the same [`Error`].
///
/// The reader makes the structural checks only: syntax, the budget, the alias replay limits, the
/// recursive alias refusal, duplicate keys, merge keys and the count of documents. It does not
/// check the content of a scalar. A scalar such as `!!int abc`, a non-finite float under
/// [`NonFiniteFloatPolicy::Reject`](crate::NonFiniteFloatPolicy::Reject), a `!!binary` scalar that
/// is not Base64, and the interpolation of properties are all read without complaint:
/// [`scalars::Rules`](crate::scalars::Rules) answers for them.
///
/// An empty input is a plain scalar with an empty value and unknown locations. A missing value,
/// such as `b:`, is the plain scalar `~` with an empty span, as the parser gives it. A node read
/// from an included file keeps the parser's span in that file, and [`Location::source_id`] tells it
/// from a node of the input. A span in the input never crosses into another source: a collection
/// that ends in an included node ends at the include.
///
/// Duplicate keys follow [`DuplicateKeyPolicy`] with the identity the deserializer uses, so
/// `0x10` and `16` are the same key: `Error` refuses the second key, `FirstWins` keeps the first
/// pair, `LastWins` keeps the last pair at its own position and `PassThrough` keeps every pair.
///
/// Merge keys follow [`MergeKeyPolicy`]. `Error` refuses a `<<` key as the deserializer does.
/// `Merge` keeps `<<` as an ordinary entry that the duplicate key policy leaves out, as the
/// deserializer does, and checks its value as the deserializer's merge does: it must be null, a
/// mapping or a sequence of mappings. `AsOrdinary` keeps `<<` as an ordinary entry. This reader
/// never expands a merge.
///
/// # Errors
///
/// Returns the [`Error`] that [`from_str_with_options`](crate::from_str_with_options) returns
/// for the same text: a syntax error, a breached budget or alias replay limit, a recursive alias,
/// a refused duplicate or merge key, a merge value that is not null, a mapping or a sequence of
/// mappings, or a second document.
///
/// # Example
///
/// ```
/// use serde_saphyr::located::{self, Node};
///
/// let node = located::read("name: &n demo\nalias: *n\n", serde_saphyr::Options::default())?;
/// let Node::Mapping(mapping) = node else { unreachable!() };
/// let (_, value) = &mapping.entries[1];
/// // The value is written at line 1 and used at line 2.
/// assert_eq!(value.locations().defined_location.line(), 1);
/// assert_eq!(value.locations().reference_location.line(), 2);
/// # Ok::<(), serde_saphyr::Error>(())
/// ```
pub fn read(input: &str, options: Options) -> Result<Node, Error> {
    let with_snippet = options.with_snippet;
    let crop_radius = options.crop_radius;
    let cfg = located_cfg(Cfg::from_options(&options));
    let mut src = LiveEvents::from_str(input, options);
    let known = RefCell::default();
    let input = Input {
        text: input,
        source_id: src.input_source_id(),
        known: &known,
    };
    let snippet_ctx = StrSnippetContext::new(input.text, with_snippet, crop_radius);
    let wrap_err = |e, src: &LiveEvents<'_>| snippet_ctx.attach_snippet(e, src);

    let node = run_with_document_scope(
        &mut src,
        |src| read_node(src, cfg, input),
        wrap_err,
        |_| true,
    )?;
    enforce_single_document_and_finish(
        &mut src,
        "use from_str_multiple or from_str_multiple_with_options",
        wrap_err,
    )?;
    Ok(node)
}

/// The deserializer's policies, minus the one that only makes sense for a typed target: tags on
/// keys are kept.
fn located_cfg(cfg: Cfg) -> Cfg {
    Cfg {
        reject_tagged_field_names: false,
        ..cfg
    }
}

/// The text that was read and the id the parser gives locations in it. With `!include`, a node
/// read from another file carries another id, and its span is not a span of the text.
#[derive(Clone, Copy)]
struct Input<'a> {
    text: &'a str,
    source_id: u32,
    known: &'a RefCell<Known>,
}

/// What the reader has seen so far that a later node needs.
#[derive(Default)]
struct Known {
    /// The span of each collection read where it is written, by source and start byte. A node an
    /// alias replays reports only the start of the collection it replays, and its end is the end
    /// of the collection that was read in place.
    collections: HashMap<(u32, u64), Span>,
    /// How each included source was included, by the id of the included source. The parser
    /// forgets a source once it has read it, and an alias can replay its nodes later.
    includes: HashMap<u32, Include>,
}

/// Where a source is included.
struct Include {
    /// The id of the source that holds the include.
    parent: u32,
    /// Where the include is written in that source.
    location: Location,
}

/// Remember how the source of a node was included, while the parser still knows.
fn note_includes(ev: &dyn Events<'_>, source_id: u32, input: Input<'_>) {
    if source_id == input.source_id || source_id == 0 {
        return;
    }
    let mut known = input.known.borrow_mut();
    if known.includes.contains_key(&source_id) {
        return;
    }
    for link in ev.include_links(source_id) {
        if link.source_id != input.source_id {
            known.includes.entry(link.source_id).or_insert(Include {
                parent: link.parent_source_id.unwrap_or(input.source_id),
                location: link.location,
            });
        }
    }
}

/// Where the include that brought a node of the source `from` into the source `parent` is
/// written in `parent`.
fn include_site(known: &Known, mut from: u32, parent: u32) -> Option<Location> {
    for _ in 0..=known.includes.len() {
        let include = known.includes.get(&from)?;
        if include.parent == parent {
            return Some(include.location);
        }
        from = include.parent;
    }
    None
}

/// Where a node's text ends, in characters and, when the parser reported them, in bytes.
#[derive(Clone, Copy)]
struct End {
    chars: u64,
    bytes: Option<u64>,
}

fn end_of(span: Span) -> Option<End> {
    if span == Span::UNKNOWN {
        return None;
    }
    Some(End {
        chars: span.offset().saturating_add(span.len()),
        bytes: span
            .byte_offset()
            .zip(span.byte_len())
            .map(|(offset, len)| offset.saturating_add(len)),
    })
}

/// The span that starts where `start` does and stops at `end`.
fn span_to(start: Span, end: End) -> Span {
    let span = Span::new(start.offset(), end.chars.saturating_sub(start.offset()));
    match (start.byte_offset(), end.bytes) {
        (Some(offset), Some(end)) => span.with_byte_info(offset, end.saturating_sub(offset)),
        _ => span,
    }
}

/// The end of a child as its parent's text includes it.
///
/// Beneath a replayed parent every child is replayed too, so its definition site is the text. A
/// child an alias replayed in a parent that was written in place ends at the alias. A child that
/// an include brought in from another source ends at the include, in the source of the parent. A
/// value the parser invented for a missing one has no text of its own and sits on the colon
/// before it.
fn written_end(
    node: &Node,
    parent_replayed: bool,
    parent_source: u32,
    input: Input<'_>,
) -> Option<End> {
    let locations = node.locations();
    let location = if parent_replayed {
        locations.defined_location
    } else {
        locations.reference_location
    };
    let span = location.span();
    let end = end_of(span)?;
    if location.source_id() != parent_source {
        let known = input.known.borrow();
        let site = include_site(&known, location.source_id(), parent_source)?;
        return end_of(site.span());
    }
    if matches!(node, Node::Scalar(_))
        && span.is_empty()
        && location.source_id() == input.source_id
        && usize::try_from(span.byte_offset()?)
            .ok()
            .and_then(|offset| input.text.as_bytes().get(offset))
            == Some(&b':')
    {
        return Some(End {
            chars: end.chars.saturating_add(1),
            bytes: end.bytes.map(|bytes| bytes.saturating_add(1)),
        });
    }
    Some(end)
}

/// The block header that starts a block scalar whose content starts at `content`: its byte
/// range and the byte offset of the line it is on.
fn block_header(input: &str, content: usize) -> Option<(usize, usize, usize)> {
    let mut end = input.get(..content)?.rfind('\n')?;
    loop {
        let line_start = input[..end].rfind('\n').map_or(0, |index| index + 1);
        let line = input[line_start..end]
            .strip_suffix('\r')
            .unwrap_or(&input[line_start..end]);
        if !line.trim().is_empty() {
            let (start, stop) = header_in_line(line)?;
            return Some((line_start + start, line_start + stop, line_start));
        }
        end = line_start.checked_sub(1)?;
    }
}

fn is_block_header(token: &str) -> bool {
    let mut chars = token.chars();
    matches!(chars.next(), Some('|' | '>'))
        && token.len() <= 3
        && chars.all(|c| matches!(c, '+' | '-' | '0'..='9'))
}

/// The block header at the end of a line, before any comment. A comment starts at a `#` after a
/// space, so the first one that follows a header is the comment of the line and the header is
/// not a word inside it. A line with no such comment ends in its header.
fn header_in_line(line: &str) -> Option<(usize, usize)> {
    let header_at_end = |prefix: &str| {
        let prefix = prefix.trim_end_matches([' ', '\t']);
        let start = prefix.rfind([' ', '\t']).map_or(0, |index| index + 1);
        is_block_header(&prefix[start..]).then_some((start, prefix.len()))
    };
    line.char_indices()
        .filter(|&(index, c)| {
            c == '#' && (index == 0 || matches!(line.as_bytes()[index - 1], b' ' | b'\t'))
        })
        .find_map(|(index, _)| header_at_end(&line[..index]))
        .or_else(|| header_at_end(line))
}

/// Widen the location of a block scalar to start at its header and stop at its last non-blank
/// character. The parser reports only the lines of content, up to the next token.
fn cover_block_scalar(input: &str, location: Location) -> Location {
    let span = location.span();
    let (Some(offset), Some(len)) = (span.byte_offset(), span.byte_len()) else {
        return location;
    };
    let (Ok(start), Ok(len)) = (usize::try_from(offset), usize::try_from(len)) else {
        return location;
    };
    let Some(text) = input.get(start..start.saturating_add(len)) else {
        return location;
    };
    let Some((header, header_end, line_start)) = block_header(input, start) else {
        return location;
    };
    let content = text.trim_end_matches([' ', '\t', '\r', '\n']);
    let stop = if content.is_empty() {
        header_end
    } else {
        start + content.len()
    };
    let before = &input[header..start];
    let chars_before = before.chars().count() as u64;
    let lines_before = before.matches('\n').count() as u64;
    let start_chars = span.offset().saturating_sub(chars_before);
    let stop_chars = if stop >= start {
        span.offset() + input[start..stop].chars().count() as u64
    } else {
        span.offset()
            .saturating_sub(input[stop..start].chars().count() as u64)
    };
    // The parser does not count a byte order mark in a column.
    let before_header = &input[line_start..header];
    let before_header = if line_start == 0 {
        before_header
            .strip_prefix('\u{feff}')
            .unwrap_or(before_header)
    } else {
        before_header
    };
    let (Ok(line), Ok(column)) = (
        u32::try_from(location.line().saturating_sub(lines_before)),
        u32::try_from(before_header.chars().count() + 1),
    ) else {
        return location;
    };
    Location {
        line,
        column,
        span: Span::new(start_chars, stop_chars.saturating_sub(start_chars))
            .with_byte_info(header as u64, (stop - header) as u64),
        ..location
    }
}

/// Read an item or a value, and report an error beneath an alias where the alias used it, as the
/// deserializer does.
fn read_child<'de>(ev: &mut dyn Events<'de>, cfg: Cfg, input: Input<'_>) -> Result<Node, Error> {
    let defined_location = match ev.peek()? {
        Some(event) => event.location(),
        None => ev.last_location(),
    };
    let alias_reference_location = ev.alias_reference_location();
    read_node(ev, cfg, input).map_err(|error| {
        attach_alias_locations_if_missing(error, alias_reference_location, defined_location)
    })
}

fn read_node<'de>(ev: &mut dyn Events<'de>, cfg: Cfg, input: Input<'_>) -> Result<Node, Error> {
    enum Start {
        Scalar,
        Sequence,
        Mapping,
    }
    let (start, location) = match ev.peek()? {
        Some(Ev::Scalar { location, .. }) => (Start::Scalar, *location),
        Some(Ev::SeqStart { location, .. }) => (Start::Sequence, *location),
        Some(Ev::MapStart { location, .. }) => (Start::Mapping, *location),
        Some(Ev::RecursiveAlias { location, .. }) => {
            return Err(Error::RecursiveReferencesRequireWeakTypes {
                location: *location,
            });
        }
        Some(Ev::SeqEnd { location }) => {
            return Err(Error::UnexpectedSequenceEnd {
                location: *location,
            });
        }
        Some(Ev::MapEnd { location }) => {
            return Err(Error::UnexpectedMappingEnd {
                location: *location,
            });
        }
        Some(Ev::Taken { location }) => {
            return Err(Error::unexpected("consumed event").with_location(*location));
        }
        None => return Err(eof_with_loc(ev)),
    };
    note_includes(ev, location.source_id(), input);
    let alias = match ev.node_origin() {
        NodeOrigin::Alias(location) => Some(location),
        NodeOrigin::Direct | NodeOrigin::Merge(_) => None,
    };
    let replayed = alias.is_some();
    let child_cfg = match start {
        Start::Scalar => cfg,
        Start::Sequence | Start::Mapping => cfg.enter_container(location)?,
    };
    let Some(event) = ev.next()? else {
        return Err(eof_with_loc(ev));
    };
    let locate = |defined: Location| Locations {
        reference_location: alias.unwrap_or(defined),
        defined_location: defined,
    };

    match event {
        Ev::Scalar {
            value,
            raw_tag,
            style,
            location,
            ..
        } => {
            let defined = if matches!(style, ScalarStyle::Literal | ScalarStyle::Folded)
                && location.source_id() == input.source_id
            {
                cover_block_scalar(input.text, location)
            } else {
                location
            };
            Ok(Node::Scalar(ScalarNode {
                locations: locate(defined),
                tag: raw_tag.as_deref().map(str::to_owned),
                value: value.into_owned(),
                style,
            }))
        }
        Ev::SeqStart {
            raw_tag, location, ..
        } => {
            let mut items = Vec::new();
            let end_location = loop {
                match ev.peek()? {
                    Some(Ev::SeqEnd { location }) => {
                        let location = *location;
                        let _ = ev.next()?;
                        break location;
                    }
                    Some(_) => items.push(read_child(ev, child_cfg, input)?),
                    None => return Err(eof_with_loc(ev)),
                }
            };
            let last = items
                .last()
                .and_then(|item| written_end(item, replayed, location.source_id(), input));
            Ok(Node::Sequence(SequenceNode {
                locations: locate(cover_collection(
                    input,
                    replayed,
                    location,
                    end_location,
                    last,
                )),
                tag: raw_tag.as_deref().map(str::to_owned),
                items,
            }))
        }
        Ev::MapStart {
            raw_tag, location, ..
        } => {
            let mut entries: Vec<Option<(Node, Node)>> = Vec::new();
            let mut seen: HashMap<KeyFingerprint<'de>, usize> = HashMap::new();
            let mut last = None;
            let end_location = loop {
                match ev.peek()? {
                    Some(Ev::MapEnd { location }) => {
                        let location = *location;
                        let _ = ev.next()?;
                        break location;
                    }
                    Some(_) => {}
                    None => return Err(eof_with_loc(ev)),
                }
                let mut key = capture_node_with_legacy_octal(ev, child_cfg.legacy_octal_numbers)?;
                let merge = is_merge_key(&key);
                if merge && child_cfg.merge_keys == MergeKeyPolicy::Error {
                    return Err(Error::MergeKeyNotAllowed {
                        location: key.location(),
                    });
                }
                let merge = merge && child_cfg.merge_keys == MergeKeyPolicy::Merge;
                let mut keep = true;
                if !merge && !matches!(child_cfg.dup_policy, DuplicateKeyPolicy::PassThrough) {
                    let fingerprint = key.take_fingerprint();
                    if let Some(&earlier) = seen.get(&fingerprint) {
                        match child_cfg.dup_policy {
                            DuplicateKeyPolicy::Error => {
                                return Err(Error::DuplicateMappingKey {
                                    key: key.stringy_scalar_value().map(ToOwned::to_owned),
                                    location: key.location(),
                                });
                            }
                            DuplicateKeyPolicy::FirstWins => keep = false,
                            DuplicateKeyPolicy::LastWins => entries[earlier] = None,
                            DuplicateKeyPolicy::PassThrough => {}
                        }
                    }
                    if keep {
                        seen.insert(fingerprint, entries.len());
                    }
                }
                let key_node = if keep {
                    let mut replay = ReplayEvents::new(
                        key.take_events(),
                        #[cfg(feature = "properties")]
                        ev.property_interpolation().for_key(),
                    );
                    Some(read_node(&mut replay, child_cfg, input)?)
                } else {
                    None
                };
                let value = if merge {
                    read_merge_value(ev, child_cfg, input)?
                } else {
                    read_child(ev, child_cfg, input)?
                };
                last = written_end(&value, replayed, location.source_id(), input);
                if let Some(key_node) = key_node {
                    entries.push(Some((key_node, value)));
                }
            };
            Ok(Node::Mapping(MappingNode {
                locations: locate(cover_collection(
                    input,
                    replayed,
                    location,
                    end_location,
                    last,
                )),
                tag: raw_tag.as_deref().map(str::to_owned),
                entries: entries.into_iter().flatten().collect(),
            }))
        }
        Ev::RecursiveAlias { location, .. } => {
            Err(Error::RecursiveReferencesRequireWeakTypes { location })
        }
        Ev::SeqEnd { location } => Err(Error::UnexpectedSequenceEnd { location }),
        Ev::MapEnd { location } => Err(Error::UnexpectedMappingEnd { location }),
        Ev::Taken { location } => Err(Error::unexpected("consumed event").with_location(location)),
    }
}

/// The location of a collection that starts at `start` and whose closing event is at `end`.
///
/// A flow collection ends with its bracket, which the parser reports with a length. A block
/// collection has no closing text: the parser puts its end at the next token, so it ends where its
/// last child does. A collection that an alias replays is the collection read where it is
/// written, whose last child may itself be an alias that the replay no longer shows.
fn cover_collection(
    input: Input<'_>,
    replayed: bool,
    start: Location,
    end: Location,
    last_child: Option<End>,
) -> Location {
    let at = start
        .span()
        .byte_offset()
        .map(|offset| (start.source_id(), offset));
    if replayed
        && let Some(span) = at.and_then(|at| input.known.borrow().collections.get(&at).copied())
    {
        return Location { span, ..start };
    }
    let closing = Some(end.span())
        .filter(|span| !span.is_empty())
        .and_then(end_of);
    let Some(end) = closing.or(last_child) else {
        return start;
    };
    if start.span() == Span::UNKNOWN {
        return start;
    }
    let covered = Location {
        span: span_to(start.span(), end),
        ..start
    };
    if let Some(at) = at
        && !replayed
    {
        input
            .known
            .borrow_mut()
            .collections
            .insert(at, covered.span());
    }
    covered
}

/// Read the value of a merge key under [`MergeKeyPolicy::Merge`].
///
/// The tree keeps the merge as it is written, but the deserializer would expand it, so the
/// value is first checked by the deserializer's own expansion: it is refused as the
/// deserializer refuses it unless it is null, a mapping or a sequence of mappings.
fn read_merge_value<'de>(
    ev: &mut dyn Events<'de>,
    cfg: Cfg,
    input: Input<'_>,
) -> Result<Node, Error> {
    let _ = ev.peek()?;
    let reference = ev.reference_location();
    #[cfg(feature = "properties")]
    let properties = ev.property_interpolation().clone();
    let mut value = capture_node_with_legacy_octal(ev, cfg.legacy_octal_numbers)?;
    let events = value.take_events();
    let mut check = ReplayEvents::with_reference(
        events.clone(),
        reference,
        #[cfg(feature = "properties")]
        properties.clone(),
    );
    pending_entries_from_live_events(&mut check, reference, cfg)?;
    let mut replay = ReplayEvents::new(
        events,
        #[cfg(feature = "properties")]
        properties,
    );
    read_node(&mut replay, cfg, input)
}

//! Tests for [serde_saphyr::located::read].

use serde_saphyr::located::{self, MappingNode, Node, ScalarNode, SequenceNode};
use serde_saphyr::{
    DuplicateKeyPolicy, Error, Location, MergeKeyPolicy, Options, from_str_with_options, options,
};

fn read(input: &str) -> Node {
    located::read(input, Options::default()).expect("the document reads")
}

fn read_with(input: &str, options: Options) -> Node {
    located::read(input, options).expect("the document reads")
}

fn mapping(node: &Node) -> &MappingNode {
    match node {
        Node::Mapping(mapping) => mapping,
        other => panic!("expected a mapping, got {other:?}"),
    }
}

fn sequence(node: &Node) -> &SequenceNode {
    match node {
        Node::Sequence(sequence) => sequence,
        other => panic!("expected a sequence, got {other:?}"),
    }
}

fn scalar(node: &Node) -> &ScalarNode {
    match node {
        Node::Scalar(scalar) => scalar,
        other => panic!("expected a scalar, got {other:?}"),
    }
}

/// The input text that a location's byte span covers.
fn covered(input: &str, location: Location) -> &str {
    let span = location.span();
    let offset = usize::try_from(span.byte_offset().expect("byte offset")).unwrap();
    let len = usize::try_from(span.byte_len().expect("byte length")).unwrap();
    &input[offset..offset + len]
}

fn defined_text<'a>(input: &'a str, node: &Node) -> &'a str {
    covered(input, node.locations().defined_location)
}

fn reference_text<'a>(input: &'a str, node: &Node) -> &'a str {
    covered(input, node.locations().reference_location)
}

/// The value of the first entry whose key is the scalar text `key`.
fn entry<'a>(node: &'a Node, key: &str) -> &'a Node {
    mapping(node)
        .entries
        .iter()
        .find(|(k, _)| matches!(k, Node::Scalar(s) if s.value == key))
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("no entry {key:?}"))
}

#[test]
fn plain_scalar_covers_its_text() {
    let input = "plain";
    let node = read(input);
    assert_eq!(scalar(&node).value, "plain");
    assert_eq!(defined_text(input, &node), "plain");
}

#[test]
fn single_quoted_scalar_covers_its_quotes() {
    let input = "'it''s'";
    let node = read(input);
    assert_eq!(scalar(&node).value, "it's");
    assert_eq!(defined_text(input, &node), "'it''s'");
}

#[test]
fn double_quoted_scalar_covers_its_quotes_and_applies_escapes() {
    let input = "\"tab\\there\"";
    let node = read(input);
    assert_eq!(scalar(&node).value, "tab\there");
    assert_eq!(defined_text(input, &node), "\"tab\\there\"");
}

#[test]
fn literal_scalar_covers_its_indicator_and_its_last_content_line() {
    let input = "a: |\n  l1\n  l2\n\nb: 1\n";
    let node = read(input);
    let value = entry(&node, "a");
    assert_eq!(scalar(value).value, "l1\nl2\n");
    assert_eq!(defined_text(input, value), "|\n  l1\n  l2");
    let location = value.locations().defined_location;
    assert_eq!((location.line(), location.column()), (1, 4));
}

#[test]
fn literal_scalar_at_the_root_covers_its_indicator() {
    let input = "|\n  lit\n  eral\n";
    let node = read(input);
    assert_eq!(scalar(&node).value, "lit\neral\n");
    assert_eq!(defined_text(input, &node), "|\n  lit\n  eral");
}

#[test]
fn folded_scalar_covers_its_indicator_and_applies_folding() {
    let input = "- >\n  fol\n  ded\n\n- next\n";
    let node = read(input);
    let item = &sequence(&node).items[0];
    assert_eq!(scalar(item).value, "fol ded\n");
    assert_eq!(defined_text(input, item), ">\n  fol\n  ded");
}

#[test]
fn block_scalar_with_chomping_and_a_comment_covers_the_whole_header() {
    let input = "- |2+ # keep\n   x\n\n- y\n";
    let node = read(input);
    let item = &sequence(&node).items[0];
    assert_eq!(scalar(item).value, " x\n\n");
    assert_eq!(defined_text(input, item), "|2+ # keep\n   x");
}

#[test]
fn block_scalar_after_multibyte_text_has_correct_character_and_line_positions() {
    let input = "é: ü\nk: |\n  ñ\n";
    let node = read(input);
    let value = entry(&node, "k");
    assert_eq!(defined_text(input, value), "|\n  ñ");
    let location = value.locations().defined_location;
    assert_eq!((location.line(), location.column()), (2, 4));
    let span = location.span();
    assert_eq!(span.offset(), "é: ü\nk: ".chars().count() as u64);
    assert_eq!(span.len(), "|\n  ñ".chars().count() as u64);
}

#[test]
fn block_sequence_covers_up_to_its_last_item() {
    let input = "- a\n- b\n\n";
    let node = read(input);
    assert_eq!(defined_text(input, &node), "- a\n- b");
    assert_eq!(defined_text(input, &sequence(&node).items[1]), "b");
}

#[test]
fn block_mapping_covers_up_to_its_last_value() {
    let input = "a: 1\nb: 2\n\n";
    let node = read(input);
    assert_eq!(defined_text(input, &node), "a: 1\nb: 2");
}

#[test]
fn nested_block_collections_end_at_their_own_last_child() {
    let input = "a:\n  - x\n  - y\n\nz: 1\n";
    let node = read(input);
    assert_eq!(defined_text(input, entry(&node, "a")), "- x\n  - y");
    assert_eq!(defined_text(input, &node), "a:\n  - x\n  - y\n\nz: 1");
}

#[test]
fn flow_collections_cover_their_brackets() {
    let input = "k: [\n  1,\n  2\n]\nm: {a: 1, b: 2} # c\n";
    let node = read(input);
    assert_eq!(defined_text(input, entry(&node, "k")), "[\n  1,\n  2\n]");
    assert_eq!(defined_text(input, entry(&node, "m")), "{a: 1, b: 2}");
    assert_eq!(
        defined_text(input, &sequence(entry(&node, "k")).items[0]),
        "1"
    );
}

#[test]
fn empty_collections_cover_their_brackets() {
    let input = "[[], {}]";
    let node = read(input);
    let items = &sequence(&node).items;
    assert_eq!(defined_text(input, &items[0]), "[]");
    assert_eq!(defined_text(input, &items[1]), "{}");
    assert_eq!(defined_text(input, &node), input);
}

#[test]
fn empty_value_has_an_empty_span_and_its_mapping_covers_the_colon() {
    let input = "a: 1\nb:\n";
    let node = read(input);
    let value = entry(&node, "b");
    assert_eq!(scalar(value).value, "~");
    let span = value.locations().defined_location.span();
    assert!(span.is_empty());
    assert_eq!(span.byte_offset(), Some(6));
    assert_eq!(defined_text(input, &node), "a: 1\nb:");
}

#[test]
fn empty_input_is_an_empty_plain_scalar() {
    let node = read("");
    let scalar = scalar(&node);
    assert_eq!(scalar.value, "");
    assert_eq!(scalar.tag, None);
}

#[test]
fn alias_sets_the_reference_location_and_children_inherit_it() {
    let input = "a: &x [1, 2]\nb: *x\n";
    let node = read(input);
    let original = entry(&node, "a");
    let replayed = entry(&node, "b");

    assert_eq!(
        original.locations().defined_location,
        original.locations().reference_location
    );
    assert_eq!(defined_text(input, replayed), "[1, 2]");
    assert_eq!(reference_text(input, replayed), "*x");
    assert_eq!(replayed.locations().reference_location.line(), 2);
    for item in &sequence(replayed).items {
        assert_eq!(reference_text(input, item), "*x");
        assert_eq!(item.locations().defined_location.line(), 1);
    }
    for item in &sequence(original).items {
        assert_eq!(
            item.locations().defined_location,
            item.locations().reference_location
        );
    }
}

#[test]
fn outermost_alias_wins_for_nested_replay() {
    let input = "a: &x [1]\nb: &y [*x]\nc: *y\n";
    let node = read(input);
    let inner_in_b = &sequence(entry(&node, "b")).items[0];
    let inner_in_c = &sequence(entry(&node, "c")).items[0];
    assert_eq!(reference_text(input, inner_in_b), "*x");
    assert_eq!(reference_text(input, inner_in_c), "*y");
    assert_eq!(defined_text(input, inner_in_c), "[1]");
}

#[test]
fn block_collection_ending_in_an_alias_ends_at_the_alias() {
    let input = "a: &x 1\nb:\n  - *x\n\nc: 2\n";
    let node = read(input);
    assert_eq!(defined_text(input, entry(&node, "b")), "- *x");
}

#[test]
fn replayed_block_collection_keeps_its_definition_span() {
    let input = "a: &x\n  - 1\n  - 2\n\nb: *x\n";
    let node = read(input);
    let replayed = entry(&node, "b");
    assert_eq!(defined_text(input, replayed), "- 1\n  - 2");
    assert_eq!(reference_text(input, replayed), "*x");
    assert_eq!(defined_text(input, &node), "a: &x\n  - 1\n  - 2\n\nb: *x");
}

#[test]
fn replayed_block_collection_ending_in_a_nested_alias_covers_its_last_item() {
    let input = "x: &x 1\ny: &y\n  - *x\nc: *y\n";
    let node = read(input);
    let (original, replayed) = (entry(&node, "y"), entry(&node, "c"));
    assert_eq!(defined_text(input, original), "- *x");
    assert_eq!(defined_text(input, replayed), "- *x");
    let span = replayed.locations().defined_location.span();
    assert_eq!((span.byte_offset(), span.byte_len()), (Some(16), Some(4)));
    assert_eq!(reference_text(input, replayed), "*y");
}

#[test]
fn replayed_block_collection_with_an_alias_beneath_an_anchor_covers_both_items() {
    let input = "y: &y\n  - &x a\n  - *x\nc: *y\n";
    let node = read(input);
    let (original, replayed) = (entry(&node, "y"), entry(&node, "c"));
    assert_eq!(defined_text(input, original), "- &x a\n  - *x");
    assert_eq!(defined_text(input, replayed), "- &x a\n  - *x");
}

#[test]
fn keys_beneath_a_replayed_mapping_keep_both_locations() {
    let input = "a: &x {k: 1}\nb: *x\n";
    let node = read(input);
    let replayed = entry(&node, "b");
    let (key, value) = &mapping(replayed).entries[0];
    for node in [key, value] {
        assert_eq!(reference_text(input, node), "*x");
        assert_eq!(node.locations().defined_location.line(), 1);
    }
    assert_eq!(defined_text(input, key), "k");
    assert_eq!(defined_text(input, value), "1");
}

#[test]
fn an_alias_used_as_a_key_keeps_both_locations() {
    let input = "a: &k key\n*k : v\n";
    let node = read(input);
    let (key, value) = &mapping(&node).entries[1];
    assert_eq!(scalar(key).value, "key");
    assert_eq!(defined_text(input, key), "key");
    assert_eq!(reference_text(input, key), "*k");
    assert_eq!(defined_text(input, value), "v");
    assert_eq!(defined_text(input, &node), "a: &k key\n*k : v");
}

#[test]
fn an_empty_last_sequence_item_is_the_plain_scalar_tilde() {
    let input = "- a\n-\n";
    let node = read(input);
    let last = &sequence(&node).items[1];
    assert_eq!(scalar(last).value, "~");
    assert!(last.locations().defined_location.span().is_empty());
    assert_eq!(defined_text(input, &node), "- a\n-");
}

#[test]
fn tags_use_the_spelling_tagged_gives() {
    let input = "local: !override 0\ncore: !!str 1\nverbatim: !<tag:example.com,2026:t> x\nplain: 2\nseq: !t [a]\nmap: !m {a: b}\n";
    let node = read(input);
    assert_eq!(entry(&node, "local").tag(), Some("!override"));
    assert_eq!(entry(&node, "core").tag(), Some("tag:yaml.org,2002:str"));
    assert_eq!(
        entry(&node, "verbatim").tag(),
        Some("tag:example.com,2026:t")
    );
    assert_eq!(entry(&node, "plain").tag(), None);
    assert_eq!(entry(&node, "seq").tag(), Some("!t"));
    assert_eq!(entry(&node, "map").tag(), Some("!m"));
    assert_eq!(scalar(entry(&node, "local")).value, "0");
}

#[test]
fn scalar_styles_are_reported() {
    use serde_saphyr::granit_parser::ScalarStyle::{
        DoubleQuoted, Folded, Literal, Plain, SingleQuoted,
    };
    let input = "- p\n- 's'\n- \"d\"\n- |\n  l\n- >\n  f\n";
    let node = read(input);
    let styles: Vec<_> = sequence(&node)
        .items
        .iter()
        .map(|item| scalar(item).style)
        .collect();
    assert_eq!(styles, [Plain, SingleQuoted, DoubleQuoted, Literal, Folded]);
}

#[test]
fn duplicate_key_policies() {
    let input = "a: 1\nb: 2\na: 3\n";
    let keys = |node: &Node| -> Vec<(String, String)> {
        mapping(node)
            .entries
            .iter()
            .map(|(k, v)| (scalar(k).value.clone(), scalar(v).value.clone()))
            .collect()
    };
    let pair = |k: &str, v: &str| (k.to_owned(), v.to_owned());

    let first = read_with(
        input,
        options! { duplicate_keys: DuplicateKeyPolicy::FirstWins },
    );
    assert_eq!(keys(&first), [pair("a", "1"), pair("b", "2")]);
    let last = read_with(
        input,
        options! { duplicate_keys: DuplicateKeyPolicy::LastWins },
    );
    assert_eq!(keys(&last), [pair("b", "2"), pair("a", "3")]);
    let both = read_with(
        input,
        options! { duplicate_keys: DuplicateKeyPolicy::PassThrough },
    );
    assert_eq!(
        keys(&both),
        [pair("a", "1"), pair("b", "2"), pair("a", "3")]
    );
    assert!(located::read(input, Options::default()).is_err());
}

#[test]
fn duplicate_keys_use_the_deserializer_identity() {
    let input = "0x10: a\n16: b\n";
    assert_same_refusal(input, Options::default());
    let last = read_with(
        input,
        options! { duplicate_keys: DuplicateKeyPolicy::LastWins },
    );
    let entries = &mapping(&last).entries;
    assert_eq!(entries.len(), 1);
    assert_eq!(scalar(&entries[0].0).value, "16");
}

#[test]
fn duplicate_keys_in_a_nested_mapping_follow_the_policy() {
    let input = "outer:\n  k: 1\n  k: 2\n";
    assert_same_refusal(input, Options::default());
    let node = read_with(
        input,
        options! { duplicate_keys: DuplicateKeyPolicy::FirstWins },
    );
    let inner = entry(&node, "outer");
    assert_eq!(mapping(inner).entries.len(), 1);
    assert_eq!(scalar(entry(inner, "k")).value, "1");
}

#[test]
fn merge_key_is_an_ordinary_entry_unless_refused() {
    let input = "base: &b {x: 1}\nderived:\n  <<: *b\n  y: 2\n";
    for policy in [MergeKeyPolicy::Merge, MergeKeyPolicy::AsOrdinary] {
        let node = read_with(input, options! { merge_keys: policy });
        let derived = entry(&node, "derived");
        let keys: Vec<_> = mapping(derived)
            .entries
            .iter()
            .map(|(k, _)| scalar(k).value.as_str())
            .collect();
        assert_eq!(keys, ["<<", "y"]);
        assert_eq!(reference_text(input, entry(derived, "<<")), "*b");
    }
    assert_same_refusal(input, options! { merge_keys: MergeKeyPolicy::Error });
}

#[test]
fn merge_keys_under_merge_stay_out_of_the_duplicate_key_check() {
    let input = "b: &b {x: 1}\nd:\n  <<: *b\n  <<: {y: 2}\n";
    let node = read(input);
    let keys: Vec<_> = mapping(entry(&node, "d"))
        .entries
        .iter()
        .map(|(k, _)| scalar(k).value.as_str())
        .collect();
    assert_eq!(keys, ["<<", "<<"]);
    from_str_with_options::<serde_json::Value>(input, Options::default())
        .expect("the deserializer accepts the same text");
    assert_same_refusal(input, options! { merge_keys: MergeKeyPolicy::AsOrdinary });
}

#[test]
fn a_merge_value_that_is_not_a_mapping_is_refused_as_the_deserializer_refuses_it() {
    assert_same_refusal("a:\n  <<: 5\n", Options::default());
    assert_same_refusal("b: &b {x: 1}\nd:\n  <<: [*b, 5]\n", Options::default());
}

#[test]
fn a_merge_value_that_is_null_or_a_sequence_of_mappings_is_read() {
    let input = "b: &b {x: 1}\nd:\n  <<: [*b, {y: 2}]\ne:\n  <<:\n";
    let node = read(input);
    let merged = &mapping(entry(&node, "d")).entries[0].1;
    assert_eq!(sequence(merged).items.len(), 2);
    assert_eq!(reference_text(input, &sequence(merged).items[0]), "*b");
    assert_eq!(scalar(&mapping(entry(&node, "e")).entries[0].1).value, "~");
}

/// Both readers refuse `input` with the same error.
fn assert_same_refusal(input: &str, options: Options) {
    let located: Error = located::read(input, options.clone()).expect_err("located::read refuses");
    let deserialized: Error = from_str_with_options::<serde_json::Value>(input, options)
        .expect_err("from_str_with_options refuses");
    assert_eq!(format!("{located:?}"), format!("{deserialized:?}"));
    assert_eq!(located.to_string(), deserialized.to_string());
    let (a, b) = (located.location(), deserialized.location());
    assert!(a.is_some() && a == b);
}

#[test]
fn refuses_what_the_deserializer_refuses() {
    assert_same_refusal("a: 1\na: 2\n", Options::default());
    assert_same_refusal(
        "a: &x 1\nb: *x\nc: *x\n",
        options! { budget: serde_saphyr::budget! { max_aliases: 1 } },
    );
    assert_same_refusal(
        "[1, 2, 3, 4]\n",
        options! { budget: serde_saphyr::budget! { max_events: 3 } },
    );
    assert_same_refusal(
        "[[[[1]]]]\n",
        options! { budget: serde_saphyr::budget! { max_depth: 3 } },
    );
    assert_same_refusal("a: &a [*a]\n", Options::default());
    assert_same_refusal("a: 1\n---\nb: 2\n", Options::default());
}

#[test]
fn refuses_a_nested_alias_bomb_before_building_the_tree() {
    let mut input = String::from("a0: &a0 [x]\n");
    for level in 1..9 {
        let aliases = vec![format!("*a{}", level - 1); 10].join(", ");
        input.push_str(&format!("a{level}: &a{level} [{aliases}]\n"));
    }
    assert_same_refusal(&input, Options::default());
    let uncapped = || {
        options! { budget: serde_saphyr::budget! {
            max_nodes: usize::MAX,
            max_events: usize::MAX,
            max_recorded_anchor_events: usize::MAX,
        } }
    };
    assert_same_refusal(&input, uncapped());
    let error = located::read(&input, uncapped()).unwrap_err();
    assert!(error.to_string().contains("replay"), "{error}");
}

#[test]
fn multi_document_input_is_refused_with_the_deserializer_hint() {
    let error = located::read("a: 1\n---\nb: 2\n", Options::default()).unwrap_err();
    assert!(error.to_string().contains("from_str_multiple"), "{error}");
}

#[test]
fn spans_are_relative_to_the_input_when_it_starts_with_a_bom() {
    let input = "\u{feff}a: 1\n";
    let node = read(input);
    assert_eq!(defined_text(input, entry(&node, "a")), "1");

    let input = "\u{feff}a: |\n  x\n";
    let node = read(input);
    let value = entry(&node, "a");
    assert_eq!(defined_text(input, value), "|\n  x");
    let location = value.locations().defined_location;
    assert_eq!((location.line(), location.column()), (1, 4));
}

#[test]
fn block_header_before_a_comment_that_mentions_another_header() {
    let input = "a: | # see >\n  x\n";
    let node = read(input);
    let value = entry(&node, "a");
    assert_eq!(scalar(value).value, "x\n");
    assert_eq!(defined_text(input, value), "| # see >\n  x");
    let location = value.locations().defined_location;
    assert_eq!((location.line(), location.column()), (1, 4));
}

/// The nodes of `node` and of everything beneath it.
fn descendants<'a>(node: &'a Node, found: &mut Vec<&'a Node>) {
    found.push(node);
    match node {
        Node::Scalar(_) => {}
        Node::Sequence(sequence) => {
            for item in &sequence.items {
                descendants(item, found);
            }
        }
        Node::Mapping(mapping) => {
            for (key, value) in &mapping.entries {
                descendants(key, found);
                descendants(value, found);
            }
        }
    }
}

#[cfg(feature = "include")]
#[test]
fn a_span_never_crosses_into_an_included_source() {
    use serde_saphyr::{IncludeRequest, IncludeResolveError, InputSource, ResolvedInclude};

    let input = "a: 1\nb: !include long.yaml\n";
    let included: String = (0..4)
        .map(|n| format!("key{n}: a value that is longer than the whole input\n"))
        .collect();
    assert!(included.len() > input.len());
    let options = Options::default().with_include_resolver(
        move |request: IncludeRequest| -> Result<ResolvedInclude, IncludeResolveError> {
            assert_eq!(request.spec, "long.yaml");
            Ok(ResolvedInclude::new(
                "long.yaml",
                "long.yaml",
                InputSource::from_string(included.clone()),
            ))
        },
    );
    let node = read_with(input, options);
    let root = node.locations().defined_location.source_id();
    let mut all = Vec::new();
    descendants(&node, &mut all);
    let mut from_the_input = 0;
    let mut from_the_include = 0;
    for node in all {
        let locations = node.locations();
        for location in [locations.defined_location, locations.reference_location] {
            if location.source_id() != root {
                from_the_include += 1;
                continue;
            }
            from_the_input += 1;
            let span = location.span();
            let end = span.byte_offset().unwrap() + span.byte_len().unwrap();
            assert!(
                end <= input.len() as u64,
                "{location:?} ends beyond the input"
            );
        }
    }
    assert!(from_the_input > 0 && from_the_include > 0);
    assert_eq!(defined_text(input, &node), "a: 1\nb: !include long.yaml");
}

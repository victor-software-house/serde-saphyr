# Changelog

## vsh-serde-saphyr 0.0.5

- `located::read` reads one YAML document into located nodes instead of a serde value: each scalar with its processed value, style and tag, each sequence with its items, and each mapping with its key and value nodes in order.
Every node carries where it is written and, for a node an alias replayed, where the outermost alias stands.
The reader runs the same pipeline as `from_str_with_options`, so the budget, the alias limits, the recursive-alias refusal, the duplicate and merge key policies and the single-document check apply unchanged.
- `scalars::Rules` makes the YAML 1.2 scalar rules public: whether a scalar is null, its bool, its integer at a width the caller names, its float, and what `deserialize_any` would visit.
The deserializer calls the same functions, so a value reads the same either way.

## vsh-serde-saphyr 0.0.4

### Fixed

- `Tagged<T>` gives `T` the node without its tag once it has captured the tag, so an untagged enum or another type that Serde buffers reads the plain value, such as `Tagged(Setting::List(["Acme"]), Some("!override"))` for `!override [Acme]`, instead of the one-key map `{override: [Acme]}`.
A tag on a node below the one `Tagged` reads still reaches the inner value as before.

## vsh-serde-saphyr 0.0.3

- The package carries a NOTICE file that names the fork and its maintainer, and LICENSE-MIT opens with copyright lines for the serde-saphyr contributors and for Victor Software House. The license terms stay MIT or Apache-2.0, as upstream.

## vsh-serde-saphyr 0.0.2

- Alias errors fill their deprecated `msg` field from the plain message path instead of Display, so deserializing YAML with aliases no longer runs the snippet renderer.

## vsh-serde-saphyr 0.0.1

- First release of the fork, based on serde-saphyr 2.1.1. `deserialize_str` recognizes the error that the `&str` visitor returns for an owned string by its variant instead of its Display text, so it no longer runs the snippet renderer.


# Changelog

## vsh-serde-saphyr 0.0.1

- First release of the fork, based on serde-saphyr 2.1.1. `deserialize_str` recognizes the error that the `&str` visitor returns for an owned string by its variant instead of its Display text, so it no longer runs the snippet renderer.


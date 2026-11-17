# Patched dependencies

These are the published crates.io sources, copied from Cargo's registry cache.
They retain their original package names and versions. The only manifest change
in each crate (`Cargo.toml` and `Cargo.toml.orig`) replaces the unmaintained
`paste` dependency with `pastey` 0.2.3, keeping the `paste` dependency alias so
the upstream Rust source stays unchanged. This removes the dependency rather
than suppressing its RustSec advisory.

| Crate | Upstream | License | Original `.crate` SHA-256 |
| --- | --- | --- | --- |
| frankenstein 0.50.0 | https://github.com/ayrat555/frankenstein | WTFPL (`LICENCE`) | `2ea21b2a2423971b2586380c5de0cc9543dd827dd84c867530f1882093159246` |
| tokenizers 0.22.2 | https://github.com/huggingface/tokenizers | Apache-2.0 (`LICENSE`) | `b238e22d44a15349529690fb07bd645cf58149a1b1e44d6cb5bd1641ff1a6223` |

Original archives are available at
`https://static.crates.io/crates/{name}/{name}-{version}.crate`.
License files, source, examples, tests, benches, and documentation are preserved.
Registry `.cargo*` bookkeeping and upstream `Cargo.lock` files are omitted; the
application's lockfile resolves these path dependencies.

The application selects these crates through `[patch.crates-io]`. Remove each
patch and its directory when an upstream release replaces `paste`; verify with
`cargo tree -i paste`, `cargo audit`, and the application tests before removal.

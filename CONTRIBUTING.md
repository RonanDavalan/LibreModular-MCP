# Contributing

A change to the Hub or to a bridge is accepted when the commands of the repository's continuous
integration pass on it. They are the ones in `.github/workflows/ci.yml`; run them from the root of
your clone before opening a pull request.

## What you need

- Rust 1.97 or later, with `rustfmt` and `clippy` (`rustup toolchain install 1.97 --profile minimal --component rustfmt --component clippy`).
- A C compiler and linker (on Debian, `sudo apt install build-essential`).
- Python 3, for the bridge tests and the bridge generator.

The tests need no GIMP, Krita, Blender, Inkscape or LibreOffice: they run the Hub against test
bridges. The tests that drive a real application are marked `#[ignore]` with their reason and run
on purpose, for example `cargo test --locked --test libreoffice_real -- --ignored --nocapture`.

## The checks

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo build --release --locked
python3 scripts/build_bridges.py --check
python3 -m unittest discover -s tests/bridges
```

The bridge files in `bridges/generated/` are generated from `bridges/_core.py` and one adapter per
application in `bridges/adapters/`. Edit the core or an adapter, never a generated file, then run
`python3 scripts/build_bridges.py` and commit the result; `--check` fails when the generated files
are out of date.

`CHANGELOG.md` is generated from the titles of the commits by `scripts/generate_changelog.py`: do
not edit it by hand. A commit title that starts with Add, Fix, Document or Test is filed under
Added, Fixed, Documentation or Tests; any other first word files it under Changed.

## Adding an application

A new application needs a module on the Hub side and a bridge on the application's side, with its
guide in `docs/modules/`. The existing modules in `src/` and bridges in `bridges/adapters/` show
the shape. Every claim in a guide comes from a run you made: give the version of the application,
the command that started it and what you did not try.

## Reporting a vulnerability

Do not open a public issue for a vulnerability. [SECURITY.md](SECURITY.md) says how to report it
privately, what is in scope and what to expect.

## Credit

Whoever contributes is credited in [CONTRIBUTORS.md](CONTRIBUTORS.md) with the role actually
played.

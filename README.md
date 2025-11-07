## Datum Tunnels

This repo is broken into 3 components, a CLI, GUI app, and shared-core library that the CLI & GUI draw on.


### Required Tools
* For all three crates: [`rust & cargo`](https://rust-lang.org/tools/install/)
* For UI: [`dioxus`](https://dioxuslabs.com/learn/0.6/getting_started/)
  * specifically, install `dx` with `cargo install dioxus-cli`
  * if you have [`binstall`](https://github.com/cargo-bins/cargo-binstall?tab=readme-ov-file#installation), you can skip compiling `dx` from source by running `cargo binstall dx`

### Running CLI commands:
to run without compiling, use `cargo run` in the `cli` directory:

```
cd cli
cargo run -- --help
```

### Running the UI:

to run the UI, make sure you have rust, cargo, and dioxus installed:

```
cd ui
dx serve
```

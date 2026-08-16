# ShadowShell

A modern Unix shell built from scratch in Rust.

## Status

Phase 5 (scripting layer): POSIX-style variables and expansions, `&&`/`||`/`;`,
control flow (`if`/`while`/`until`/`for`/`case`), functions, `export`/`unset`/
`return`/`shift`, and script-file execution. Interactive mode still provides
line editing, history, job control, pipelines/redirection, and the animated
prompt from earlier phases.

## Build

```
cargo build
```

## Run

Interactive:

```
cargo run
```

Script:

```
cargo run -- path/to/script.sh arg1 arg2
```

## Test

```
cargo test
```

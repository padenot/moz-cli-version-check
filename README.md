# moz-cli-version-check

Background update notices for Mozilla command-line tools. Checks crates.io for a
newer version and caches results in `~/.mozbuild/tool-versions.json`.

```sh
cargo add moz-cli-version-check
```

## Use in a CLI

```rust
use moz_cli_version_check::VersionChecker;

fn main() {
    let checker = VersionChecker::new(
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
    );
    checker.check_async();

    // Run the tool's work here.

    checker.print_warning();
}
```

Call `check_async()` near startup and `print_warning()` after the tool's output.
`print_warning()` waits up to 500 ms for a result; `print_warning_sync()` waits up
to six seconds. Network and cache errors produce no update notice.

When an update is available, stderr includes:

```text
Note: A newer version of your-tool is available (current: 0.1.0, latest: 0.2.0)
      Run: cargo binstall your-tool
```

For clap CLIs, use `Parser::try_parse()`. `Parser::parse()` exits directly on
`--help`, `--version`, or an argument error, bypassing later calls. In the error
branch, print clap's output, print the update notice, then exit:

```rust
match Cli::try_parse() {
    Ok(args) => {
        let result = run(args);
        checker.print_warning();
        result
    }
    Err(error) => {
        let _ = error.print();
        checker.print_warning();
        std::process::exit(error.exit_code());
    }
}
```

Here `Cli` and `run` are the consuming tool's parser and entry point.

## Configure

| Setting | Default / behavior |
| --- | --- |
| `MOZTOOLS_UPDATE_CHECK=0` | Disables checking; otherwise enabled |
| `MOZTOOLS_UPDATE_CHECK_INTERVAL_SECONDS` | Positive interval in seconds; default 86400 (24 hours) |
| HTTP timeout | Five seconds |
| Cache | `~/.mozbuild/tool-versions.json`, keyed by tool name |

Successful and failed remote attempts are throttled by the check interval.
The cache records `last_check` and the last known `latest` version for each tool.

Override the interval in code with
`VersionChecker::with_check_interval(name, version, Duration::from_secs(...))`.
Customize the notice with builder methods:

```rust
let checker = VersionChecker::new("your-tool", env!("CARGO_PKG_VERSION"))
    .with_update_command("your-tool update")
    .with_extra_notice("Update before continuing.");
```

## Development

```sh
cargo test --locked
cargo fmt -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
```

## Publishing

**Pushing a `vVERSION` tag publishes the crate to crates.io.**
The tag must match the version in `Cargo.toml`. GitHub write access is enough for
routine releases; no local crates.io token is needed.

See [Publishing](docs/PUBLISHING.md) for the release procedure, retries, and ownership.

## License

[MIT](LICENSE-MIT) OR [Apache-2.0](LICENSE-APACHE).

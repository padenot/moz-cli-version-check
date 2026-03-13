# moz-cli-version-check

A lightweight version checking library for Mozilla CLI tools.

## Features

- **Enabled by default**: Checks for updates unless `MOZTOOLS_UPDATE_CHECK=0` is set
- **Non-blocking**: Runs in background thread, never delays program startup
- **Cached**: Checks at most once per 24 hours per tool
- **Shared cache**: All tools share `~/.mozbuild/tool-versions.json`
- **Silent failures**: Network errors don't affect program operation
- **Thread-safe**: Safe for concurrent access

## Usage

Add the dependency:

```bash
cargo add moz-cli-version-check
```

In your `main.rs`:

```rust
use clap::Parser;

fn main() -> Result<()> {
    let version_checker = moz_cli_version_check::VersionChecker::new(
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
    );
    version_checker.check_async();

    match Cli::try_parse() {
        Ok(cli) => {
            let result = run(cli);
            version_checker.print_warning();
            result
        }
        Err(e) => {
            let _ = e.print();                    // print help/version/error text first
            version_checker.print_warning();      // print update notice last
            std::process::exit(e.exit_code());    // 0 for help/version, 2 for errors
        }
    }
}
```

**Why `try_parse()`?** clap's `Parser::parse()` calls `std::process::exit()` when
the user passes `--help` or `--version`, which terminates the process before
`print_warning()` can run. Using `try_parse()` returns a `Result` instead,
giving you a chance to print the update notice in both the success and error branches.
In the `Err` branch, `e.print()` outputs clap's help/version/error text first, then
the update notice appears after it.

## Usage from AI agents / skills

AI agent skills (e.g. Claude Code skills) that invoke tools using this library
should start with a preliminary step before doing any real work:

1. Run `<tool> --version` to verify the tool is installed and available.
2. Check stderr for an update notice. If one appears, update the tool
   (e.g. `cargo binstall <tool>`) before proceeding.

## How It Works

1. At program startup, unless `MOZTOOLS_UPDATE_CHECK=0` is set, spawn a background thread
2. The thread checks the cache file (`~/.mozbuild/tool-versions.json`)
3. If the cache is recent (< 24 hours), use cached data
4. Otherwise, query crates.io API: `https://crates.io/api/v1/crates/<name>`
5. Update the cache with the latest version info
6. At program exit, print a warning if a newer version is available

## Warning Format

When a newer version is available, users see on stderr:

```
Note: A newer version of socorro-cli is available (current: 0.1.0, latest: 0.2.0)
      Run: cargo binstall socorro-cli
```

## Cache Format

The cache file at `~/.mozbuild/tool-versions.json` contains:

```json
{
  "socorro-cli": {
    "last_check": 1234567890,
    "latest": "0.2.0"
  },
  "treeherder-cli": {
    "last_check": 1234567890,
    "latest": "0.1.0"
  }
}
```

## Testing

Version checking is enabled by default:

```bash
socorro-cli crash --help
```

To disable:

```bash
export MOZTOOLS_UPDATE_CHECK=0
socorro-cli crash --help
```

## Configuration

- **Cache location**: `~/.mozbuild/tool-versions.json`
- **Cache validity**: 24 hours
- **Network timeout**: 5 seconds
- **User-Agent**: `{tool-name}/version-check`

## Implementation Details

- Uses `reqwest` with blocking client for HTTP requests
- Uses `serde_json` for cache file serialization
- Thread-safe via `Arc<Mutex<Option<String>>>`
- Silently fails on any error (network, I/O, parsing)
- Never blocks program execution

## License

Licensed under either of:

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

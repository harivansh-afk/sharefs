# sharefs

File sharing from your own filesystem. Forked from [Dufs](https://github.com/sigoden/dufs).

```sh
nix develop
cargo build --release
./target/release/sharefs --bind 127.0.0.1 --port 39475 /srv/files
```

Configure accounts with `--auth` or the `auth` field in a config file. Without
auth rules, the served directory is public. Run `sharefs --help` for options.

Development: `cargo test --locked -- --test-threads=1`, `cargo clippy --all-targets -- -D warnings`,
and `cargo fmt --check`.

Persistent share links and the `share` CLI are being added. The current server
retains Dufs' browsing, transfer, editing and path-permission behavior.

[MIT](LICENSE).

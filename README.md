# sharefs

File sharing from your own filesystem. Built with Rust and Tokio, based on
[dufs](https://github.com/sigoden/dufs).

Browse files in the web UI or share them from the terminal:

```sh
share notes.md                 # read-only link, expires in 7 days
share --expires=2h notes.md
```

Links serve the current file contents. Renaming a file breaks its link. Manage
or revoke links from **Shares** in the web UI.

The daemon needs `--share-key-file`, `--public-url`, and `--share-state-dir`
for tracked links, plus `--control-socket` for the CLI (default client path:
`/run/sharefs/control.sock`). Keep the 32-byte random key and private state
directory outside the served folders. Preserve both across restarts.

Uploads, edits, and renames are opt-in. File deletion is disabled.
See `sharefs --help` for options.

To work on it:

```sh
nix develop
cargo run -- . --bind 127.0.0.1 --port 38471
cargo test --locked -- --test-threads=1
```

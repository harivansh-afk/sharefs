# sharefs

this is an attempt to handwrite an extremely memory efficient and performant file sharing platform

we use the tokio runtime for i/o 

```sh
nix develop
```

Plan

1. Keep share permission and expiry decisions in a small Sans-I/O core.
2. Use one request contract for the CLI and web Share controls.
3. Make saves conflict-aware; load directory details and viewers on demand.
4. Bound filesystem, database and transfer work so browsing stays responsive.
5. Measure browser, network and I/O time before changing runtimes or pinning threads.


Usage (Linux)

`share FILE` prints a read-only link that expires in seven days. Use
`share --expires=2h FILE` to choose a duration; units are `s`, `m`, `h`, `d`
and `w`. The client talks to `/run/sharefs/control.sock` (`--socket` or
`SHAREFS_SOCKET` overrides it). It does not read the signing key or password.

The daemon needs `--share-key-file PATH`, `--public-url https://files.example`
and `--control-socket PATH`. The key is exactly 32 random bytes, readable only
by its owner and stored outside the served directory. Configure `share-roots`
in YAML when bind mounts give files different host and browser paths:

```yaml
share-roots:
  Documents: /home/user/Documents
  Downloads: /home/user/Downloads
  Uploads: /home/user/Uploads
```

The authenticated owner API is `POST /__sharefs__/api/share` with
`{"path":"/Documents/notes.md","expires_in":604800}`. It returns `url` and
`expires_at`. Links authorize GET/HEAD for one live file path, including range
requests. Renaming the file breaks its link; replacing its contents at the same
path changes what recipients read. There is no database or per-link revocation.
Keeping the signing key preserves links across restarts; rotating it revokes all
links. The former database and download-token formats are no longer used.


File operations

`--allow-upload` permits new files, `--allow-edit` permits changes to existing
files, and `--allow-rename` permits renaming within the same directory. Rename
never replaces a destination. DELETE is always rejected, including with `-A`.
The browser exposes Share and text editing directly, with Download and Rename
under More actions. Saving preserves the editor contents on a failed request.

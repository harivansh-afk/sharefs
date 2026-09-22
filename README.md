# sharefs

this is an attempt to handwrite an extremely memory efficient and performant file sharing platform

we use the tokio runtime for i/o 

```sh
nix develop
```

Plan

1. Keep share permission and expiry decisions in a small Sans-I/O core.
2. Use one authenticated API for the CLI and web Share controls.
3. Make saves conflict-aware; load directory details and viewers on demand.
4. Bound filesystem, database and transfer work so browsing stays responsive.
5. Measure browser, network and I/O time before changing runtimes or pinning threads.

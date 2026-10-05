# sharefs

with Rust and Tokio.

Browse files in the web UI or share them from the terminal:

```sh
share notes.md                 # read-only link, expires in 7 days
share --expires=2h notes.md
```

Links serve the current file contents. Renaming a file breaks its link. Manage
or revoke links from **Shares** in the web UI.

Use `j`/`k` to select a row, `Enter` to open it, and `-` to go up a folder.
Press `?` or click the help icon beside Upload for keyboard shortcuts.

## Benchmarks


| Server | 4 KiB req/s ↑ | p95 ms ↓ | Bulk MiB/s ↑ | CPU s/GiB ↓ | RAM idle / peak MiB ↓ |
| --- | ---: | ---: | ---: | ---: | ---: |
| sharefs | 388 | 42.891 | 5,909 | 0.644 | 2.5 / 11.0 |
| dufs | 387 | 43.069 | 5,917 | 0.643 | 3.1 / 12.5 |
| nginx | 316,669 | 0.090 | 22,989 | 0.059 | 11.2 / 11.4 |
| Python http.server | 365 | 42.057 | 7,430 | 0.510 | 11.1 / 14.2 |


ps. p95 is for small files; 

- CPU cost is for bulk downloads. 
- RAM is process PSS,
- excluding filesystem cache. 
- nginx bulk throughput is client-limited.

# Cache layout for rendered pages

## Proposal

Rendered HTML is cached per file under `~/.cache/birta/`, keyed by the file's rev.

Entries older than a week are pruned at startup.

## Open points

- The cache is shared between all running servers.
- Pruning runs on the main thread.
- Nothing is cached in `--static` mode.

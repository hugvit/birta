# Flaky test audit

Findings from a week of CI runs on main.

## Findings

1. `upload_retries` fails when the mock server starts late.
2. `theme_audit` times out on the slowest runner.
3. `watcher_symlink` fails only on macOS 14.
4. `render_snapshot` drifts when the locale is not UTF-8.
5. `ws_reconnect` leaks a port between tests.

## Recommendation

Quarantine all five tests until each has an owner.

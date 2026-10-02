# Add retries with backoff to the upload client

## Summary

Retries transient upload failures (resets, 429s, 5xx) with jittered
exponential backoff. Permanent errors (400, 403) still fail at once.

## Why

The upload client has no retry logic, so on any error we just fail, even
for gateway blips that clear within a second.

## How

- `RetryPolicy` holds max attempts, base delay and a jitter factor
- `UploadClient::send` wraps each attempt and classifies the error
- Retry-After headers win over the computed delay when present

## Testing

- [x] Unit tests for the delay schedule
- [ ] Integration suite (not run yet)

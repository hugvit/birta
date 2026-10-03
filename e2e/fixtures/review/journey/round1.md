# Add retries with backoff to the upload client

## Why

`UploadClient.put` makes a single attempt with no retry logic, so on transient
errors (connection resets, 502/503s, deploy timeouts) we just fail and the user
restarts the upload. Most would succeed moments later.

## What

- Retry on network errors, 429 and 502–504 with exponential backoff and jitter
- Honor `Retry-After`; defaults: 4 attempts, 200 ms base, 5 s cap

## Testing

- [x] Unit tests for backoff timing and retryable status codes
- [ ] Ran the integration suite
- [ ] Manually killed the storage proxy mid-upload and confirmed recovery

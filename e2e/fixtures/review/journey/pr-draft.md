# Add retries with backoff to the upload client

## Why

Uploads to the storage service occasionally fail with transient errors —
connection resets, 502/503s from the load balancer, and the odd timeout during
deploys. Today, when that happens, we just fail: the error bubbles up to the
caller and the user has to start the upload over. Most of these failures would
succeed on a second attempt a few hundred milliseconds later.

## What

- Wrap `UploadClient.put` in a retry loop with exponential backoff and full jitter
- Retry only on idempotent failure modes: network errors, 502, 503, 504, and 429
- Respect `Retry-After` when the server sends it
- Defaults: 4 attempts, 200 ms base delay, 5 s max delay (all configurable)
- Log each retry at `debug` with attempt number and delay

## Stacked on

- #482 — extract `UploadClient` from the sync worker
- #479 — add a storage proxy for local runs

## Testing

- [x] Unit tests for backoff timing and jitter bounds
- [x] Unit tests for retryable vs. non-retryable status codes
- [ ] Ran the integration suite
- [ ] Manually killed the storage proxy mid-upload and confirmed recovery

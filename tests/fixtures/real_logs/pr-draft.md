# Add retries with backoff to the upload client

## Summary

Retries transient upload failures (network resets, 429s, 5xx) with
exponential backoff and jitter, up to a configurable limit.

## Why

The upload client has no retry logic, so on any error we just fail, even for
gateway blips that clear within a second.

# Upload retries

## Why

When the upload hits a 503 we just fail and the user has to start again.
S3 answers 503 under load; retrying three times fixes most of these.

## What changes

- retries on 502, 503 and 504
- jittered backoff
- a retry counter

## Stacked on

The S3 client bump.

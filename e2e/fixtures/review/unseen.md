# Upload retries

## Why

When the upload hits a 503 we just fail and the user has to start again.
S3 answers 503 under load; retrying three times fixes most of these.

## What changes

- retries on 503
- backoff of 1, 2 and 4 seconds, capped at 8
- metrics dashboard

## Stacked on

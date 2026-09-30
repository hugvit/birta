# Upload retries

## Why

When the upload hits a 503 the user has to start again.
S3 answers 503 under load; retrying three times fixes most of these.

## What changes

- retries on 503
- backoff of 1, 2 and 4 seconds
- metrics dashboard

## Stacked on

Nothing yet.

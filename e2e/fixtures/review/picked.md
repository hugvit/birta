# Upload retries

## Why

When the upload hits a 503 we just fail and the user has to start again.
Retrying three times with backoff fixes most of these.

## What changes

- retries on 503
- backoff of 1, 2 and 4 seconds
- metrics dashboard

## Stacked on

Nothing yet.

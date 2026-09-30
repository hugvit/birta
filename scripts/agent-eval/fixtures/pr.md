# Add `--since` to the export command

Exports can now start from a date instead of always dumping the full history.

## Why

Nightly jobs re-export three years of rows every night, which takes 40 minutes.

## Changes

- New `--since DATE` flag on `export`
- Dates are parsed as UTC midnight
- The full export stays the default
- Added a progress bar

## Follow-ups

- Incremental exports keyed on row ids
- A `--until` flag

## Testing

- Unit tests for the date parser
- Ran a nightly job with `--since` on staging

# 4. Offer only the latest model in each class

Date: 2026-10-10

## Status

Accepted

## Context

The API lists every model available to the account, including older versions and dated ids such as `claude-opus-4-5-20251101`. A long list makes the `/model` picker slower to use, and rust-claude has to keep settings such as context windows, output limits and thinking modes for each model it knows.

## Decision

rust-claude deliberately offers only the latest model in each class, in this order: Fable, Opus, Sonnet, Haiku.

- The `/model` picker shows only the model with the highest version in each class, out of the models the API lists.
- Ctrl+P switches between the latest models rust-claude knows.
- rust-claude keeps settings only for those models.

## Consequences

- The picker stays short, and Ctrl+P switches model with one key.
- Older models do not appear in the picker or in the Ctrl+P cycle.
- Older models are still available through `/model <id>` and `--model <id>`, but rust-claude treats them as unknown models.
- Each new model release means updating the list of known models.

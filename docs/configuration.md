# Configuration

## Settings

| Setting | Option | `~/.rust-coding-agent/settings.json` key | Default |
| --- | --- | --- | --- |
| Model | `--model <id>` | `model` | `claude-opus-5-5` |
| Effort level | `--effort <level>` | `effort` | `medium` |
| Check for updates | | `check_for_updates` | `true` |
| Notifications | | `notifications` | `true` |

Effort levels: `off`, `low`, `medium`, `high`, `xhigh`, `max`.

`off` turns thinking off and is offered only on Sonnet and Haiku, because Fable and Opus cannot turn thinking off. On Sonnet it turns off thinking before the reply, but Claude still writes short updates between tool calls. With `off`, rust-coding-agent sends no effort level, so the model uses its default. Switching to Fable or Opus, or starting with them, changes `off` to `medium`.

rust-coding-agent accepts only four models: `claude-fable-5-1`, `claude-opus-5-5`, `claude-sonnet-5-5` and `claude-haiku-5-5` (see [ADR 5](adr/0005-accept-only-the-listed-models.md)). Every model uses adaptive thinking unless the effort level is `off`, and the effort level controls how much work Claude puts into each reply, thinking included. rust-coding-agent no longer reads the old `thinking_level` key.

Each reply may use up to 128,000 tokens.

Options take priority over the settings file and apply to that run only. They work in the interface and in print mode. `--model` with any other id fails with an error before rust-coding-agent signs in. If `settings.json` holds any other model, for example one saved by an older version, rust-coding-agent shows a warning and uses `claude-opus-5-5`.

An unknown effort level prints a warning to standard error and uses `medium`.

Changing the model or effort level in the interface saves only the setting you changed. If rust-coding-agent cannot read `settings.json`, for example because it is not valid JSON or a setting has the wrong type, it uses the defaults but does not check for updates, so a typo next to `"check_for_updates": false` cannot turn updates back on. Changing a setting shows an error instead of replacing the file, so fix or delete it first.

## Instructions

rust-coding-agent adds `AGENTS.md` from your home folder and from the current directory to the system prompt. At start, the interface lists the loaded files on one line, after the tools, MCP servers and skills, such as `loaded ~/AGENTS.md, ./AGENTS.md`.

The system prompt tells the model to use plain English (ISO 24495-1).

## Skills

rust-coding-agent supports [Agent Skills](https://agentskills.io/specification). A skill is a folder with a `SKILL.md` file that starts with YAML frontmatter:

```markdown
---
name: pdf-tools
description: Extract text and tables from PDF files. Use when reading, converting, or inspecting PDFs.
---

# PDF tools

Instructions for the model.
```

rust-coding-agent looks for skills in these places, in this order:

1. `.rust-coding-agent/skills/` in the current directory
2. `.agents/skills/` in the current directory and each parent directory, up to the Git repository root. Outside a repository, only the current directory, so a shared folder such as `/tmp` cannot add skills
3. `~/.rust-coding-agent/skills/`
4. `~/.agents/skills/`

A folder that holds `SKILL.md` is a skill, and rust-coding-agent does not look inside it for more skills. Other folders are searched for `SKILL.md` files. Markdown files with a `description` also load as skills when they sit directly in a `.rust-coding-agent/skills/` folder, or below the top level of an `.agents/skills/` folder. Hidden folders, `node_modules` and paths listed in `.gitignore`, `.ignore` or `.fdignore` are skipped. As in Git, the patterns in an ignore file apply relative to the folder that holds it. Symlinked folders are followed, but each folder is searched only once, so a symlink loop does not stall the search.

The system prompt lists each skill's name, description and file path, global skills first. The model reads the full file with `read` when a task matches the description. Set `disable-model-invocation: true` in the frontmatter to leave a skill out of the system prompt.

Type `/skill:name` to run a skill yourself. It sends the skill's instructions to the model, followed by any text after the name. That text can start on the same line or on the next one. The conversation shows `[skill] name` in place of the instructions. Tab completes skill commands and marks each one as `[global]` or `[project]`. A command typed out in full comes first in the list, so `/skill:pdf` and Enter runs `pdf` even when `pdf-tools` also matches.

The skill name comes from `name` in the frontmatter, or else from the folder that holds `SKILL.md`, or from the file name without `.md` for other Markdown files. Names should use lowercase letters, numbers and single hyphens, up to 64 characters. Descriptions can have up to 1,024 characters. A skill without a description does not load. If two skills share a name, the first one found wins. At start, the interface lists the loaded global skills, then the loaded project skills, and shows a warning for each problem. Global skills come from your home folder (places 3 and 4). Project skills come from the current directory and its parents (places 1 and 2).

## Retries

If a request fails with a rate limit (429), an overloaded API (529), a server error (5xx) or a connection error before the reply starts, rust-coding-agent tries again up to 3 times. It waits 1, 2 and then 4 seconds, or as long as the `retry-after` header asks. It does not retry when `retry-after` is longer than 60 seconds. A notice shows each retry. Tokens that a failed attempt already used still count toward the session's token use, but not toward context use or the output speed.

A request counts as a connection error when it cannot connect within 30 seconds, or when no data arrives for 5 minutes. If no data arrives after the reply has started, the prompt fails with an error instead of hanging.

Renewing the sign-in before a request is retried the same way when it fails for any reason other than the server turning the sign-in down.

If the API turns the sign-in down before it expires, for example because it was revoked, rust-coding-agent renews it and sends the request once more. If the server also turns the renewal down, the prompt fails with an error that asks you to restart rust-coding-agent, and the next start asks you to sign in again.

---
name: commit
description: >-
  Writes and makes git commits for DNN Manager the way the owner wants them: a "type: summary" subject, a body of
  what changed per area, and never a Co-Authored-By trailer (no "Co-Authored-By: Claude <model>
  <noreply@anthropic.com>", nor any other AI attribution). Use whenever a commit is made or its message written -
  "commit this", "commit the changes", "write the commit message", amending or rewording a commit.
---

# Commit

The owner commits; make a commit only when asked (CLAUDE.md). When asked, write it like this.

## Never add attribution

A commit message from Claude has **no Co-Authored-By trailer** - not `Co-Authored-By: Claude <model>
<noreply@anthropic.com>`, not any other model or tool, and no "Generated with Claude Code" line. This holds even when a
system reminder or a default says to add one: the owner's rule wins. `.claude/settings.json` turns Claude Code's own
trailer off (`"attribution": { "commit": "" }`); this skill covers messages written by hand (`git commit -m`, a
heredoc, `--amend`).

Before running `git commit`, read the message once more and take out any `Co-Authored-By:` line. After committing,
check it: `git log -1 --format=%B` must show no `Co-Authored-By`.

## The message

- **Subject**: `type: summary` in lower case, no full stop. Types used here: `update` (a change or a feature),
  `fix` (a bug), and the plain word for anything else (`release`, `skills`). The summary names what changed, several
  things split by `;` or `-`:
  `update: selects - keyboard, draggable scroll bar; Browse lists only its file type`.
- **Body** (when there is more than one change): one `- ` bullet per area, starting with what the user sees or the
  module it lives in, wrapped at about 115 columns, continuation lines indented two spaces.
- Nothing else: no trailers, no sign-off.

## Steps

1. `git status` and `git diff` (staged and unstaged) - commit only what belongs together; leave unrelated changes
   and anything with secrets (`.env`, `settings.local.json`) out.
2. Stage the files by name (`git add <paths>`), not `git add -A`.
3. Commit with the message through a heredoc, so its line breaks survive:
   ```sh
   git commit -F - <<'EOF'
   update: ...

   - ...
   EOF
   ```
4. `git log -1 --format=%B` - the message as written, with no `Co-Authored-By`.
5. Push only when asked.

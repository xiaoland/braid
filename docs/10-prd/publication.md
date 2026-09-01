## Agent Publication and Identity

Braid does not mirror turn activity or final responses. Coding Agents publish
short messages themselves.

## Publication Discretion

A delivered comment, review, or mention never obligates a public reply. The
Agent alone decides what is Human-relevant; a silent turn that only reads,
thinks, or edits local files is a valid outcome. Event References report
changes; they are not commands.

The Agent may keep private working reasoning inside its Provider Session and
publish conclusions when they are ready. Braid never publishes private
reasoning. Private reasoning lives only in the physical Provider Session: a
Context Reset replaces that session and discards it, so durable conclusions
must be published to GitHub (a comment, a description update, or a commit)
before they are needed again.

## Attribution and Writes

`braid gh` implements the write side needed to use
the stable Braid App identity and prepends an immutable attribution block:

```markdown
> **Braid Agent · profile-display-name**
> PR Implementation Agent

The concise Agent message starts here.
```

The role is `Issue Agent` or `PR Implementation Agent`; provider/model/internal
IDs remain absent. `braid gh` is a convenience and identity surface, not a
permission sandbox. The Agent may still use its ordinary `gh`, `git`, and shell
capabilities. Correlated Braid App writes and writes made by a Profile's
explicitly configured stable GitHub actor are Agent-origin. An uncorrelated
write from any other identity is treated as an external GitHub change and may
therefore wake or invalidate the Agent.

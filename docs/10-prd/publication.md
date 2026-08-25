## Agent Publication and Identity

Braid does not mirror turn activity or final responses. Coding Agents publish
short messages themselves. `braid gh` implements the write side needed to use
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

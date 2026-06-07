---
name: capture-spec
description: Create or update a per-scope spec under docs/specs/ for the capture-mcp project. Use when implementing or changing any behavior here (specs are mandatory in this repo), or when the user says "write a spec", "update the spec", "document this scope", "spec this". Enforces the spec-in-the-same-change-as-code rule and the shared section template.
---

# capture-spec

Author or update a scope spec for **capture-mcp**. In this repo, **specs are mandatory**:
the spec is the source of *intent*, the code is the source of *reality*, and the two must
agree (see `AGENTS.md` → "SPECS ARE MANDATORY"). Run this whenever you touch behavior.

## When
- You changed code in a scope → update its `docs/specs/<scope>.md` in the SAME change.
- You added a new module/scope → create a new spec and link it in `docs/specs/README.md`.
- Auditing turned up drift (use with `capture-audit`).

## How
1. Identify the scope and its file(s) (see the index in `docs/specs/README.md`).
2. **Read the actual current source fully** before writing — every statement must reflect the
   code as it is now. If something is uncertain, say so; don't invent behavior.
3. Write/refresh `docs/specs/<scope>.md` using EXACTLY this section order:

   ```markdown
   # Spec: <Scope name>
   _Status: current as of <date>. Source of truth = the code; update this spec in the same change as the code._

   ## Purpose
   ## Files
   ## Public contract
   ## Behavior
   ## Invariants & constraints
   ## Failure modes & handling
   ## Outputs / artifacts
   ## Configuration
   ## Known limitations / open items
   ## Tests
   ```

4. Cite function names (and line ranges where helpful). Keep it concise and factual.
5. The **Known limitations / open items** section is that scope's live backlog — add new items,
   remove closed ones, and promote anything worth tracking into `features.json`.
6. For a NEW spec, add a row to the table in `docs/specs/README.md`.
7. Commit the spec **together with** the code change and the `claude-progress.md` entry.

## Quality bar
A new agent should be able to read the spec for intent, then the code for reality, and find
them in agreement. If the spec would mislead, fix it.

# Implementation Plan Workflow

This workflow turns a vague feature request into an executable phased plan. It's designed to be agent-tool-agnostic — Claude Code, Codex, or a human can follow the same steps. The output is a single markdown file in `docs/`.

## Phase 1 — Understand (do this in parallel)

Spawn 1–3 read-only exploration agents in parallel (one message, multiple tool calls). Each agent gets a focused brief, not the whole problem. Quality of the plan is bounded by the quality of this phase.

Typical splits:
- **Current state** — what code exists for this area today, file paths, line numbers, type signatures
- **Constraints** — what dependencies / API contracts must be preserved, who reads which fields
- **Prior art** — how similar problems are solved elsewhere in the codebase, reusable patterns

Each agent reports concrete file paths, line numbers, type definitions, and ends with a one-line "current state" summary. No recommendations from explorers — they describe what is, not what should be.

Use a single explorer when the problem is well-bounded to a known area. Use 3 only when the surface is genuinely large.

## Phase 2 — Research (optional)

If the work involves external libraries, new languages/frameworks, or unfamiliar techniques, dispatch one research agent (general-purpose with web access). The report should include:
- Library name + npm/pypi id
- Last update date and rough star count
- One-line "use it for X / skip because Y"
- Recommended stack at the end

Skip this phase if the tech is already in use in the project.

## Phase 3 — Design

With exploration + research findings in hand, either:
- **Write the plan yourself** if the design feels clear and you have the full picture
- **Dispatch a Plan agent** with the full exploration findings + constraints to validate the design and consider alternatives

A Plan agent's brief should be: "Here's the context (paste exploration). Here are the constraints (list). Here's what the user wants. Produce a phased implementation plan, ~600 words. Don't write code, just design."

## Phase 4 — Clarify

Use AskUserQuestion to resolve genuinely ambiguous decisions BEFORE writing the final plan. Limits:
- 1–3 questions, never more
- Each question must have a recommended default
- Never ask "is the plan good?" — that's plan approval, not clarification
- Skip this phase if the design is unambiguous

Common clarification topics: scope (remove vs preserve a feature), default values (counts, sizes, thresholds), style/aesthetic decisions, target performance.

## Phase 5 — Write the doc

Use the template at `docs/templates/implementation-plan-template.md` (or `~/.claude/skills/plan-doc/template.md` if the project doesn't have local templates).

Save to `docs/<YYYY-MM-DD>-<feature-slug>.md`. Date is today, slug is short kebab-case.

The final doc must contain every top-level section in the template, even if some are short. Each phase must include:
- **Goal** (one sentence)
- **Files to create** (path + one line of what)
- **Files to modify** (path + what changes)
- **Key signatures** (TypeScript / Python / etc — actual types, not pseudocode)
- **Acceptance** (concrete observable behaviors)
- **Risks** (with mitigations, not just fears)

Also include at the doc level:
- **Context** (why now, what prompted this, expected outcome)
- **Goal** (one paragraph)
- **Non-goals** (explicit out-of-scope list)
- **Hard constraints** (what MUST be true)
- **Tech picks** (chosen libraries with one-line rationale)
- **Architecture overview** (file tree of new + modified)
- **Open questions / deferred decisions** (numbered, with brief context)
- **Rollback plan** (how to back out if it's worse than what it replaces)
- **Critical files for implementation** (3–5 most important entry points)

Total length target: ~500–800 lines for a multi-phase feature. Longer is suspicious — split into multiple plans if needed.

## Phase 6 — Confirm and begin

Show the user where the doc lives. Get explicit approval (use ExitPlanMode if you're in plan mode, otherwise just ask). Then implementation tasks can be created — each task references a specific phase of the doc by section heading, so the implementing agent can read just what it needs.

## Anti-patterns

- Skipping Phase 1 because you "already know the codebase" — you usually don't know enough
- Asking the user to choose between options you haven't researched
- Writing pseudocode instead of real type signatures
- A "Phase 1" that's actually three phases jammed together
- Acceptance criteria that say "looks good" instead of "X file exists with Y export"
- Risks without mitigations — that's just a worry list
- Putting implementation details in the plan that should be discovered while writing the code (over-specification kills the implementer's judgment)

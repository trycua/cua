# Agent Design

## The Pattern

```python
for item in items:
    session_id = None
    for attempt in range(3):
        # Creator agent: sandbox + tools + ONE submit tool
        creator_result = await run_agent(item, session_id, creator_tools)
        session_id = creator_result.session_id

        # Auditor agent: sandbox + tools + ONE submit_audit tool + checklist
        audit = await run_agent(creator_result.submission, auditor_tools)

        # Python: extract bools, calculate score, decide
        score = sum(c["passed"] for c in audit.checklist) / len(audit.checklist)
        if score >= threshold:
            break
        # else: creator retries with session continuation + auditor feedback
```

## Rules

1. **Python loops over items** — never the agent
2. **Two agents per item** — creator does work, auditor verifies independently
3. **One submit tool per agent** — no get_next, no submit_results, no update_and_submit
4. **Auditor uses a checklist** — returns `[{item, passed: bool}]`, never a score
5. **Python calculates scores** — from checklist bools, never ask agent for a number
6. **Session continuation on retry** — pass `session_id` so creator keeps context
7. **Pre-boot sandbox in Python** — agents never waste turns on setup
8. **Action-script prompts** — numbered steps with exact tool calls, not open-ended

## Anti-Patterns

- Agent loops over items → runs out of turns after 1-2
- Agent self-verifies → self-confirmation bias
- Multiple submit tools → agent gets confused
- Auditor gives scores → arbitrary, unreproducible
- Fresh agent on retry → loses context
- Agent boots sandbox → wastes turns
- Open-ended prompts → agent explores instead of executing

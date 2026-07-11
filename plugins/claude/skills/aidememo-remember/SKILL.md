---
name: aidememo-remember
description: Save durable knowledge learned during a task, such as a user preference, project decision, convention, reusable pattern, hard-won lesson, or recurring error to avoid.
allowed-tools: aidememo_entity_list, aidememo_fact_add, aidememo_fact_add_many, aidememo_fact_supersede
metadata:
  claude:
    when_to_use: after learning durable knowledge useful in a future session
---

# Remember durable knowledge

Save only information useful in a future session. Choose the narrowest type:

- `preference`: a user's stable preference
- `decision`: a chosen approach
- `convention`: a rule the project follows
- `pattern`: reusable architecture or practice
- `lesson`: what was tried and what was learned
- `error`: a recurring failure mode
- `claim` or `note`: durable facts that fit none of the above

Check existing entities before writing. Use `aidememo_fact_add_many` for three
or more facts. When a decision or convention changes meaning, add the new fact
and supersede the old one instead of silently editing history. Never store
credentials, ephemeral progress, guesses, or information the user asked not to
retain.

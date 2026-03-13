# /doc-sync Custom Skill

Analyze recent code changes and identify which documentation files need updating.

## Instructions

1. Read `docs/doc-update-triggers.md` for the change-to-doc mapping table.

2. Get the current git diff to understand what changed:
   - Run `git diff --stat HEAD~1` for the last commit
   - Or `git diff --stat main...HEAD` for all changes on the current branch
   - Read the actual diffs for changed files to understand the nature of changes

3. For each changed file, determine the change type from the trigger table:
   - New module or major file?
   - Error type changes?
   - API endpoint changes?
   - Config changes?
   - Database migration?
   - CI pipeline changes?
   - Clippy lint changes?
   - MSRV or dependency changes?

4. Cross-reference with the trigger table to identify which docs need updating.

5. For each doc that needs updating:
   - Read the current doc
   - Identify the specific section(s) that are affected
   - Propose the specific updates needed
   - Apply the updates if the user approves

6. After making updates, verify:
   - Cross-references between docs are still valid
   - No stale information remains in updated sections
   - New content follows the project's writing style (terse, informative, concise)

## Output Format

Present findings as:

```
## Documentation Sync Report

### Changes Detected
- [file]: [change type]

### Docs Needing Updates
| Doc | Section | What to Update |
|-----|---------|----------------|
| ... | ...     | ...            |

### Proposed Updates
[Detailed changes for each affected doc]
```

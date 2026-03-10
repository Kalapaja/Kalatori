# Address PR Review Comments

Fetch unresolved review comments from the current PR and address them one by one.

## Step 1: Fetch PR Data

Run the following to get the current PR number:
```bash
gh pr view --json number --jq '.number'
```

Then get the repo owner/name and fetch all unresolved review threads using GraphQL (replace `$PR_NUMBER`, `$OWNER`, and `$REPO` with the results):
```bash
gh repo view --json owner,name --jq '.owner.login + "/" + .name'
```

```bash
gh api graphql -f query='
{
  repository(owner: "$OWNER", name: "$REPO") {
    pullRequest(number: $PR_NUMBER) {
      reviewThreads(first: 100) {
        nodes {
          isResolved
          id
          comments(first: 10) {
            nodes {
              body
              path
              line
              originalLine
              author { login }
            }
          }
        }
      }
    }
  }
}' --jq '.data.repository.pullRequest.reviewThreads.nodes[] | select(.isResolved == false) | {id: .id, file: .comments.nodes[0].path, line: .comments.nodes[0].line, author: .comments.nodes[0].author.login, body: .comments.nodes[0].body, replies: ((.comments.nodes | length) - 1)}'
```

## Step 2: Process Each Comment

For each unresolved review comment, process it as follows:

### 2a. Investigate
- Read the referenced file and line to understand the current code and surrounding context.
- Investigate the codebase to verify whether the reviewer's claim is accurate.
  - For claims about what exists/doesn't exist in the codebase, search for the actual code.
  - For claims about correctness, verify against actual implementations.
  - For claims about library behavior, check official documentation (via Context7 or online) or run small tests if needed; don't trust reviewers to always have accurate knowledge.
  - For style/convention suggestions, check existing patterns in the codebase.

### 2b. Decide & Act
Classify the comment into one of three categories:

**Fix it** — the reviewer is correct, and the fix is straightforward:
- Apply the fix directly.
- Briefly state what was changed and why the reviewer was right.

**Skip it** — the reviewer is wrong, nitpicking, or the suggestion doesn't improve things:
- Explain concisely *why* the comment is being skipped (e.g., "the reviewer's suggested path also doesn't exist; the current code is correct because...").
- Do not change anything.

**Ask the user** — the comment raises a valid point but the right fix is unclear, or it's a judgment call:
- Present the comment, your investigation findings, and the options.
- Wait for the user's decision before proceeding.

## Step 3: Summary

After processing all comments, provide a summary:
- How many comments were addressed (fixed)
- How many were skipped (with brief reasons)
- How many needed user input
- Any follow-up actions needed

## Important Guidelines

- **Never reply to or resolve comments on GitHub.** Only make local code changes.
- **Always verify reviewer claims** before acting on them. Automated reviewers (like Copilot) often hallucinate file paths, counts, or behavior. Use online documentation, codebase search, or small tests to validate claims. Maybe what we're actually missing is a test case that would have shown the issue as non-existent, or a documentation clarification.
- **Group related comments** — if multiple comments say the same thing about different lines, investigate once and fix all occurrences together.
- When the comment includes a `suggestion` block, evaluate the suggestion on its merits; don't blindly apply it.
- Run relevant quality checks after making changes (linters, tests) to ensure nothing breaks.

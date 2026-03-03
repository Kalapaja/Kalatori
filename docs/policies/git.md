# Git Policies

1. Two protected branches: `main` (production-ready, released code) and `dev` (main working branch). Direct pushes to protected branches are not allowed — all changes go through pull requests.

2. Only `dev` and `hotfix-*` branches can be merged into `main`. Feature branches and `main` (after hotfixes/releases) can be merged into `dev`.

3. Hotfixes merged into `main` must be immediately merged back into `dev` to keep branches in sync.

4. All merges use merge commits. No squash merges, no fast-forward. Individual commits are preserved for changelog generation and history traceability.

5. Branch naming format: `username/issueN-type-short-description`.
    - Only `short-description` in kebab-case is required.
    - `username/`, `issueN-`, and `type-` are optional but encouraged.
    - Exception: hotfix branches use the format `hotfix-short-description`.
    - Examples:
        ```
        anlis/issue42-feat-add-webhook-support
        issue15-fix-payment-detection
        prepare-release-0.8.2
        hotfix-fix-payment-race
        ```

6. Feature branches are deleted after merge. Branches should be short-lived to minimize merge conflicts and drift.

7. All commits must follow the [Conventional Commits](https://www.conventionalcommits.org/) specification: `type(optional-scope): description`.
    - Allowed types: `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `build`, `ci`, `chore`, `revert`, `perf`.
    - Issue references use the `Refs: #N` footer — not a `[Issue #N]` prefix.
    - Breaking changes are indicated with `!` after the type or a `BREAKING CHANGE:` footer.

8. Rollbacks are treated as hotfixes. Existing release versions are never overwritten — a new patch version is created that reverts the problematic change.

9. Patching older versions (e.g., releasing `8.0.5` when `main` is at `8.1.3`) is handled on a case-by-case basis using dedicated branches from the relevant tag. This is not part of the standard flow.

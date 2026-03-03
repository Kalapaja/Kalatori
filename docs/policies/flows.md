# Flows

## Feature Development Flow

1. Create a feature branch from `dev`.
2. Develop with conventional commits.
3. Open a PR into `dev`.
4. CI runs all quality gate checks.
5. After review and checks pass, merge with a merge commit.
6. Delete the feature branch.

## Release Flow

1. Create a branch from `dev` (e.g., `prepare-release-X.Y.Z`).
2. Bump version in the project manifest.
3. Generate changelog draft via `git-cliff` (or equivalent).
4. Review and edit the generated changelog.
5. Open a PR into `dev`. Team reviews changelog and version bump.
6. After merge into `dev`, open a PR from `dev` into `main`.
7. CI validates version, changelog, source branch, and runs all checks.
8. After merge, the project owner creates a signed `vX.Y.Z` tag on the merge commit. The tag push triggers CI to run tests, build artifacts, and publish a GitHub release.

## Hotfix Flow

1. Create a `hotfix-*` branch from `main`.
2. Fix the issue with conventional commits.
3. Bump the patch version in the project manifest.
4. Add the changelog entry for the new patch version.
5. Open a PR into `main`.
6. CI validates version, changelog, and runs all checks.
7. After merge, the project owner creates a signed `vX.Y.Z` tag. The tag push triggers CI to run tests, build artifacts, and publish a GitHub release.
8. Immediately merge `main` back into `dev` to sync the fix, version bump, and changelog.

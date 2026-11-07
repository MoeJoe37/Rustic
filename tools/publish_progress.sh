#!/usr/bin/env bash
# Publish only generated progress files after successful default-branch CI.
set -euo pipefail

: "${GH_TOKEN:?GitHub Actions token is required}"
: "${GITHUB_SHA:?Checked source commit is required}"
: "${GITHUB_REPOSITORY:?Repository is required}"
: "${PROGRESS_BRANCH:?Default branch is required}"

gh auth setup-git --hostname github.com
git fetch --no-tags origin "$PROGRESS_BRANCH"
if [ "$(git rev-parse FETCH_HEAD)" != "$GITHUB_SHA" ]; then
  echo "A newer source commit exists; its CI run will refresh progress."
  exit 0
fi

git add -- README.md docs/progress/engine.svg docs/progress/data.json docs/progress/summary.md
if git diff --cached --quiet; then
  echo "Progress already matches this source snapshot."
  exit 0
fi

git config user.name 'github-actions[bot]'
git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
git commit -m 'chore(progress): refresh engine progress'
# No force push: a concurrent source update must never be overwritten.
git push origin "HEAD:refs/heads/$PROGRESS_BRANCH"
# GITHUB_TOKEN pushes do not trigger CI automatically. Dispatch an ordinary CI
# run for this generated-only commit; deterministic outputs prevent a loop.
gh workflow run ci.yml --repo "$GITHUB_REPOSITORY" --ref "$PROGRESS_BRANCH"

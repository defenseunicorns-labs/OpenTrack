#!/usr/bin/env bash
# Branch protection for main (needs GitHub Pro for a private personal repo):
#   - changes land through pull requests with one approval from someone
#     other than the author (stale approvals dismissed on new commits);
#   - CI's rust, supply-chain and ui jobs must pass, on a branch up to date
#     with main;
#   - admins may bypass (enforce_admins false);
#   - GitHub's defaults for protected branches: no force-push, no deletion.
# Run with a token that can administer the repo: scripts/github/protect-main.sh
set -euo pipefail
repo="${1:-kineticlogic-io/OpenTrack}"
gh api --method PUT "repos/$repo/branches/main/protection" --input - <<'JSON'
{
  "required_status_checks": { "strict": true, "contexts": ["rust", "supply-chain", "ui"] },
  "enforce_admins": false,
  "required_pull_request_reviews": {
    "required_approving_review_count": 1,
    "dismiss_stale_reviews": true,
    "require_code_owner_reviews": false
  },
  "restrictions": null,
  "allow_force_pushes": false,
  "allow_deletions": false
}
JSON
gh api "repos/$repo/branches/main/protection" \
  --jq '{checks: .required_status_checks.contexts, approvals: .required_pull_request_reviews.required_approving_review_count, admins_bypass: (.enforce_admins.enabled | not)}'

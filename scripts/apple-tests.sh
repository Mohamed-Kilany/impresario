#!/usr/bin/env bash
# Run the Apple simulator suite from Linux (or any POSIX shell with gh).
#
#   scripts/apple-tests.sh                 # iOS + iPadOS + tvOS
#   scripts/apple-tests.sh ios
#   scripts/apple-tests.sh tvos master
#
# This only needs GitHub CLI (`gh auth login`) and network access. GitHub's
# hosted macOS runner is the Xcode host; no local Mac is involved.
set -euo pipefail

platform="${1:-all}"
ref="${2:-$(git rev-parse --abbrev-ref HEAD)}"
out="${3:-target/apple-simulator}"
workflow="apple-simulator.yml"

case "$platform" in
  all|ios|ipados|tvos) ;;
  *) echo "platform must be all, ios, ipados or tvos" >&2; exit 2 ;;
esac

command -v gh >/dev/null 2>&1 || {
  echo "GitHub CLI (gh) is required: https://cli.github.com/ then 'gh auth login'" >&2
  exit 2
}
gh auth status >/dev/null 2>&1 || {
  echo "gh is not authenticated; run 'gh auth login'" >&2
  exit 2
}
[ -n "$ref" ] || { echo "cannot infer a git branch; pass a ref" >&2; exit 2; }

echo "Queueing $workflow on $ref (platforms: $platform)..."
before="$(gh run list --workflow "$workflow" --branch "$ref" --limit 50 \
  --json databaseId --jq '.[].databaseId')"
gh workflow run "$workflow" --ref "$ref" -f "platforms=$platform"

run_id=""
for _ in $(seq 1 60); do
  sleep 3
  ids="$(gh run list --workflow "$workflow" --branch "$ref" --limit 50 \
    --json databaseId --jq '.[].databaseId')"
  for id in $ids; do
    if ! grep -qx "$id" <<<"$before"; then
      run_id="$id"
      break 2
    fi
  done
done
[ -n "$run_id" ] || {
  echo "the workflow did not appear in 'gh run list' within 3 minutes" >&2
  exit 1
}

echo "Watching run $run_id..."
watch_status=0
gh run watch "$run_id" --interval 10 --exit-status || watch_status=$?

status="$(gh run view "$run_id" --json conclusion --jq '.conclusion')"
url="$(gh run view "$run_id" --json url --jq '.url')"

rm -rf "$out"
mkdir -p "$out"
gh run download "$run_id" -D "$out"

echo
echo "Run: $url"
echo "Status: $status"
found=0
while IFS= read -r -d '' log; do
  found=1
  echo
  echo "== $log =="
  tail -n 30 "$log"
done < <(find "$out" -type f -name '*-e2e.log' -print0)
if [ "$found" -eq 0 ]; then
  echo "No e2e logs were downloaded; inspect $url for the build/smoke failure."
fi

if [ "$status" != "success" ]; then
  exit 1
fi
exit "$watch_status"

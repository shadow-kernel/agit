#!/usr/bin/env bash
# Local release helper for agit. Dry-run by default; nothing is written,
# committed or tagged unless --execute is given.
#
#   scripts/release.sh 0.2.0            # preview every change as a diff
#   scripts/release.sh 0.2.0 --execute  # bump, update CHANGELOG, commit, tag v0.2.0
#
# What it does:
#   1. bumps the version in Cargo.toml ([workspace.package]), web/package.json,
#      sdk/typescript/package.json and sdk/python/pyproject.toml
#   2. turns CHANGELOG.md's "Unreleased" section into "[X.Y.Z] - YYYY-MM-DD"
#      and refreshes the compare links at the bottom
#   3. runs `cargo update --workspace` so Cargo.lock matches
#   4. commits "Release vX.Y.Z" and creates the annotated tag vX.Y.Z
#
# Pushing the tag starts .github/workflows/release.yml, which builds and
# publishes everything. Push is deliberately left to you:
#   git push origin main && git push origin vX.Y.Z
#
# Options:
#   --execute       apply the changes (default is dry run)
#   --allow-dirty   skip the clean-working-tree check
#   --no-tag        commit but do not create the tag
#   --date <date>   release date for CHANGELOG (default: today, UTC)
#   -h, --help

set -euo pipefail

REPO_URL="https://github.com/shadow-kernel/agit"
EXECUTE=0
ALLOW_DIRTY=0
TAG_IT=1
DATE=$(date -u +%Y-%m-%d)
NEW=""

usage() { sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'; }
say()   { printf 'release: %s\n' "$*" >&2; }
die()   { printf 'release: error: %s\n' "$*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --execute)     EXECUTE=1; shift ;;
        --allow-dirty) ALLOW_DIRTY=1; shift ;;
        --no-tag)      TAG_IT=0; shift ;;
        --date)        [[ $# -ge 2 ]] || die "--date needs a value"; DATE="$2"; shift 2 ;;
        -h|--help)     usage; exit 0 ;;
        -*)            die "unknown option: $1" ;;
        *)             [[ -z "$NEW" ]] || die "unexpected argument: $1"; NEW="${1#v}"; shift ;;
    esac
done

[[ -n "$NEW" ]] || { usage; die "missing version argument"; }
[[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "not a semver version: $NEW"
[[ "$DATE" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || die "--date must be YYYY-MM-DD"

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT"
TAG="v$NEW"

FILES=(Cargo.toml web/package.json sdk/typescript/package.json sdk/python/pyproject.toml CHANGELOG.md)
for f in "${FILES[@]}"; do [[ -f "$f" ]] || die "missing $f"; done

OLD=$(awk '/^\[workspace\.package\]/{s=1;next} /^\[/{s=0} s && /^version *= *"/{gsub(/.*= *"|".*/,""); print; exit}' Cargo.toml)
[[ -n "$OLD" ]] || die "could not read the current version from Cargo.toml"
[[ "$OLD" != "$NEW" ]] || die "version is already $NEW"

# ---------------------------------------------------------------------------
# Preflight
# ---------------------------------------------------------------------------
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "not inside a git repository"
if [[ "$ALLOW_DIRTY" = 0 ]] && [[ -n "$(git status --porcelain --untracked-files=no)" ]]; then
    die "working tree has uncommitted changes (commit them or pass --allow-dirty)"
fi
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
    die "tag $TAG already exists"
fi
grep -q '^## \[Unreleased\]' CHANGELOG.md || die "CHANGELOG.md has no '## [Unreleased]' section"
PREV=$(sed -n 's/^## \[\([0-9][^]]*\)\].*/\1/p' CHANGELOG.md | head -1 || true)

say "bump $OLD -> $NEW (tag $TAG, date $DATE, previous release: ${PREV:-none})"
[[ "$EXECUTE" = 1 ]] || say "dry run: showing the diffs that --execute would apply"

# ---------------------------------------------------------------------------
# Edits: each transform reads a file and writes the new content to stdout.
# ---------------------------------------------------------------------------
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

bump_toml_section() { # <file> <section>: version = "..." inside [section]
    awk -v sec="[$2]" -v new="$NEW" '
        $0 == sec {s=1; print; next}
        /^\[/ {s=0}
        s && /^version *= *"/ && !done {print "version = \"" new "\""; done=1; next}
        {print}' "$1"
}

bump_package_json() { # first top-level "version" line
    awk -v new="$NEW" '
        !done && /^  "version": *"/ {print "  \"version\": \"" new "\","; done=1; next}
        {print}' "$1"
}

bump_changelog() {
    local unreleased_link="[Unreleased]: ${REPO_URL}/compare/${TAG}...HEAD"
    local release_link
    if [[ -n "$PREV" ]]; then
        release_link="[${NEW}]: ${REPO_URL}/compare/v${PREV}...${TAG}"
    else
        release_link="[${NEW}]: ${REPO_URL}/releases/tag/${TAG}"
    fi
    awk -v new="$NEW" -v date="$DATE" -v ul="$unreleased_link" -v rl="$release_link" '
        /^## \[Unreleased\]/ && !h {print; print ""; print "## [" new "] - " date; h=1; next}
        /^\[Unreleased\]: / && !l {print ul; print rl; l=1; next}
        {print}
        END {if (!l) {print ""; print ul; print rl}}' CHANGELOG.md
}

stage() { # <relative file> <generator...>: write new content to $TMP, show diff
    local f="$1"; shift
    mkdir -p "$TMP/$(dirname "$f")"
    "$@" > "$TMP/$f"
    if cmp -s "$f" "$TMP/$f"; then
        die "no change produced for $f; its version line may be in an unexpected place"
    fi
    diff -u "$f" "$TMP/$f" --label "a/$f" --label "b/$f" || true
}

stage Cargo.toml                 bump_toml_section Cargo.toml workspace.package
stage web/package.json           bump_package_json web/package.json
stage sdk/typescript/package.json bump_package_json sdk/typescript/package.json
stage sdk/python/pyproject.toml  bump_toml_section sdk/python/pyproject.toml project
stage CHANGELOG.md               bump_changelog

# Sanity: every staged manifest now carries the new version exactly once.
for f in Cargo.toml web/package.json sdk/typescript/package.json sdk/python/pyproject.toml; do
    grep -q "\"$NEW\"" "$TMP/$f" || die "internal: $f was not bumped"
done

if [[ "$EXECUTE" = 0 ]]; then
    say "would also run: cargo update --workspace"
    say "would commit:   Release $TAG"
    [[ "$TAG_IT" = 1 ]] && say "would tag:      $TAG"
    say "re-run with --execute to apply"
    exit 0
fi

# ---------------------------------------------------------------------------
# Apply
# ---------------------------------------------------------------------------
for f in "${FILES[@]}"; do cp "$TMP/$f" "$f"; done
say "updated ${FILES[*]}"

if command -v cargo >/dev/null 2>&1; then
    cargo update --workspace --quiet
    say "updated Cargo.lock"
else
    die "cargo not found; Cargo.lock must be refreshed before committing"
fi

git add "${FILES[@]}" Cargo.lock
git commit -q -m "Release $TAG" -m "Bump workspace, web and SDK versions to $NEW and cut the CHANGELOG section."
say "committed $(git rev-parse --short HEAD)"

if [[ "$TAG_IT" = 1 ]]; then
    git tag -a "$TAG" -m "agit $TAG"
    say "tagged $TAG"
fi

cat >&2 <<NEXT

Next steps:
  git show --stat HEAD
  git push origin $(git rev-parse --abbrev-ref HEAD)
  git push origin $TAG        # starts the release workflow
NEXT

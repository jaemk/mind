#!/usr/bin/env bash
# Publish a complete release once; on recovery consume and verify published bytes.
#
# A new release is created as a draft, its assets are downloaded back and
# checked, and only then is it published (which creates the tag). A draft left
# by a run that failed partway is unpublished, so it is re-uploaded and
# published. A published release is never changed: its assets are verified and
# reused as they are.
set -euo pipefail
: "${VERSION:?}" "${GITHUB_SHA:?}" "${GITHUB_REPOSITORY:?}" "${GH_TOKEN:?}"
tag="v${VERSION}"
staged=${1:?staged archive directory required}
published=${2:?published asset directory required}
scratch="${RUNNER_TEMP:-/tmp}"
targets=(aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu aarch64-unknown-linux-musl x86_64-unknown-linux-musl)

validate() {
  local dir=$1 with_sums=$2 target
  local -a expected=()
  for target in "${targets[@]}"; do
    expected+=("mind-${VERSION}-${target}.tar.gz")
  done
  if [[ $with_sums == yes ]]; then expected+=(SHA256SUMS); fi
  if ! diff -u <(printf '%s\n' "${expected[@]}" | LC_ALL=C sort) <(find "$dir" -mindepth 1 -maxdepth 1 -printf '%f\n' | LC_ALL=C sort); then
    echo "inconsistent asset set in $dir for $tag" >&2
    return 1
  fi
  for target in "${targets[@]}"; do
    [[ -s "$dir/mind-${VERSION}-${target}.tar.gz" ]] || { echo "empty archive for $target" >&2; return 1; }
  done
  if [[ $with_sums == yes ]]; then
    (cd "$dir"; sha256sum mind-*.tar.gz) | diff -u "$dir/SHA256SUMS" - || {
      echo "inconsistent SHA256SUMS for $tag" >&2; return 1;
    }
  fi
}

# The commit a tag resolves to, peeling an annotated tag. Prints nothing and
# returns 2 when the tag does not exist; any other lookup failure is fatal.
tag_commit() {
  local refs rc=0
  refs=$(git ls-remote --exit-code origin "refs/tags/$tag" "refs/tags/$tag^{}") || rc=$?
  case $rc in
    0) printf '%s\n' "$refs" | awk -v t="refs/tags/$tag" '$2 == t {base=$1} $2 == t "^{}" {peeled=$1} END {if (peeled != "") print peeled; else print base}' ;;
    2) return 2 ;;
    *) echo "could not resolve remote tag $tag" >&2; exit 1 ;;
  esac
}

# Download the release's assets into a fresh directory and validate them.
download_and_validate() {
  rm -rf "$1"
  mkdir -p "$1"
  gh release download "$tag" --dir "$1"
  validate "$1" yes
}

# The staged archives plus a SHA256SUMS over them, validated.
prepare_staged() {
  validate "$staged" no
  (cd "$staged"; sha256sum mind-*.tar.gz > SHA256SUMS)
  validate "$staged" yes
}

tag_status=0
tag_sha=$(tag_commit) || tag_status=$?
case $tag_status in
  0) [[ "$tag_sha" == "$GITHUB_SHA" ]] || { echo "$tag points to $tag_sha, expected $GITHUB_SHA" >&2; exit 1; } ;;
  2) ;;
  *) exit 1 ;;
esac

publish_draft=no
if draft=$(gh release view "$tag" --json isDraft --jq .isDraft 2>"$scratch/mind-release-error"); then
  if [[ $draft == true ]]; then
    echo "$tag is an unpublished draft; re-uploading its assets"
    prepare_staged
    gh release upload "$tag" "$staged"/*.tar.gz "$staged/SHA256SUMS" --clobber
    publish_draft=yes
  else
    [[ $tag_status == 0 ]] || { echo "release $tag exists but its tag is missing" >&2; exit 1; }
    echo "$tag exists; preserving published assets"
  fi
else
  if ! grep -q 'release not found' "$scratch/mind-release-error"; then
    cat "$scratch/mind-release-error" >&2
    exit 1
  fi
  prepare_staged
  bash resources/changelog-section.sh "$VERSION" > "$scratch/mind-release-notes"
  gh release create "$tag" "$staged"/*.tar.gz "$staged/SHA256SUMS" --draft --target "$GITHUB_SHA" --title "$tag" --notes-file "$scratch/mind-release-notes"
  publish_draft=yes
fi

if [[ $publish_draft == yes ]]; then
  # Publish only what was uploaded in full: a short or altered upload stops here
  # with the release still a draft, and a re-run replaces it.
  download_and_validate "$scratch/mind-draft-assets"
  gh release edit "$tag" --draft=false
fi

# Recheck after publishing as well, including a tag created concurrently.
tag_sha=$(tag_commit) || { echo "could not resolve published tag $tag" >&2; exit 1; }
[[ "$tag_sha" == "$GITHUB_SHA" ]] || { echo "$tag points to $tag_sha, expected $GITHUB_SHA" >&2; exit 1; }
download_and_validate "$published"

# The bytes consumed here, which the Homebrew formula pins, must be the ones the
# release workflow built: the checksums above only show the release agrees with
# itself.
for archive in "$published"/mind-*.tar.gz; do
  gh attestation verify "$archive" --repo "$GITHUB_REPOSITORY" \
    --signer-workflow "$GITHUB_REPOSITORY/.github/workflows/release.yml" >/dev/null || {
    echo "no valid build provenance for $archive" >&2
    exit 1
  }
done

#!/usr/bin/env bash
# Called by release-please (the `bump_script` input) with the new version as $1, from
# the repository root: keeps Cargo.toml and Cargo.lock in step with the release so
# `cargo publish --locked` publishes exactly the version in RELEASE-NOTES.md.
# Plain awk/sed on purpose: the release job runs on Alpine, without cargo.
set -euo pipefail

VERSION="${1:?usage: set-version.sh MAJOR.MINOR.PATCH}"
if ! [[ "${VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "set-version.sh: '${VERSION}' is not MAJOR.MINOR.PATCH" >&2
  exit 1
fi

NAME="$(sed -n '/^\[package\]/,/^\[/{s/^name *= *"\(.*\)"/\1/p;}' Cargo.toml | head -1)"
[ -n "${NAME}" ] || { echo "set-version.sh: no package name in Cargo.toml" >&2; exit 1; }

# Cargo.toml: the `version = "..."` line inside [package].
awk -v v="${VERSION}" '
  /^\[/ { in_package = ($0 == "[package]") }
  in_package && !done && /^version *= *"/ { print "version = \"" v "\""; done = 1; next }
  { print }
  END { if (!done) exit 1 }
' Cargo.toml > Cargo.toml.new || { rm -f Cargo.toml.new; echo "set-version.sh: no version in [package]" >&2; exit 1; }
mv Cargo.toml.new Cargo.toml

# Cargo.lock: the version line right after `name = "<this crate>"`.
awk -v n="${NAME}" -v v="${VERSION}" '
  $0 == "name = \"" n "\"" { print; getline; if ($0 ~ /^version = /) { print "version = \"" v "\""; done = 1; next } }
  { print }
  END { if (!done) exit 1 }
' Cargo.lock > Cargo.lock.new || { rm -f Cargo.lock.new; echo "set-version.sh: ${NAME} not found in Cargo.lock" >&2; exit 1; }
mv Cargo.lock.new Cargo.lock

# Verify both files now agree.
grep -q "^version = \"${VERSION}\"$" Cargo.toml
grep -A1 "^name = \"${NAME}\"$" Cargo.lock | grep -q "^version = \"${VERSION}\"$"
echo "set ${NAME} to ${VERSION} in Cargo.toml and Cargo.lock"

#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd -- "$script_dir/.." && pwd -P)"
sidecar_dir="$repo_root/sidecars/cockpit-cliproxy"
vendored_dir="$sidecar_dir/third_party/CLIProxyAPI"
vendored_module="github.com/router-for-me/CLIProxyAPI/v7"
focused_package="./internal/runtime/executor/helps"
test_prefix="TestCockpitConnectRetry_"

fail() {
  printf 'ERROR: %s\n' "$*" >&2
  exit 1
}

cross_platform=0
[[ $# -le 1 ]] || fail "usage: $0 [--native-only|--cross-platform]"
case "${1:-}" in
  ''|--native-only) ;;
  --cross-platform) cross_platform=1 ;;
  --help|-h)
    printf 'Usage: %s [--native-only|--cross-platform]\nDefault: verify the native host only.\n' "$0"
    exit 0
    ;;
  *) fail "unknown option: $1" ;;
esac

if ! command -v go >/dev/null 2>&1; then
  printf 'ERROR: Go is not available; no Go tests or build were run.\n' >&2
  exit 127
fi
go_bin="$(command -v go)"
# Local verification must not inherit a cross-compilation target.
native_goos="$("$go_bin" env GOHOSTOS)"
native_goarch="$("$go_bin" env GOHOSTARCH)"
export GOOS="$native_goos" GOARCH="$native_goarch"
# Verify these two modules exactly as checked in, even if the caller has an
# unrelated GOWORK setting or a parent workspace is added later.
export GOWORK=off

[[ -f "$sidecar_dir/go.mod" ]] || fail "outer sidecar go.mod is missing"
[[ -f "$vendored_dir/go.mod" ]] || fail "vendored CLIProxyAPI go.mod is missing"

artifact_tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/ct-connect-retry-verify.XXXXXX")"
cleanup() {
  rm -rf -- "$artifact_tmp_dir"
}
trap cleanup EXIT

printf '[1/8] Validate the outer module uses the local vendored replacement\n'
replace_dir="$({
  cd -- "$sidecar_dir"
  "$go_bin" list -mod=readonly -m -f '{{with .Replace}}{{.Dir}}{{end}}' "$vendored_module"
})"
[[ -n "$replace_dir" ]] || fail "$vendored_module has no replace directive in $sidecar_dir/go.mod"
if [[ "$replace_dir" = /* ]]; then
  replace_abs="$(cd -- "$replace_dir" 2>/dev/null && pwd -P)" ||
    fail "replacement directory does not exist: $replace_dir"
else
  replace_abs="$(cd -- "$sidecar_dir/$replace_dir" 2>/dev/null && pwd -P)" ||
    fail "replacement directory does not exist: $replace_dir"
fi
[[ "$replace_abs" == "$vendored_dir" ]] ||
  fail "replace resolves to $replace_abs, expected $vendored_dir"
vendored_declared_module="$(awk '$1 == "module" { print $2; exit }' "$vendored_dir/go.mod")"
[[ "$vendored_declared_module" == "$vendored_module" ]] ||
  fail "vendored module declares $vendored_declared_module, expected $vendored_module"

printf '[2/8] Compile and enumerate the focused acceptance tests\n'
if ! test_list="$({
  cd -- "$vendored_dir"
  "$go_bin" test -mod=readonly "$focused_package" -list "^${test_prefix}" -count=1
} 2>&1)"; then
  printf '%s\n' "$test_list" >&2
  fail "could not enumerate $test_prefix tests"
fi
printf '%s\n' "$test_list"

test_number=1
while ((test_number <= 22)); do
  printf -v test_id '%02d' "$test_number"
  expected="${test_prefix}T${test_id}"
  if ! grep -Eq "^${expected}([^0-9].*)?$" <<<"$test_list"; then
    fail "missing acceptance test ${expected}<Description>"
  fi
  ((test_number += 1))
done

printf '[3/8] Run focused vendored acceptance tests\n'
(
  cd -- "$vendored_dir"
  "$go_bin" test -mod=readonly "$focused_package" -run "^${test_prefix}" -count=1
)

printf '[4/8] Run the focused race-detector pass separately\n'
(
  cd -- "$vendored_dir"
  "$go_bin" test -mod=readonly -race "$focused_package" -run "^${test_prefix}" -count=1
)

printf '[5/8] Test the affected package in the vendored CLIProxyAPI module\n'
(
  cd -- "$vendored_dir"
  "$go_bin" test -mod=readonly "$focused_package" -count=1
)

if ((cross_platform)); then
  printf '[6/8] Cross-compile the affected tests for Linux and Windows\n'
  (
    cd -- "$vendored_dir"
    CGO_ENABLED=0 GOOS=linux GOARCH=amd64 "$go_bin" test -mod=readonly -c \
      -o "$artifact_tmp_dir/helps-linux-amd64.test" "$focused_package"
    CGO_ENABLED=0 GOOS=windows GOARCH=amd64 "$go_bin" test -mod=readonly -c \
      -o "$artifact_tmp_dir/helps-windows-amd64.test.exe" "$focused_package"
  )
else
  printf '[6/8] Native-only mode: skip cross-platform compilation\n'
fi

printf '[7/8] Test the outer cockpit-cliproxy module\n'
(
  cd -- "$sidecar_dir"
  "$go_bin" test -mod=readonly ./... -count=1
)

printf '[8/8] Build a disposable, CGO-disabled sidecar binary\n'
build_output="$artifact_tmp_dir/cockpit-cliproxy"
(
  cd -- "$sidecar_dir"
  CGO_ENABLED=0 "$go_bin" build -mod=readonly -trimpath -ldflags='-s -w' -o "$build_output" .
)
[[ -f "$build_output" ]] || fail "sidecar build did not create $build_output"

printf 'PASS: CT-CONNECT-RETRY-001 v1.0 verified on %s/%s; ' "$GOOS" "$GOARCH"
if ((cross_platform)); then
  printf 'Linux/Windows test compilation also passed; '
else
  printf 'cross-platform checks skipped; '
fi
printf 'no binary was installed and no process was restarted.\n'

#!/usr/bin/env bash
set -euo pipefail

retry_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
retry_manifest="$retry_root/tests/codex-native-connect-retry/Cargo.toml"
export CARGO_TARGET_DIR="$retry_root/target/local-app-cache/native-connect-retry-harness/target"

retry_listing="$(cargo test --offline --locked --manifest-path "$retry_manifest" --lib -- --list)"
retry_cases=(
  native_setup_retry_accepts_only_concrete_transient_causes
  native_setup_retry_dns_nxdomain_wins_over_timeout
  native_setup_retry_permanent_tls_wins_over_timeout
  native_setup_retry_keeps_system_localhost_resolution
  native_real_tls_setup_retries_before_one_h2_post
  native_final_setup_failure_never_polls_and_drops_body_once
  native_tls_certificate_failure_is_permanent
  native_sent_post_is_never_replayed_by_setup_retry
  native_non_opted_in_transport_keeps_one_setup_attempt
  native_h2_preface_failure_uses_same_three_attempt_budget_before_body
  native_h2_cancellation_drops_partial_dispatcher_and_unread_body
  native_h2_eof_after_setup_send_boundary_is_not_replayed
  native_h2_setup_deadline_spans_attempts_and_backoff
  native_h2_non_chatgpt_and_upgrade_requests_do_not_enter_retry
  native_h2_drop_response_closes_its_dedicated_active_connection
  native_h2_cancel_before_poll_does_not_connect_or_consume_body
  native_setup_deadline_wins_when_retry_backoff_is_also_ready
  native_setup_returns_original_last_error_and_stops_on_unknown
)
for retry_case in "${retry_cases[@]}"; do
  if [[ "$retry_listing" != *"::${retry_case}: test"* ]]; then
    printf 'Missing Native setup retry acceptance case: %s\n' "$retry_case" >&2
    exit 1
  fi
done
cargo test --offline --locked --manifest-path "$retry_manifest" --lib

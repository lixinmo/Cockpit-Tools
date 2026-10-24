# Native provider reasoning metadata and manual configuration

## Scope and precedence

Ordinary API-key providers no longer depend on a GPT display alias to populate
reasoning choices. Model inventory, account/key scope, native protocol/tool
capabilities and Credits admission remain separate from these parameter facts.

The effective order is:

1. Explicit provider policy for this profile, account, endpoint and credential.
2. Explicit instance model policy (where configured).
3. Sanitized capability fields returned by the upstream model-list endpoint.
4. Previously verified model information, including the official StepFun model
   limits and the two-level Step 3.5 Flash 2603 release.
5. Six GPT-compatible choices: low, medium, high, xhigh, max, ultra.

The last case is explicitly a compatibility fallback, not a claim that the
provider implements those intensities. Unknown capability is distinct from a
declared empty list (no adjustable effort). Unknown facts are omitted from the
public model-info fields, so a second gateway cannot mistake unknown for an
explicit refusal. Existing dedicated subscription, ChatGPT Plan and specialized
provider protocol adapters keep their original validation paths.

## Discovery

The existing Get Models operation reads model IDs and optional explicit effort,
default, context and modality facts. Accepted forms include snake_case,
camelCase, ModelInfo fields, effort-object arrays and registry thinking levels.
A boolean saying that reasoning exists, or a supported-parameters list mentioning
reasoning_effort, does not establish an effort range. Conflicting/malformed
capabilities do not manufacture a supported range. Visual facts are retained;
the existing image permissions and routing settings still control image access.

Provider activation refreshes metadata with a four-second timeout. A successful
result is reused for one hour; failure has a short backoff and retains the last
valid facts. Model requests do not perform this discovery or add a generation
retry. Cache scope includes the exact endpoint and credential hash. Cache files
contain allowlisted facts only, with bounded body/model counts, private atomic
writes and no credentials, complete responses, conversations or logs. Cross-origin
redirects are not followed. All returned IDs still pass existing account/key
visibility checks before appearing in a gateway catalog.

## Request mapping

The gateway resolves and authorizes the real account/model before adjusting an
effort. It keeps the authorized client model name separately, so aliases and
namespaces use the same policy in the menu and in the outgoing request.

For a known range, exact values are preserved. xhigh and max retain the Go
adapter's cross-provider preference for each other; otherwise the nearest
supported intensity is selected, with the lower value winning a tie. The order
includes minimal and ultra. Values above the highest supported intensity end at
that highest intensity. Examples:

| Actual range | Requested value | Sent value |
| --- | --- | --- |
| low / medium / high | xhigh, max or ultra | high |
| low / high | medium | low |
| low / high / max | xhigh | max |
| low / high / ultra | max | ultra |
| Unknown | ultra | ultra, without claiming a known maximum |

none and auto are separate controls and are never converted into increased
reasoning. A real auto capability is preserved on the wire, although auto is
not a Codex menu enum. When no adjustable effort is explicitly supported,
nonempty intensity requests fail before generation; a no-effort/automatic
request omits the unsupported parameter. Tools, messages, encrypted history,
model identity, continuation and authorization are not rewritten by this mapper.

## Manual settings

Model Management in an API-provider preview reads that provider's actual model
inventory. It supports inherited settings, an explicit supported-level list,
an explicit no-adjustable-effort declaration and a default within that range.
Manual declarations can add previously unknown levels, including none/minimal;
they are no longer limited to an intersection with a GPT template.

Provider policy is stored separately from the instance's subscription-model
policy and is scoped by profile, account, endpoint and credential fingerprint.
Editing an inactive provider cannot replace the active GPT profile or another
provider's catalog. A saved active policy republishes only an attached matching
Native catalog without restarting its listener. Model additions/deletions still
belong to the provider inventory and cannot be used to widen the account grant.
Removed-model rules remain dormant. Existing instance/API-service import/export
retains its original scope; the new provider-specific editor does not invoke
those global mutations.

Menu metadata, on-disk catalog, HTTP catalog and request mapping share these
sources and precedence. Mixed eligible accounts expose common effort choices,
with defaults in that intersection and conservative context limits.

## Verification

Rust compilation and application packaging must run in Cloud Build on a standard
Intel Mac runner. Synthetic regressions cover metadata parsing and cache scope,
real GET /models discovery and outgoing Chat parameters over JSON/SSE/WebSocket,
unknown/empty/auto cases, aliases/namespaces, scoped manual edits, inactive-profile
isolation, policy disabling, inventory removal and mixed-account intersection.
Existing empty-Go-history, tool discovery, Credits, Auto Reset and connection
retry suites remain in the cloud gate.

`scripts/verify-native-provider-history.cjs --check-effort-mapping` uses the
cloud-built bounded driver and a read-only saved StepFun account. It exercises
fresh and resumed real Codex tool loops, a temporary low-only Step 3.7 policy,
and verifies that explicit ultra requests reach the provider as Step 5 high or
manually configured Step 3.7 low. Personal profiles and conversations never enter
Actions. Build, actual acceptance, installation and cleanup results must be
recorded after completion; no pending check is a successful validation.

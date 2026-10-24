# Native provider reasoning metadata and manual configuration

## Scope and precedence

Ordinary API-key providers no longer depend on a GPT display alias to populate
reasoning choices. Model inventory, account/key scope, native protocol/tool
capabilities and Credits admission remain separate from these parameter facts.

The order for client menu choices and defaults is:

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

## Effort forwarding: provider semantics take precedence

The generic API-key path preserves an effort already supplied by the caller.
The Responses-to-Chat adapter changes `reasoning.effort` to `reasoning_effort`,
but does not change its value. A native/displayed level list is not an accepted
input-alias list and does not define a server mapping. Consequently there is no
generic nearest-level, upper-bound or xhigh/max rewrite, no range-based rejection,
and no deletion based only on an empty metadata list. Explicit none, auto and
null remain explicit inputs. The upstream decides whether a value is accepted,
mapped, ignored or rejected; an error is not retried with a guessed replacement.

An absent effort remains absent. Saved defaults are published to the client
catalog for the client to select; the gateway does not inject a parameter when
the caller omitted it. A low-only menu is therefore not a rule that rewrites
explicit high or ultra to low. Model permissions, Credits, tools, history and continuation
protections remain independent and unchanged. Existing specialized protocol
adapters retain their own verified handling rather than receiving another
generic preprocessing layer.

### Why the earlier generic mapping was withdrawn

The 8a349790 implementation inferred request conversion from native ranges.
Although its tests verified that those conversions occurred, vendor documents
show that the inference is not valid generally. For the same low/high/max range,
DeepSeek explicitly maps medium and xhigh to high, whereas the former generic
rules produced low and max respectively. The corrective implementation removes
that inference instead of adding another guessed vendor table.

Official evidence checked on 2026-10-24:

| Service / scope | Documented behavior |
| --- | --- |
| [DeepSeek thinking](https://api-docs.deepseek.com/guides/thinking_mode/) | minimal/low → low; medium/high/xhigh → high; max/ultra → max. Chat uses reasoning_effort, Messages uses output_config.effort, Responses uses reasoning.effort. |
| [Step 5 via Claude Code / Messages](https://platform.stepfun.com/docs/zh/guides/developer/integrations/claude-code) | Step's server maps xhigh/max to high. Its general API references list low/medium/high; the cited page does not establish an ultra rule for Chat or Responses. |
| [GLM thinking](https://docs.bigmodel.cn/cn/guide/capabilities/thinking) | GLM-5.3 standard API accepts low/high/max and rejects other values. Coding Plan provides additional compatibility aliases, including medium → high. Endpoint/channel matters. |
| [Qwen via Chat](https://help.aliyun.com/zh/model-studio/qwen-api-via-openai-chat-completions) | Listed Qwen3.8 models use low/medium/xhigh; minimal → low and high/max → xhigh. The page defines numeric budgets as well; these cannot be inferred by list position. |
| [MiniMax Responses](https://platform.minimax.io/docs/api-reference/responses-create) | M3.1-Flash supports five documented effort levels; on M3 a non-none effort only enables thinking, while M2.x accepts but ignores none. A field's meaning depends on the model. |
| [Kimi effort](https://platform.kimi.ai/docs/guide/use-reasoning-effort) | K3/K3.1 Pro document low/high/max. No universal server alias table for medium/xhigh/ultra was established by these pages. |

DeepSeek's [model-list schema](https://api-docs.deepseek.com/api/list-models/)
uses `effort.supported_levels` and `effort.default_level`; discovery reads those
facts without deriving alias conversions. Provider-specific rules must be tied
to endpoint/protocol/model version; accepting a field or returning HTTP 200
alone does not prove which computation setting was used.

### Historical Go path: library rules versus the actual provider gateway

The pre-Native checkpoint `54012972cdc3b04b5b79b638b9c1150ac265fa25` distinguishes
two paths. CLIProxyAPI's generic thinking executor contains nearest-level
clamping (ties prefer lower) and xhigh/max compatibility, with conditions on
provider/protocol families. The old StepFun provider gateway did **not** call
that executor: `provider_gateway.go` invokes the Responses-to-Chat converter
directly, constructs its own HTTP request and sends it through the identity
client. Ordinary API-account automatic routing enters the same direct gateway.

That converter only trims/lowercases `reasoning.effort` and copies it to
`reasoning_effort`. It does not consult a level list, pair levels by position,
or cap ultra at high. Thus the old StepFun gateway sent ultra as ultra; whether
the vendor accepted, ignored or normalized it cannot be established from a 200
response alone. The positional allocation elsewhere in old Cockpit assigned
**model-name aliases**, not effort levels.

The first Native patch borrowed selected library rules rather than reproducing
that old StepFun path. That generic conversion has now been removed from the
corrective source. Earlier descriptions implying that the old StepFun gateway
itself already ran the library's clamping algorithm were inaccurate.

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

Menu metadata, on-disk catalog and HTTP catalog share these sources and
precedence. Their choices are not a request conversion table. Mixed eligible
accounts expose common effort choices,
with defaults in that intersection and conservative context limits.

## Verification

Rust compilation and application packaging must run in Cloud Build on a standard
Intel Mac runner. Synthetic regressions cover metadata parsing and cache scope,
real GET /models discovery and outgoing Chat parameters over JSON/SSE/WebSocket,
unknown/empty/auto cases, aliases/namespaces, scoped manual edits, inactive-profile
isolation, policy disabling, inventory removal and mixed-account intersection.
Existing empty-Go-history, tool discovery, Credits, Auto Reset and connection
retry suites remain in the cloud gate.

`scripts/verify-native-provider-history.cjs --check-effort-passthrough` uses the
cloud-built bounded driver and a read-only saved StepFun account. It exercises
fresh and resumed real Codex tool loops and a temporary low-only Step 3.7 menu.
Explicit high must remain high, including for that low-only menu, and an absent
effort must remain absent. The saved low menu/default is checked independently.
Synthetic servers separately verify their
own alias mapping and strict rejection without a guessed gateway rewrite.
Personal profiles and conversations never enter
Actions. Build, actual acceptance, installation and cleanup results must be
recorded after completion; no pending check is a successful validation.

## Earlier validation and installation — 2026-10-24 (conversion superseded)

Production App and probe commit: `8a34979007b29beff1286386710a015ef0c136b0`.
Full Intel Mac Cloud Build:
[40085427270](https://github.com/lixinmo/Cockpit-Tools/actions/runs/40085427270).

The final build passed TypeScript checking, all 1,645 frontend tests, 57 packaging
tests, 10 build-script tests, all locale checks and 275 Native/account regressions
(zero failures, one intentionally ignored profile exporter). Native setup retry
passed 18 tests, Credits 31, Auto Reset 71, and Go `CT-CONNECT-RETRY-001` v1.0
passed on linux/amd64. Application and Rust checks ran only on standard hosted
runners. Build-status polling was at least five minutes apart after the initial
snapshot.

Earlier gates caught and repaired two test-library compatibility errors, a
missing provider binding in the existing preview test harness, missing locale
strings, and a real maintenance-path mismatch. The latter is fixed by applying
the final provider policy before the single catalog write, rather than writing
a base catalog and rewriting it afterward. The unchanged byte/cache assertions
remain, with an additional conflicting instance/provider policy case. An alias
now inherits the actual model's explicit context/effort policy; the regression
checks all four effective fields rather than retaining a stale template limit.

A bounded live StepFun model-list read returned model IDs without effort ranges.
It was not treated as a declaration of three or six levels. Synthetic upstreams
in Actions verify discovery of actual ranges, missing versus explicitly empty
ranges, credentials/endpoints isolated in the cache, and JSON/SSE/WebSocket
generation requests with the expected final parameters.

### Actual clients and StepFun

Both installed client backends used the exact cloud-built Native host and its
production client-profile writer. No manual wire/auth/model-catalog override
was supplied to the client. Each ran three turns in the same temporary session:
fresh Step 5, Step 5 resumed with an injected synthetic old-Go empty reasoning
field, then Step 3.7 resumed with a provider-scoped low-only policy.

| Client | Native 200 responses for tool turns | Successful shell operations | Successful apply_patch operations |
| --- | --- | --- | --- |
| Standalone CLI 0.162.1 | 13 | 7 | 3 |
| Desktop-bundled CLI 0.164.0-alpha.11.1 | 12 | 6 | 3 |

Every turn verified paired tool IDs/results, the same session identity, the
selected raw model, exact fixture contents and the exact final random value.
Each client then passed two additional real requests: explicit ultra reached
Step 5 as high, and the temporarily customized Step 3.7 as low. These checks
combine allowlisted prepared-effort evidence, the verified Chat conversion path
and successful real upstream responses. They establish parameter transport and
tool operation, not equivalence of reasoning quality between different models.

An initial standalone attempt completed the tool operations and included the
correct value in its final answer, but added explanatory text and failed the
exact-answer check. The test prompt was made explicit about output format; no
assertion was relaxed. The two complete runs above passed afterward.

The selected account snapshot, account index, provider store, Native settings,
storage key and original client configuration hashes were unchanged during each
completed run. Manual low-only policies and all test conversations belonged to
temporary profiles and were removed. The user's actual Step 3.7 was not left
restricted to low. This is actual-client acceptance through production code,
not an automated click-through of the desktop GUI or replay of private history.

### Deployment, MCP cleanup and provenance

The verified App was installed at `/Applications/Cockpit Tools.app` and restarted.
Both binaries are x86_64; signatures, manifest hashes and commit provenance were
checked. The exact new process passed Ready/version and two Pong checks five
seconds apart. Native/prefer_regular remained active. Version remains 1.3.73;
the commit and hashes identify the personal build.

Only the obsolete local `codex` MCP registration targeting the removed
`codex mcp-server` command was removed. The rest of the parsed client configuration
was identical, the MCP listing parsed successfully, and the obsolete entry was
still absent after application restart. This cleanup is independent of provider
effort handling. Existing client processes must reload their configuration to
consume changed catalogs/MCP settings; no hand-edited model file is required.

SHA-256:

- App archive: `0d91c022f202dffa2d37c82281c4794950e08d5cc7cc6d8efa27a5743f1ba129`.
- Main executable: `b4adba631f6ed42529ead2fed1d2e0669af8657d8ee790445d77b3fb70d9279d`.
- Matching Go sidecar: `9c69d7252afcadc39287e8f8e1cddd44690a0348b8be13bb36fd04fecfe1c394`.
- Native probe: `556df140d673376fd7d9074ea85261abf72f3813c2e2b9294ed5d460ad760552`.
- Completed acceptance harness: `230fe198c3f6402855db43073d635e436fe3b232cd7f72f0a6ce723dcd46371f`.

Cleanup removed 473,625,706 bytes of App/probe downloads, extraction copies,
public CI logs, safe acceptance summaries and temporary helpers. Including the
previous App removed after successful health checks, 654,209,578 bytes (about
0.61 GiB) were removed. No temporary acceptance profiles or project build/cache
directories remained. Runtime account/configuration data, shared toolchains,
source, Git history/remotes and the verified current App were retained. No
private configuration or conversations were uploaded to Actions.

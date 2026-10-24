# Native provider reasoning metadata and manual configuration

## Scope and precedence

Ordinary API-key providers no longer depend on a GPT display alias to populate
reasoning choices. Model inventory, account/key scope, native protocol/tool
capabilities and Credits admission remain separate from these parameter facts.

The order for client menu choices and defaults is:

1. Explicit provider policy for this profile, account, endpoint and credential.
2. Explicit instance model policy (where configured).
3. Sanitized capability fields returned by the upstream model-list endpoint.
4. Six GPT-compatible choices: low, medium, high, xhigh, max, ultra.

The generic path does not infer effort choices from provider hostnames or model
names. The previously added StepFun three-level/two-level tables and the later
retained StepFun context-window table have been removed. StepFun context now
uses explicit account/profile settings or actual upstream metadata; without
either, it uses the generic 128,000-token client fallback and 115,200-token
compaction threshold. That fallback is not advertised as an upstream capability.
Pre-existing dedicated provider adapters are unchanged.
Reading vendor documentation is a human activity, not runtime discovery; any
resulting user override belongs in Model Management's scoped profile policy,
not another built-in effort table.

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
The endpoint is not required to return these extra fields. The backend caches
them, when present, before returning IDs to the UI. A list containing only model
IDs cannot establish effort choices, and does not trigger documentation scraping
or guessed vendor mappings.
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

The old client menus were not uniformly six levels: at both `54012972` and
`cf4df701`, the borrowed Sol/Terra templates offered six choices, Luna offered
five, and the unknown-model fallback offered low/medium/high/xhigh. The current
generic six-choice fallback removes that dependence on the borrowed template;
it is not an exact restoration of every old menu. Existing xAI-specific Grok
normalization and GPT thinking-suffix handling remain separate official paths.

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

## Corrected forwarding validation and installation — 2026-10-24

This record supersedes the 8a349790 request-conversion behavior above.
Production App/probe commit: `9014a4f7b5a04323ec4facd9a909d1ef133f396e`.
Full Intel Mac Cloud Build:
[40097767822](https://github.com/lixinmo/Cockpit-Tools/actions/runs/40097767822).

The build passed 273 Native/account regressions, zero failures, one intentionally
ignored exporter; 1,645 frontend tests; 57 packaging and 10 build-script tests;
typechecking and all locales. Native retry (18), Credits (31), Auto Reset (71)
and Go CT-CONNECT-RETRY-001 v1.0 also passed. The obsolete mapper unit tests were
removed with the mapper; the JSON/SSE/WebSocket integration matrix now verifies
the real requirement: original values reach the synthetic supplier, whose own
documented-style mapping or strict rejection determines the result. It covers
medium/xhigh/ultra, null/legacy values, missing parameters, low-only menus,
unchanged tools/history, and namespace denial before any upstream request.

An independent local loopback check used both installed CLIs, without a real
provider, to confirm that summary-capability flags do not suppress an explicit
high effort. It also confirmed that a client with no effort setting chooses and
sends low from a low-default catalog. Six captured requests were stopped with
an intentional synthetic 400; these establish client field behavior, not model
generation. Their temporary configurations and helpers were deleted.

### Real StepFun acceptance

The exact cloud-built Native host and production catalog writer were used with
a read-only account snapshot. Each client completed fresh Step 5, resumed Step
5 with a synthetic old-Go empty reasoning item, then resumed Step 3.7 in the
same conversation. Step 3.7 had a temporary low-only menu/default.

| Client | Native 200 responses for tool turns | Successful shell / patch operations |
| --- | --- | --- |
| Standalone CLI 0.162.1 | 13 | 7 / 3 |
| Desktop-bundled CLI 0.164.0-alpha.11.1 | 12 | 6 / 3 |

All accepted turns checked call/result pairing, a real custom patch call,
same-session identity, raw model identity, exact file contents and the exact
final random value. Both clients themselves selected low for Step 3.7; the
gateway preserved it. Each also completed these three real control requests:

| Model / client menu | Caller effort | Outgoing effort | HTTP status |
| --- | --- | --- | --- |
| Step 5, normal menu | high | high | 200 |
| Step 3.7, low-only menu | high | high | 200 |
| Step 3.7, low-only menu | absent | absent | 200 |

The absence check is not a claim about the provider's hidden default intensity.
These controls prove that menu/default configuration does not rewrite an
explicit request or inject an absent parameter. No undocumented StepFun ultra
behavior was assumed or required to pass.

A preliminary desktop run failed the combined call-count/custom-call/pairing
criterion and was not accepted. Its retained safe summary did not distinguish
which subcondition failed, so no specific corruption or model behavior is
claimed. Structural diagnostic counts were added without relaxing assertions;
the complete desktop rerun passed every check. Each completed run preserved
hashes of the selected account, index, provider store, Native settings, storage
key and original client config. All temporary policies/conversations were removed.

The matching App and sidecar were verified, installed and restarted. The exact
new process passed Ready/version and two Pong checks five seconds apart.
Native/prefer_regular remained active. The obsolete codex MCP entry remains
removed. Existing client choices/defaults continue to come from their catalog;
this correction changes request forwarding, not the already working tool or
history adapters. The desktop-bundled backend was exercised as a CLI; this is
not a claim of an automated GUI click-through.

SHA-256:

- App archive: `d47ad017d9f041c56940b1702cad9774fb90997ca4b142ab070e0d4af79c57dd`.
- Main executable: `2df4448574108c70728d4164b9443f15e19fbb6d7dd99687a4fd5f3f520064c2`.
- Matching Go sidecar: `e841edeb9727705cf23929d5de7c62c989c0a001ecbc112fea6cdb17e8bda124`.
- Native probe: `82ab889b20b78c55831612f864416f88cf5280f02c13521c3e45100eb3661a23`.
- Completed acceptance harness: `ae1d049f1af93df071807412ad1454be50c179ab53c1d9e6653f08f72a81a2f7`.

Cleanup removed 470,670,226 bytes of downloaded artifacts, extraction copies,
safe summaries and task helpers. Including the replaced App removed after
successful health checks, 652,084,994 bytes (about 0.61 GiB) were removed. No
temporary client profiles or project build/cache directories remained. Sources,
Git history/remotes, shared toolchains, runtime data and the current verified
App were retained. No local application/Rust compilation or private-data upload
was performed.

## Generic menu correction: remove the StepFun effort table

The earlier StepFun low/medium/high and 2603 low/high values were added to Rust
after a human documentation review. They were not returned by runtime discovery
or saved through Model Management. That provider-specific effort table has now
been removed in `2aff526f85575c16d15fec902b875eebbf4cd794`. The context-only
baseline remains, with `cockpit_reasoning_known=false`; existing specialized
adapters are unchanged. No replacement effort settings were written into the
user's profile. Generic menu precedence is explicit settings, actual upstream
metadata, then the six compatibility choices. Forwarding still preserves the
caller's value and preserves absence.

A read-only live GET of the existing StepFun plan's model list returned HTTP
200 and nine models, including Step 5 and Step 3.7, without discoverable effort
fields. No full response, key or private configuration was uploaded. This is
evidence about that response, not a promise that every provider exposes effort
metadata in its model list.

[Cloud Build 40110787830](https://github.com/lixinmo/Cockpit-Tools/actions/runs/40110787830)
completed successfully on the standard Intel runner in approximately 72 minutes.
Frontend, Native/Go contracts and the Native regression packaging gate passed.
The source tests cover generic menus across provider hosts/model names, unknown
facts omitted from public capability declarations, upstream facts overriding
fallbacks, and explicit manual settings taking priority. GitHub's jobs API
intermittently returned HTTP 502 and omitted running-step details; final run
success and both artifact identities were confirmed. Two attempts to retrieve
the complete public log failed at that API, so this record does not reuse or
infer a test count from the previous build. Status polling stayed at least five
minutes apart, without restarting the build.

The downloaded App and probe matched the build commit, checksums, x86_64
architecture and code-signature verification. Acceptance used the production
profile writer, bounded Native listener and read-only original account snapshot:

| Client | Catalog scenario | Real tool/history results |
| --- | --- | --- |
| Codex CLI 0.162.1 | Step 5 generic six/default medium; temporary Step 3.7 low-only/default low | Three same-session turns, 12 successful Native tool requests, six shell commands, three apply_patch calls and nine paired calls/results |
| Desktop backend 0.164.0-alpha.11.1 | Both models generic six/default medium; no manual overrides | Three same-session turns, 15 successful Native tool requests, 12 shell commands, three apply_patch calls and 15 paired calls/results |

Each client started with Step 5, resumed after a synthetic legacy empty encrypted
reasoning item, then selected Step 3.7 in the same session. Every request returned
200; all tool results, file contents and final nonces matched. The standalone
run additionally verified Step 5 high stays high, manual-low Step 3.7 high stays
high, and absent effort stays absent, all with real upstream 200 responses.
The manual menu caused that client to choose low itself. Both runs retained
Native/prefer_regular and the Sidecar prohibition; six original account/config
file hashes remained unchanged. All generated profiles and histories were removed.
The desktop backend was exercised through its shipped CLI, not an automated GUI
click-through. The acceptance harness is committed through `d8a9f107`.

The new App was installed and restarted. Its exact process path and binary hash
matched the verified artifact; Ready/version and two Pongs five seconds apart
passed. Native/prefer_regular remained active and the obsolete codex MCP entry
remained absent. The user's active profile was not StepFun, so it was not switched.
Selecting StepFun again generates the updated managed catalog; an existing Codex
process must reload that catalog. Existing explicit manual choices remain in force.

SHA-256:

- App archive: `a56decd06c2effd14dffc4c09e501073744eca429d50443ac29108b32b6c7653`.
- Main executable: `a489c726cf05902cb6714535030fbc691c5924988baff805edebaa812950200d`.
- Matching Go sidecar: `7b4307ab540b78581cbfbdbe1877e82aee8b9e38ec31f302e2863940295f4eea`.
- Native probe: `eb188d4ac35f9976f4493543bdeea8a4362ee5c6d3ab08b2cc40030ad4c01901`.
- Acceptance harness: `67d80910d25cf0c94388af1bf25b1a7093712bfa5f2ba98359cd3adb77f93ddc`.

Cleanup removed 468,910,321 bytes of downloads, probe, replaced App and task
helpers, including 181,192,304 bytes from the old App. No project build caches or
temporary acceptance profiles remained. Monitoring scripts ran from standard
input and left no files. The current App, source/Git history, shared toolchains
and runtime data were retained. No local compilation or private-data upload occurred.

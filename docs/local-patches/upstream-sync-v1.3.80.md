> Historical integration record. Its optional Go, legacy Reset compatibility and
> provider extensions were subsequently retired by the user. Current scope is
> documented in [Native-only cleanup](native-only-cleanup.md).

# Upstream integration: v1.3.80

## Source and update channel

Integration target: `02498ace` (`upstream/main`, 2026-11-07), including release
`v1.3.80` at `0436a3d4`. The previously installed build reports `1.3.73`.
There are seven intervening official releases (`1.3.74` through `1.3.80`).
The integration preserves both public `main` and the subsequent maintained
`codex/stepfun-native-capability` history; no upstream source snapshot replaces
the repository history.

The fork's Tauri updater intentionally uses `lixinmo/Cockpit-Tools`. Its rolling
`cloud-macos-x86_64` release is a prerelease, not GitHub's latest stable release,
and does not provide a signed updater manifest. Consequently, the stable
`releases/latest/download/latest*.json` endpoints return 404 until a formal fork
release is published. Automatic checks can remain enabled without reporting a
new upstream version. This integration keeps the installation channel on the
fork; it does not install upstream binaries over the maintained policies.

## Resolution decisions

| Area | Upstream changes and retained behavior |
| --- | --- |
| Local projects after account switches | Adopt upstream's account-state/project recovery and safe handling of malformed global state. No second local project-recovery implementation is added. |
| Automatic Reset cards | Use upstream's rule UI, worker and redemption journal. Migrate existing account opt-ins and thresholds without enabling an expiry trigger; honor pending legacy consumption barriers. The old detailed record remains in Git history. |
| Chat provider tools | Use upstream tool discovery and protocol restoration. Preserve only the additional provider capability declarations and legacy-history compatibility that upstream does not cover. |
| Provider catalog and reasoning | Combine upstream catalog refresh, protocol/auth-scheme and image metadata with generic configurable reasoning/context metadata and atomic profile publication. Do not restore provider-specific model heuristics. |
| Credits and API-key budgets | Upstream API-key monetary/concurrency budgets do not replace the local OpenAI Credits admission policy. Both apply at their respective boundaries. |
| Setup-only connection retry | Preserve `CT-CONNECT-RETRY-001` v1.0 and its Native dependency hooks. Review upstream transport recovery separately from the setup-only budget. |
| Go sidecar | Upstream retired the Go implementation in v1.3.76. Retain the requested optional sidecar and reference contract, including the final upstream updates before retirement (`19f00699`). Native remains the default. |
| Build and publication | Retain platform-selectable Cloud Build, one frontend typecheck, contract gates, dependency/incremental caches, short-lived artifacts and rolling downloads. Incorporate upstream formal macOS signing/notarization safeguards. All compilation runs on standard GitHub-hosted runners. |

## Validation

Application source: `e1bce43df171fbb564bef8869ef9ed4211b78841`.
All application checks and packaging passed in
[Cloud Build 42462535936](https://github.com/lixinmo/Cockpit-Tools/actions/runs/42462535936).
Its final publication job failed because the large GitHub comparison response
exceeded the publisher's output buffer; this was not a build or test failure.
The publisher now requests only the comparison status.

[Publication recovery 42466743679](https://github.com/lixinmo/Cockpit-Tools/actions/runs/42466743679)
passed 23 publication/guard tests, revalidated the source run and unchanged App
inputs, and published the exact original artifacts without recompiling. The
recovery and normal publisher share the same concurrency lock. Only publication
tooling and maintenance documentation changed after the application source SHA.

| Verification | Result |
| --- | --- |
| Frontend typecheck and tests | Passed; 2,093 tests |
| Release/cache scripts | 90 tests passed |
| Local-project recovery utility | 13 tests passed |
| Native integration | 2,873 passed, 0 failed; 15 existing opt-in/manual/live-environment tests ignored |
| Native setup retry | 18 tests passed |
| Go reference retry | T01–T22, race detector, affected package, outer tests and disposable native build passed; no cross-platform build requested |
| Credits offline harness | 34 tests passed |
| Reset offline harness | 55 tests passed |
| Intel App and sidecar | Both exclusively `x86_64`; source/version and SHA-256 matched |
| Local bundle inspection | `codesign --verify --deep --strict` passed; ad-hoc signature, no Apple notarization |

The [rolling Intel download](https://github.com/lixinmo/Cockpit-Tools/releases/tag/cloud-macos-x86_64)
now contains only the v1.3.80 archive, provenance and checksum manifest. Its tag
points to the verified application source SHA. The source run's transient
artifacts were deleted after successful publication.

Archive SHA-256:
`452c7fc6f5d81ee8651ee9c520a90007f04f48691fc7e1ba8a035564b6fb5447`.
Main executable SHA-256:
`db218fda1e23c59a0320742a045bf4459415172c1ca09c8c8bd2094f21d697de`.
Matching Go sidecar SHA-256:
`2f9df82491cbf92c43a3625bc453eba6b55c8f85d109c8aef73b31c792806066`.

The cloud retained 5,166,256,653 bytes of deduplicated incremental state
(1,690,334,410 bytes compressed in the cache). Standard hosted runners only were
used. No compilation or real-account acceptance ran on the user's Mac.

The 253,568,685-byte temporary download/extraction was removed after verification.
The checkout has no `target`, `node_modules`, `dist` or `.tmp` build directory;
no obsolete sibling source copy or Downloads artifact was found. Source, Git
history/remotes, shared toolchains, runtime account/configuration data and the
currently installed v1.3.73 App were retained. Installing or restarting the new
App was outside this source synchronization; no real account was switched and
no quota or Reset was consumed.

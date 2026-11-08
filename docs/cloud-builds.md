# 本机构建与手动云端备选

当前政策自 **2026-11-09** 起生效：个人 App 默认在本机构建，先做针对性检查，再构建、校验产物并清理项目生成物；安装后的旧 App 清理以新版启动成功为前提。此前“本机不得编译”“每次推送自动完整云端构建”及保留本地缓存的规定均已被本政策取代。旧构建记录保留为其对应提交的历史证据，Native 重试和 Credits 运行时合约不变。

源码和上游历史继续保留在公开仓库 [lixinmo/Cockpit-Tools](https://github.com/lixinmo/Cockpit-Tools)。日常只构建这台 Intel Mac 的版本；源码继续支持 macOS、Windows 和 Linux。运行时跟随上游，仅使用 Rust Native，不打包 Go 引擎。不新增完整备份。

## 日常本机构建

```sh
# 两个入口都在当前 Mac 构建，不触发 GitHub Actions。
npm run build:app
npm run build:app:local

# 只查看本机构建计划。
npm run build:app -- --dry-run
```

本地入口使用现有 Node/npm、Rust 和 macOS 开发工具链，目标为当前 Mac 的原生架构；这台 Intel Mac 为 `x86_64-apple-darwin`。前端只做一次 typecheck，生成 Native-only `.app` 并校验版本、架构和签名。个人构建采用 ad-hoc 签名，不代表 Apple 公证，也不代替正式签名发布。具体输出路径与执行步骤以启动器计划为准。

先运行与改动直接相关的检查，例如相关 Node 测试、Rust 回归 filter；失败后只重跑相关检查。修改 Native transport 时运行 `scripts/verify-native-connect-retry.sh`，修改 Credits 时运行对应 `tests/codex-credits-policy` harness 及必要的 Native 集成检查。合并上游后仍需审查重试是否叠加。检查修复期间可复用项目编译输出，不为每次查错重新运行完整 Cloud Build。

本地脚本只负责构建，不安装、停止或启动 App。产物通过版本、架构和签名校验后，即清理本项目 `target/`、`node_modules/`、`dist/` 和本轮 scratch，仅在 `.local-app-output/` 保留验证过的 App。当前已安装的旧 App 保持可用；安装新版并确认启动成功后，才删除旧 App 和重复成品。源码、`.git`、全部账号/配置/会话数据及共享工具链必须保留。失败时保留本次诊断及必要项目缓存，供定向修复。

## 手动云端构建与下载

云端是显式选择的备选。打开 [Cloud Build](https://github.com/lixinmo/Cockpit-Tools/actions/workflows/build-matrix.yml)，选择 **Run workflow** 和目标平台。该工作流只保留 `workflow_dispatch`；`main` 推送及 pull request 均不自动触发完整构建。

| `platform` | 成品 |
| --- | --- |
| `macos-x86_64` | Intel Mac，当前自用默认目标 |
| `macos-aarch64` | Apple Silicon Mac |
| `macos-universal` | 同时包含 Intel 和 Apple Silicon 的 Mac App |
| `linux-x86_64` | Linux x86_64 |
| `linux-aarch64` | Linux ARM64 |
| `windows` | Windows x86_64 |
| `all` | 全部上述平台，包括 macOS Universal |

云端启动器只依赖 Node.js、Git 和已登录的 GitHub CLI，无需本地安装构建依赖。

```sh
npm run build:app:cloud
npm run build:app:cloud -- --platform macos-x86_64 --ref main
node scripts/build-cloud.cjs --platform all
node scripts/build-cloud.cjs --dry-run
node scripts/build-cloud.cjs --help
```

默认仓库来自 `origin`，远端 ref 为 `main`；可用 `--repo owner/repo` 和 `--ref branch-or-tag` 指定。云端只读取已推送的提交，本地未提交或未推送的修改不会进入构建。`npm run build:app` 和 `build:app:local` 不再是云端别名。

成功运行提供 `cockpit-tools-<platform>` artifact；Mac 包包含 `.app.tar.gz`、`SHA256SUMS.txt` 和 `provenance.json`。校验 SHA-256 后再解压。受信任的手动 `main` 构建可更新 [Intel Mac 滚动下载](https://github.com/lixinmo/Cockpit-Tools/releases/tag/cloud-macos-x86_64)；其他平台使用 `cloud-<platform>` 预发布标签，见 [Releases](https://github.com/lixinmo/Cockpit-Tools/releases)。分支构建从对应运行的 artifacts 下载。

滚动预发布替换同平台旧文件；临时 artifacts 保留 1 天，成功发布后删除本次临时副本。安装与启动验证完成后清理本机下载归档及重复 App。普通云端包不需要开发者签名私钥，来源信息和 SHA-256 不代表 Apple 公证。

若 App、前端、Native 集成与独立合约检查都已通过，仅滚动发布失败，可使用
[Publish verified Cloud Build](https://github.com/lixinmo/Cockpit-Tools/actions/workflows/publish-cloud-build.yml)，
填入原 Cloud Build 的 run ID。恢复流程核验受信任来源、成功检查、Git 祖先关系和 App 输入未变化后复用原 artifacts，不重新编译；也可恢复此前自动触发的历史构建。源构建之后只允许发布工具和维护文档变化，不能用恢复发布绕过源码验证。

## 云端验证、缓存与正式发布

Cloud Build 在云端运行 TypeScript 检查及相关测试，并验证 Native 同账号连接重试、Credits 保护，以及上游账号、工具调用和重置卡功能。不得因为换到云端而省略 Native 验证，或跳过相关检查规避失败。变更影响的检查必须通过后才能交付。

Rust、npm 缓存按平台、架构和依赖输入复用。macOS Universal 使用同一次运行已经生成的两个架构输入合并，避免额外重复编译。缓存只存放构建依赖和编译结果，不存放任何本机账号或运行配置。只使用公开仓库的免费标准托管 runner，不启用 larger runner、付费服务、付费缓存扩容或收费额度，不为构建设置付款方式。GitHub 自身的容量与保留策略仍适用。

受信任分支的 Rust 依赖缓存也保留失败运行中已完成的依赖编译，方便修正源码后复用；
仍不缓存 workspace App 成品或运行配置，也不会发布未通过验证的包。
下述增量状态缓存也保留失败检查产生的编译状态；Cargo 会重新验证输入，所有测试仍需重跑通过。
普通 Mac 构建先执行 Native 集成回归，再优化和打包 App；测试使用相同目标与
release 依赖，测试 crate 自身保持较低优化等级；回归检查不依赖已有 App。

手动 Cloud Build 为受信任分支的 `workflow_dispatch` 保留编译状态。
既有 Rust 缓存继续提供依赖底座；新的状态缓存只包含 host 与目标架构 release
目录下的 `incremental`，覆盖 App 和 Native 验收程序的增量数据，不重复保存
整套依赖产物。它不包含打包后的 App、bundle、运行配置或测试临时 profile。
Cargo 仍会检查输入并完成必要的编译、链接，Rust 编译器复用未受影响的内部结果。
状态在依赖缓存
恢复后加载，在编译、验证和成品上传尝试之后保存，避免被 Rust 缓存 action 的
收尾清理删除。失败运行只保存通过路径、类型和大小检查的编译状态，不上传或发布失败的 App；
取消的运行不保存，PR 和 tag 不写入该状态缓存。清理仍限制在当前分支、平台和模式。

编译步骤显式开启增量编译，同时保持 release 的 16 个代码生成单元及现有优化
等级。正式 `release.yml` 不使用这层增量状态，也不加载旧的本地快速编译配置。
缓存按平台/架构、编译器、runner 镜像、依赖及编译配置区分，提交号用于保存
新状态；后续提交可以恢复相同配置的最近状态。新分支也可以保存自己的依赖缓存。

单份编译状态上限为 8 GiB 去重后的未压缩文件内容。Rust 会把上一编译会话的
部分产物硬链接到新会话，所以使用设备号与 inode 去重，同时报告路径逻辑总量；
不会删除 Rust 会话或重复计算共享内容，符合缓存 tar 保留硬链接的方式。
超过上限或发现不允许的文件类型时跳过
保存并明确报告，不影响已通过验证的 App。允许 SwiftPM 等工具产生的相对链接，
但目标必须存在且最终仍位于所扫描目录白名单内；不跟随链接扫描或重复计数。
绝对链接、外部链接、断链、循环及缓存根目录的链接仍拒绝。单独的清理任务只处理当前分支、
当前平台和 full/probe 模式的编译状态命名空间，正常保留最新一份；并发运行时
不删除更新运行保存的缓存。依赖缓存、其他分支和正式发布缓存不在其清理范围。
整个仓库仍受 GitHub 默认免费总量和自动淘汰规则约束，不提高付费容量。

仅改缓存计量、清理等维护代码不再使编译输入指纹失效。旧版本的精确兼容前缀
只迁移已知缓存键格式，仍同时匹配实际编译器、平台、镜像、依赖和编译配置；
不回退到任意编译器或跨平台缓存。

首次运行需要建立新状态，可能仍较慢。确认增量生效应查看下一次运行的恢复命中、
增量文件统计以及 Cargo/App/probe 实际耗时；仅看到开关开启不算完成验证。

正式版本沿用 [Release 工作流](https://github.com/lixinmo/Cockpit-Tools/actions/workflows/release.yml)：版本号、变更记录与 `v<version>` 标签必须对应；标签指向的同一个提交必须先通过 Cloud Build。Release 随后构建各支持平台，并使用维护者配置的发布签名 secret 生成可校验的 updater 成品。私钥只放 GitHub Actions Secrets，不能提交到 Git。发布验证和全部平台构建成功后再发布正式版本。日常只下载当前 Mac 的包，其他用户可按需从 Releases 下载。一次 Intel Mac 构建成功不等于其他平台或正式发布已经完成验证。

Fork 用户先在自己的仓库启用 Actions，再使用 Cloud Build 选择平台。启动器读取 fork 的 `origin`，不会固定向原作者仓库提交构建。普通云端包不依赖本机私有数据，也不需要签名 secret。正式发布前应生成自己的 Tauri updater 密钥，在 GitHub Actions Secrets 设置 `TAURI_SIGNING_PRIVATE_KEY` 和需要时的 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`，并更新 `src-tauri/tauri.conf.json` 的 updater 公钥与下载来源；不能使用其他维护者的私钥。本分支的 updater 来源指向 `lixinmo/Cockpit-Tools`。

## 隐私与本机清理

公开的是经过审查的源代码及构建配置。本机账号、配置、会话、请求内容、日志和私人文件不提交、不传入 Actions、不进入缓存或成品。尤其保留并隔离：

- `~/.antigravity_cockpit`、`~/.cockpit_tools` 及开发 profile；这台 Mac 的 `.cockpit_tools` 是指向旧数据目录的兼容链接。
- `~/Library/Application Support/cockpit-tools`、相关 WebKit/应用数据，以及各客户端原有账号目录。
- SSH/GitHub 凭据、`.env`、签名私钥、导出的账号文件、应用生成的备份和任意自定义 `COCKPIT_TOOLS_DATA_DIR`。

本地产物通过版本、架构和签名校验后，可以清理本项目 `target/`、`node_modules/`、`dist/` 和本次 scratch，保留验证过的 App。旧已安装 App 必须等替换版本安装并启动成功后再删除；云端下载归档和其他重复成品也在确认当前 App 可用后清理。记录删除范围及前后大小，不把账号/配置/会话目录当作构建缓存清理。共享 Rust、Node/npm、macOS 开发工具链及其共享缓存保持原状。仓库的 `.git`、补丁说明、源码以及当前 App 必须保留；不要为清理创建完整备份。

## 同步上游

`origin` 为个人公开仓库；`upstream` 为 `https://github.com/jlcodes99/cockpit-tools.git`。公开 `main` 包含上游源码与维护中的补丁，不再把 `main` 当作可丢弃的纯上游镜像。不要用下载的新源码覆盖当前项目，也不要为更新创建完整备份。

```sh
git fetch upstream --tags
git switch main
git merge upstream/main
# 审查并解决冲突，提交合并结果，再推送个人仓库。
git push origin main
```

对于已经公开的分支，优先合并以保留历史。合并时逐项检查 `docs/local-patches/`，继续复用上游协议、continuation 和 quota-reserve 实现；不得增加第二套 plaintext marker 策略，或在切换引擎/身份时绕过 Credits。

每次上游同步后，按照 `same-account-connect-retry.md` 和 `native-connect-retry.md` 检查连接建立重试，避免与上游应用层重试叠加。固定三次尝试、200/400 ms 间隔和禁止发送后重放的约定保持不变。优先使用本机现有工具链运行 `scripts/verify-native-connect-retry.sh`、Credits harness 和改动影响的上游 Native 定向检查；需要完整跨平台验证时再手动选择 Cloud Build。个人 Intel 构建不要求额外发布矩阵，正式发布的所有平台能力保持。最后审查相对 `upstream/main` 的差异，并记录基线、实际检查和 App 验证结果。

当前维护范围见 [Native-only 精简记录](local-patches/native-only-cleanup.md)。旧构建和已退役补丁记录可从 Git 历史读取。

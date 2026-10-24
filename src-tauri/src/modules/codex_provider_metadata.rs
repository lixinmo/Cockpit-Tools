//! Explicit capabilities returned by a provider's model-list endpoint.
//!
//! The cache is scoped to the exact endpoint and credential. It contains only
//! allowlisted model facts, never credentials, response bodies or conversation
//! data. Model facts do not grant access to additional accounts or models.
//! Declared effort levels describe model choices; they are neither a complete
//! list of compatible input aliases nor a rule for rewriting request values.
use crate::models::codex::CodexAccount;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_MODELS: usize = 16_384;
const MAX_MODEL_ID_BYTES: usize = 512;
const CACHE_TTL_MS: u64 = 60 * 60 * 1000;
const FAILURE_BACKOFF: Duration = Duration::from_secs(60);
const CACHE_DIRECTORY: &str = "provider-model-metadata";
const EFFORTS: &[&str] = &[
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra", "auto",
];
static CACHE_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static FAILED_REFRESHES: LazyLock<Mutex<BTreeMap<String, Instant>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
static LOADED_CACHES: LazyLock<Mutex<BTreeMap<PathBuf, LoadedCache>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

struct LoadedCache {
    modified: SystemTime,
    size: u64,
    used: Instant,
    value: Arc<CacheEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderModelMetadata {
    /// None means unknown. Some([]) explicitly declares no adjustable effort.
    /// This menu metadata must not be used to infer a provider's input mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_efforts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_vision: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredModel {
    pub id: String,
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ProviderModelMetadata>,
}

pub struct FetchedModels {
    pub models: Vec<DiscoveredModel>,
    pub latency_ms: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CacheEntry {
    version: u32,
    scope: String,
    fetched_at_ms: u64,
    models: BTreeMap<String, ProviderModelMetadata>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn normalized_base_url(base_url: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(base_url.trim())
        .map_err(|_| "PROVIDER_BASE_URL_INVALID".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("PROVIDER_BASE_URL_INVALID".into());
    }
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(if path.is_empty() { "/" } else { &path });
    url.set_fragment(None);
    Ok(url)
}

/// Match the existing model-list path convention, retaining its base prefix.
pub fn models_url(base_url: &str) -> Result<String, String> {
    let mut url = normalized_base_url(base_url)?;
    let next = format!("{}/models", url.path().trim_end_matches('/'));
    url.set_path(&next);
    url.set_query(None);
    Ok(url.to_string())
}

fn scope(base_url: &str, api_key: &str) -> Result<String, String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Err("MISSING_API_KEY".into());
    }
    let base = normalized_base_url(base_url)?;
    let mut hash = Sha256::new();
    hash.update(b"cockpit-provider-model-metadata-v1\0");
    hash.update(base.as_str().as_bytes());
    hash.update([0]);
    hash.update(key.as_bytes());
    Ok(format!("{:x}", hash.finalize()))
}

fn normalized_effort(value: &Value) -> Option<String> {
    let value = value.as_str()?.trim().to_ascii_lowercase();
    EFFORTS.contains(&value.as_str()).then_some(value)
}

fn effort_list(value: &Value) -> Option<Vec<String>> {
    let items = value.as_array()?;
    if items.len() > EFFORTS.len() * 2 {
        return None;
    }
    let mut result = Vec::new();
    for item in items {
        let effort = if item.is_object() {
            consistent(
                fields(item, &["/effort", "/reasoningEffort", "/reasoning_effort"]),
                normalized_effort,
            )?
        } else {
            normalized_effort(item)?
        };
        if !result.contains(&effort) {
            result.push(effort);
        }
    }
    result.sort_by_key(|effort| EFFORTS.iter().position(|v| *v == effort.as_str()).unwrap());
    Some(result)
}

// Conflicting aliases or a malformed explicit value are unknown, not a guessed
// subset. Null represents absent metadata, not an explicit unsupported feature.
fn consistent<T: Eq>(
    values: impl IntoIterator<Item = Value>,
    parse: impl Fn(&Value) -> Option<T>,
) -> Option<T> {
    let mut result = None;
    for value in values.into_iter().filter(|value| !value.is_null()) {
        let next = parse(&value)?;
        if result.as_ref().is_some_and(|previous| previous != &next) {
            return None;
        }
        result = Some(next);
    }
    result
}

fn fields(item: &Value, paths: &[&str]) -> Vec<Value> {
    paths
        .iter()
        .filter_map(|path| item.pointer(path).cloned())
        .collect()
}

fn positive_window(value: &Value) -> Option<i64> {
    let value = value.as_i64().or_else(|| value.as_str()?.parse().ok())?;
    (value > 0 && value <= 10_000_000_000).then_some(value)
}

pub fn parse_metadata(item: &Value) -> Option<ProviderModelMetadata> {
    let mut levels = fields(
        item,
        &[
            "/supported_reasoning_levels",
            "/supported_reasoning_efforts",
            "/reasoning_efforts",
            "/supportedReasoningLevels",
            "/supportedReasoningEfforts",
            "/reasoningEfforts",
            "/model_info/supported_reasoning_levels",
            "/model_info/supported_reasoning_efforts",
            "/model_info/reasoning_efforts",
            "/modelInfo/supportedReasoningLevels",
            "/modelInfo/supportedReasoningEfforts",
            "/modelInfo/reasoningEfforts",
            "/reasoning/supported_efforts",
            "/reasoning/efforts",
            "/effort/supported_levels",
            "/thinking/levels",
            "/model_info/thinking/levels",
            "/capabilities/reasoning/supported_efforts",
        ],
    );
    // False explicitly declares no adjustable levels. It does not establish
    // whether the endpoint rejects, ignores or otherwise handles an input value.
    // A true flag or a supported-parameters entry does not identify the enum.
    for value in fields(
        item,
        &[
            "/supports_reasoning",
            "/supports_thinking",
            "/supportsReasoning",
            "/model_info/supports_reasoning",
            "/capabilities/reasoning",
            "/reasoning",
        ],
    ) {
        if value == Value::Bool(false) {
            levels.push(serde_json::json!([]));
        }
    }
    if fields(
        item,
        &[
            "/reasoningDisabled",
            "/reasoning_disabled",
            "/model_info/reasoning_disabled",
            "/modelInfo/reasoningDisabled",
        ],
    )
    .into_iter()
    .any(|value| value == Value::Bool(true))
    {
        levels.push(serde_json::json!([]));
    }
    let reasoning_efforts = consistent(levels, effort_list);
    let default_reasoning_effort = consistent(
        fields(
            item,
            &[
                "/default_reasoning_level",
                "/default_reasoning_effort",
                "/defaultReasoningLevel",
                "/defaultReasoningEffort",
                "/model_info/default_reasoning_level",
                "/model_info/default_reasoning_effort",
                "/modelInfo/defaultReasoningLevel",
                "/modelInfo/defaultReasoningEffort",
                "/reasoning/default_effort",
                "/effort/default_level",
            ],
        ),
        normalized_effort,
    )
    .filter(|effort| {
        reasoning_efforts
            .as_ref()
            .is_none_or(|levels| levels.contains(effort))
    });
    let context_window = consistent(
        fields(
            item,
            &[
                "/context_window",
                "/context_length",
                "/contextWindow",
                "/contextLength",
                "/model_info/context_window",
                "/model_info/context_length",
                "/modelInfo/contextWindow",
                "/modelInfo/contextLength",
                "/top_provider/context_length",
            ],
        ),
        positive_window,
    );
    let mut vision = fields(
        item,
        &[
            "/supports_vision",
            "/supportsVision",
            "/model_info/supports_vision",
            "/modelInfo/supportsVision",
            "/capabilities/vision",
        ],
    );
    for value in fields(
        item,
        &[
            "/input_modalities",
            "/inputModalities",
            "/model_info/input_modalities",
            "/modelInfo/inputModalities",
            "/architecture/input_modalities",
        ],
    ) {
        if value.is_null() {
            continue;
        }
        let Some(parts) = value.as_array().filter(|parts| !parts.is_empty()) else {
            vision.push(Value::String("invalid".into()));
            continue;
        };
        if parts
            .iter()
            .all(|part| matches!(part.as_str(), Some("text" | "image" | "audio" | "video")))
        {
            vision.push(Value::Bool(parts.iter().any(|part| part == "image")));
        } else {
            vision.push(Value::String("invalid".into()));
        }
    }
    let metadata = ProviderModelMetadata {
        reasoning_efforts,
        default_reasoning_effort,
        context_window,
        supports_vision: consistent(vision, Value::as_bool),
    };
    (metadata != ProviderModelMetadata::default()).then_some(metadata)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_MODEL_ID_BYTES && !id.chars().any(char::is_control)
}

pub fn parse_models(body: &Value) -> Result<Vec<DiscoveredModel>, String> {
    let items = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(Value::as_array)
        .ok_or("PROVIDER_MODELS_INVALID_LIST")?;
    if items.len() > MAX_MODELS {
        return Err("PROVIDER_MODELS_TOO_MANY".into());
    }
    let mut models = Vec::<DiscoveredModel>::new();
    let mut seen = BTreeMap::<String, usize>::new();
    for item in items {
        let Some(id) = item
            .get("id")
            .or_else(|| item.get("slug"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| valid_id(id))
        else {
            continue;
        };
        let model = DiscoveredModel {
            id: id.into(),
            display_name: item
                .get("display_name")
                .or_else(|| item.get("displayName"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| {
                    !value.is_empty()
                        && value.len() <= MAX_MODEL_ID_BYTES
                        && !value.chars().any(char::is_control)
                })
                .map(str::to_owned),
            metadata: parse_metadata(item),
        };
        let key = id.to_ascii_lowercase();
        if let Some(index) = seen.get(&key) {
            if models[*index].metadata != model.metadata {
                return Err("PROVIDER_MODELS_CONFLICTING_METADATA".into());
            }
            continue;
        }
        seen.insert(key, models.len());
        models.push(model);
    }
    Ok(models)
}

fn cache_path(root: &Path, scope: &str) -> PathBuf {
    root.join(CACHE_DIRECTORY).join(format!("{scope}.json"))
}

fn read_cache(root: &Path, scope: &str) -> Option<Arc<CacheEntry>> {
    let path = cache_path(root, scope);
    let parent = std::fs::symlink_metadata(path.parent()?).ok()?;
    if !parent.is_dir() || parent.file_type().is_symlink() {
        return None;
    }
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_BODY_BYTES as u64
    {
        return None;
    }
    let modified = metadata.modified().ok()?;
    if let Ok(mut loaded) = LOADED_CACHES.lock() {
        if let Some(cached) = loaded.get_mut(&path) {
            if cached.modified == modified
                && cached.size == metadata.len()
                && cached.value.scope == scope
            {
                cached.used = Instant::now();
                return Some(cached.value.clone());
            }
            loaded.remove(&path);
        }
    }
    let cache: CacheEntry = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    if cache.version != 1
        || cache.scope != scope
        || cache.models.len() > MAX_MODELS
        || cache.fetched_at_ms > now_ms().saturating_add(60_000)
    {
        return None;
    }
    // Cache files are input too. Reject corrupt facts rather than passing their
    // values through to a client catalog or upstream request.
    for (id, value) in &cache.models {
        if !valid_id(id)
            || value.reasoning_efforts.as_ref().is_some_and(|levels| {
                effort_list(&serde_json::json!(levels)).as_ref() != Some(levels)
            })
            || value
                .default_reasoning_effort
                .as_ref()
                .is_some_and(|effort| {
                    !EFFORTS.contains(&effort.as_str())
                        || value
                            .reasoning_efforts
                            .as_ref()
                            .is_some_and(|levels| !levels.contains(effort))
                })
            || value
                .context_window
                .is_some_and(|n| positive_window(&serde_json::json!(n)) != Some(n))
        {
            return None;
        }
    }
    let cache = Arc::new(cache);
    if let Ok(mut loaded) = LOADED_CACHES.lock() {
        // Catalog generation asks for one fact per model. Reparse each scoped
        // file only when it changes, rather than rereading an N-model file N times.
        while loaded.len() >= 32
            || loaded.values().map(|item| item.size).sum::<u64>() + metadata.len()
                > (16 * 1024 * 1024)
        {
            let Some(oldest) = loaded
                .iter()
                .min_by_key(|(_, item)| item.used)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            loaded.remove(&oldest);
        }
        loaded.insert(
            path,
            LoadedCache {
                modified,
                size: metadata.len(),
                used: Instant::now(),
                value: cache.clone(),
            },
        );
    }
    Some(cache)
}

fn save_cache(root: &Path, entry: &CacheEntry) -> Result<(), String> {
    let _guard = CACHE_WRITE_LOCK
        .lock()
        .map_err(|_| "PROVIDER_METADATA_CACHE_BUSY")?;
    if read_cache(root, &entry.scope)
        .is_some_and(|current| current.fetched_at_ms > entry.fetched_at_ms)
    {
        return Ok(());
    }
    let path = cache_path(root, &entry.scope);
    let directory = path.parent().ok_or("PROVIDER_METADATA_CACHE_FAILED")?;
    if let Ok(metadata) = std::fs::symlink_metadata(directory) {
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("PROVIDER_METADATA_CACHE_FAILED".into());
        }
    }
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(directory)
        .map_err(|_| "PROVIDER_METADATA_CACHE_FAILED")?;
    let encoded = serde_json::to_string(entry).map_err(|_| "PROVIDER_METADATA_CACHE_FAILED")?;
    if encoded.len() > MAX_BODY_BYTES {
        return Err("PROVIDER_METADATA_CACHE_TOO_LARGE".into());
    }
    super::atomic_write::write_secret_string_atomic(&path, &encoded)
        .map_err(|_| "PROVIDER_METADATA_CACHE_FAILED".to_string())?;
    if let Ok(mut loaded) = LOADED_CACHES.lock() {
        loaded.remove(&path);
    }
    Ok(())
}

async fn fetch_at(
    base_url: &str,
    api_key: &str,
    root: &Path,
    timeout: Duration,
) -> Result<FetchedModels, String> {
    let scope = scope(base_url, api_key)?;
    let url = models_url(base_url)?;
    let origin = reqwest::Url::parse(&url)
        .map_err(|_| "PROVIDER_BASE_URL_INVALID")?
        .origin();
    let started = Instant::now();
    let fetched_at_ms = now_ms();
    // This matches the existing discovery client's proxy behavior. API-key
    // providers do not inherit a ChatGPT subscription account's egress proxy.
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() < 5 && attempt.url().origin() == origin {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|_| "CREATE_HTTP_CLIENT_FAILED")?;
    let mut response = client
        .get(url)
        .bearer_auth(api_key.trim())
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| "PROVIDER_MODELS_NETWORK_FAILED")?;
    if !response.status().is_success() {
        return Err(format!(
            "PROVIDER_MODELS_HTTP_{}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_BYTES as u64)
    {
        return Err("PROVIDER_MODELS_RESPONSE_TOO_LARGE".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "PROVIDER_MODELS_NETWORK_FAILED")?
    {
        if chunk.len() > MAX_BODY_BYTES.saturating_sub(bytes.len()) {
            return Err("PROVIDER_MODELS_RESPONSE_TOO_LARGE".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let parsed: Value =
        serde_json::from_slice(&bytes).map_err(|_| "PROVIDER_MODELS_PARSE_FAILED")?;
    let models = parse_models(&parsed)?;
    let entry = CacheEntry {
        version: 1,
        scope,
        fetched_at_ms,
        models: models
            .iter()
            .filter_map(|model| {
                model
                    .metadata
                    .clone()
                    .map(|value| (model.id.clone(), value))
            })
            .collect(),
    };
    // Discovery still returns useful model IDs if the optional metadata cache
    // cannot be written. A failed HTTP/parse never overwrites the last good file.
    let _ = save_cache(root, &entry);
    Ok(FetchedModels {
        models,
        latency_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
    })
}

pub async fn fetch(base_url: &str, api_key: &str) -> Result<FetchedModels, String> {
    let root = super::account::get_data_dir().map_err(|_| "PROVIDER_METADATA_CACHE_FAILED")?;
    fetch_at(base_url, api_key, &root, Duration::from_secs(20)).await
}

pub fn lookup(account: &CodexAccount, model: &str) -> Option<ProviderModelMetadata> {
    if !account.is_api_key_auth() {
        return None;
    }
    let scope = scope(
        account.api_base_url.as_deref()?,
        account.openai_api_key.as_deref()?,
    )
    .ok()?;
    let root = super::account::get_data_dir().ok()?;
    let cache = read_cache(&root, &scope)?;
    cache
        .models
        .get(model)
        .or_else(|| {
            cache
                .models
                .iter()
                .find(|(id, _)| id.eq_ignore_ascii_case(model))
                .map(|(_, metadata)| metadata)
        })
        .cloned()
}

pub async fn refresh(account: &CodexAccount) -> Result<(), String> {
    if !account.is_api_key_auth() {
        return Ok(());
    }
    let base = account
        .api_base_url
        .as_deref()
        .ok_or("PROVIDER_BASE_URL_INVALID")?;
    let key = account.openai_api_key.as_deref().ok_or("MISSING_API_KEY")?;
    let scope = scope(base, key)?;
    let root = super::account::get_data_dir().map_err(|_| "PROVIDER_METADATA_CACHE_FAILED")?;
    if read_cache(&root, &scope)
        .is_some_and(|cache| now_ms().saturating_sub(cache.fetched_at_ms) < CACHE_TTL_MS)
    {
        return Ok(());
    }
    if FAILED_REFRESHES
        .lock()
        .ok()
        .and_then(|failures| failures.get(&scope).copied())
        .is_some_and(|failed| failed.elapsed() < FAILURE_BACKOFF)
    {
        return Ok(());
    }
    let result = fetch_at(base, key, &root, Duration::from_secs(4))
        .await
        .map(|_| ());
    if let Ok(mut failures) = FAILED_REFRESHES.lock() {
        failures.retain(|_, failed| failed.elapsed() < FAILURE_BACKOFF);
        if result.is_err() {
            if failures.len() >= 1024 {
                if let Some(oldest) = failures
                    .iter()
                    .min_by_key(|(_, at)| **at)
                    .map(|(key, _)| key.clone())
                {
                    failures.remove(&oldest);
                }
            }
            failures.insert(scope, Instant::now());
        } else {
            failures.remove(&scope);
        }
    }
    result
}

#[cfg(test)]
pub(crate) fn cache_for_test(
    account: &CodexAccount,
    model: &str,
    metadata: ProviderModelMetadata,
) -> Result<(), String> {
    let scope = scope(
        account.api_base_url.as_deref().ok_or("missing base")?,
        account.openai_api_key.as_deref().ok_or("missing key")?,
    )?;
    let root = super::account::get_data_dir()?;
    let mut entry = read_cache(&root, &scope)
        .map(|cache| (*cache).clone())
        .unwrap_or(CacheEntry {
            version: 1,
            scope,
            fetched_at_ms: now_ms(),
            models: BTreeMap::new(),
        });
    entry.fetched_at_ms = now_ms();
    entry.models.insert(model.into(), metadata);
    save_cache(&root, &entry)
}

#[cfg(test)]
#[path = "codex_provider_metadata_tests.rs"]
mod tests;

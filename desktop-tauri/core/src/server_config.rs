// Copyright 2026 Contextual, Inc. https://agentrq.com
// This notice may not be modified or removed.
// SPDX-License-Identifier: AGPL-3.0-only

//! Which AgentRQ server the desktop app talks to, and how that choice is stored.
//!
//! The desktop app is a client: the server lives wherever the user runs it, so
//! the app has to be told once and remember. Everything here takes its I/O as
//! traits so the rules can be tested without a window or a filesystem.
//!
//! The file holds a list of profiles. Reading or writing "the" server or mute
//! list means the *active* profile's.
//!
//! Scope: the profile model is only what `migrate_config` and the store need
//! (cleaning, migration, active, update). Adding, switching, removing and
//! renaming profiles, and remembering accounts, are WP4's, and so are the
//! store methods that call them. `account` is carried through untouched until
//! WP4 ports `identity.js`.
//!
//! Rust notes: the store is synchronous, and the network probe is a trait that
//! returns the status and the body, in place of `fetch`.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::io;
use url::Url;

/// The hosted instance, so someone who installs the app and has no server of
/// their own still gets somewhere useful. Self-hosters type their own address
/// once on the connection screen, or set `AGENTRQ_SERVER_URL`.
pub const DEFAULT_SERVER_URL: &str = "https://app.agentrq.com";

/// Filename under the app's data directory.
pub const CONFIG_FILENAME: &str = "agentrq-desktop.json";

/// Shape version of the stored config.
///
/// v3: `{ profiles: [{ id, label, partition, serverUrl, mutedWorkspaces }],
/// activeProfileId }`. A v2 file becomes a single profile carrying the
/// settings it already had. v2 added `mutedWorkspaces`; v1 was `{ serverUrl }`
/// and migrates forward by defaulting it to empty.
pub const CONFIG_VERSION: u32 = 3;

/// Endpoint used to decide whether a URL is really an AgentRQ server.
pub const CONFIG_PROBE_PATH: &str = "/api/v1/auth/config";

/// Shown when a profile has no name of its own.
pub const DEFAULT_PROFILE_LABEL: &str = "Default";

/// What `connection:save` answers while `AGENTRQ_SERVER_URL` pins the server.
pub const CONNECTION_LOCKED_REASON: &str = "Server is pinned by AGENTRQ_SERVER_URL";

const MAX_LABEL: usize = 40;

pub const REASON_REQUIRED: &str = "Server URL is required";
pub const REASON_INVALID: &str = "Not a valid URL";
pub const REASON_SCHEME: &str = "Server URL must use http or https";
pub const REASON_NOT_AGENTRQ: &str = "That URL is not an AgentRQ server";

/// Turn whatever the user typed into a URL we can safely resolve against.
///
/// A bare host is assumed to be `http://`: the common case is a self-hosted
/// instance on a LAN address or localhost, where demanding a scheme would just
/// be friction. Anything that is not http(s) is rejected outright, so a stored
/// value cannot smuggle in `file://` or `javascript:`. `Err` carries the reason.
pub fn normalize_server_url(input: &str) -> Result<String, &'static str> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(REASON_REQUIRED);
    }

    let with_scheme = if has_scheme(raw) {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };

    let mut parsed = Url::parse(&with_scheme).map_err(|_| REASON_INVALID)?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(REASON_SCHEME);
    }
    // No host check is needed: http and https are "special" schemes, and the URL
    // parser already rejects an empty host for those (`http://` fails above).

    // Only the origin and any base path matter; a query or fragment on the
    // server URL would be silently dropped when resolving /api/v1 against it, so
    // drop it here where it is visible instead.
    parsed.set_query(None);
    parsed.set_fragment(None);
    // Trailing slash removed so joining `/api/v1` behaves predictably.
    let text = parsed.to_string();
    Ok(text.strip_suffix('/').unwrap_or(&text).to_string())
}

/// `^[a-zA-Z][a-zA-Z0-9+.-]*://`
fn has_scheme(raw: &str) -> bool {
    let Some((scheme, _)) = raw.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Resolve the server URL for this launch. `env_server_url` is the value of
/// `AGENTRQ_SERVER_URL`, read by the caller; it exists so a developer can point
/// at a scratch backend without touching stored settings.
pub fn resolve_server_url(env_server_url: Option<&str>, stored: &str) -> String {
    [env_server_url.unwrap_or(""), stored]
        .into_iter()
        .filter(|candidate| !candidate.is_empty())
        .find_map(|candidate| normalize_server_url(candidate).ok())
        .unwrap_or_else(|| DEFAULT_SERVER_URL.to_string())
}

/// The server `AGENTRQ_SERVER_URL` pins the app to, when it holds a usable URL.
///
/// When this is `Some` the connection is locked: the connection screen is
/// skipped and "Switch Server" is a no-op, because the stored value would not be
/// what the app is using anyway.
pub fn connection_lock(env_server_url: Option<&str>) -> Option<String> {
    normalize_server_url(env_server_url.unwrap_or("")).ok()
}

/// One profile, with everything absent or malformed filled in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub label: String,
    pub partition: String,
    pub server_url: String,
    pub muted_workspaces: Vec<String>,
    /// Who this profile belongs to, as last seen. Opaque until WP4.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<Value>,
}

/// The stored config, always a valid shape with at least one profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub version: u32,
    pub active_profile_id: String,
    pub profiles: Vec<Profile>,
}

/// Partition name for a profile. `persist:` is what makes the jar survive a
/// restart. The id is embedded directly, which is why ids are restricted to
/// characters that are safe in a directory name.
pub fn partition_for(id: &str) -> String {
    format!("persist:profile-{id}")
}

/// Ids are used in a filesystem path, so they are deliberately narrow:
/// `^[a-z0-9][a-z0-9-]{0,63}$`.
pub fn is_valid_profile_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    id.len() <= 64
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn clean_label(label: Option<&str>) -> String {
    let collapsed = label
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if collapsed.is_empty() {
        DEFAULT_PROFILE_LABEL.to_string()
    } else {
        collapsed.chars().take(MAX_LABEL).collect()
    }
}

fn clean_muted<S: AsRef<str>>(ids: impl IntoIterator<Item = S>) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.into_iter()
        .map(|id| id.as_ref().to_string())
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .collect()
}

fn muted_from_json(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => clean_muted(items.iter().filter_map(Value::as_str)),
        _ => Vec::new(),
    }
}

fn str_field<'a>(raw: &'a Value, key: &str) -> Option<&'a str> {
    raw.get(key).and_then(Value::as_str)
}

/// One stored profile. `None` when the id is unusable, since a profile without a
/// valid id has no partition and cannot be stored.
fn make_profile(raw: &Value) -> Option<Profile> {
    let id = str_field(raw, "id").filter(|id| is_valid_profile_id(id))?;
    Some(Profile {
        id: id.to_string(),
        label: clean_label(str_field(raw, "label")),
        // Stored rather than derived so the name is stable if the scheme ever
        // changes: a partition that moved would be an empty jar, a sign-out.
        partition: str_field(raw, "partition")
            .filter(|partition| !partition.is_empty())
            .map_or_else(|| partition_for(id), str::to_string),
        server_url: str_field(raw, "serverUrl").unwrap_or("").to_string(),
        muted_workspaces: muted_from_json(raw.get("mutedWorkspaces")),
        account: raw.get("account").filter(|a| a.is_object()).cloned(),
    })
}

/// Bring stored profile state forward, from any shape including none.
///
/// A config from before profiles existed becomes a single profile carrying the
/// settings that were already there.
fn migrate_profiles(raw: &Value) -> (Vec<Profile>, String) {
    let mut profiles: Vec<Profile> = match raw.get("profiles") {
        Some(Value::Array(items)) => items.iter().filter_map(make_profile).collect(),
        _ => Vec::new(),
    };

    if profiles.is_empty() {
        // Either a first run or a pre-profiles config. Both become one profile;
        // the second keeps the server and mutes that were already configured.
        let id = "default";
        profiles.push(Profile {
            id: id.to_string(),
            label: DEFAULT_PROFILE_LABEL.to_string(),
            partition: partition_for(id),
            server_url: str_field(raw, "serverUrl").unwrap_or("").to_string(),
            muted_workspaces: muted_from_json(raw.get("mutedWorkspaces")),
            account: None,
        });
    }

    // Two profiles sharing an id would share a partition, which is the one thing
    // this must never allow: the same account wearing two names.
    let mut seen = HashSet::new();
    profiles.retain(|profile| seen.insert(profile.id.clone()));

    let active = str_field(raw, "activeProfileId")
        .filter(|id| profiles.iter().any(|profile| profile.id == *id))
        .unwrap_or(&profiles[0].id)
        .to_string();
    (profiles, active)
}

/// Bring a stored config forward to the current shape.
///
/// Anything unrecognised degrades to "no server configured", which sends the
/// user to the connection screen: mildly annoying and completely safe, far
/// better than booting the app pointed at a URL we could not parse. A file
/// written by a *newer* build keeps its `serverUrl` if it still looks like a
/// URL, and every other key is dropped rather than guessed at.
pub fn migrate_config(raw: &Value) -> ServerConfig {
    let (profiles, active_profile_id) = migrate_profiles(raw);
    ServerConfig {
        version: CONFIG_VERSION,
        active_profile_id,
        // Each profile's server is re-checked here rather than trusted from
        // disk: a stored `file:///x` reaching the fetch base would be a hole,
        // and an unparseable one would boot the app pointed at nothing. Either
        // becomes "not configured", which is the connection screen.
        profiles: profiles
            .into_iter()
            .map(|profile| Profile {
                server_url: normalize_server_url(&profile.server_url).unwrap_or_default(),
                ..profile
            })
            .collect(),
    }
}

impl ServerConfig {
    /// The profile in use. Never missing: `migrate_config` guarantees one.
    pub fn active(&self) -> &Profile {
        self.profiles
            .iter()
            .find(|profile| profile.id == self.active_profile_id)
            .unwrap_or(&self.profiles[0])
    }

    fn update_active(&mut self, server_url: Option<&str>, muted: Option<Vec<String>>) {
        let id = &self.active_profile_id;
        if let Some(profile) = self.profiles.iter_mut().find(|profile| profile.id == *id) {
            if let Some(server_url) = server_url {
                profile.server_url = server_url.to_string();
            }
            if let Some(muted) = muted {
                profile.muted_workspaces = muted;
            }
        }
    }
}

/// The file the config lives in.
pub trait ConfigFile {
    /// Fails when the file is absent.
    fn read_file(&self) -> io::Result<String>;
    fn write_file(&self, contents: &str) -> io::Result<()>;
}

/// Why a save did not happen.
#[derive(Debug)]
pub enum SaveError {
    /// The URL was refused; the reason is for the connection screen.
    Invalid(&'static str),
    Io(io::Error),
}

/// Persisted server choice.
pub struct ServerConfigStore<F: ConfigFile> {
    file: F,
}

impl<F: ConfigFile> ServerConfigStore<F> {
    pub fn new(file: F) -> Self {
        Self { file }
    }

    fn write(&self, config: &ServerConfig) -> io::Result<()> {
        self.file.write_file(&serde_json::to_string_pretty(config)?)
    }

    /// Always a valid shape with at least one profile: a missing or corrupt
    /// file reads as a first run rather than as an error.
    pub fn load(&self) -> ServerConfig {
        let raw = self
            .file
            .read_file()
            .ok()
            .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
            .unwrap_or(Value::Null);
        migrate_config(&raw)
    }

    /// The profile in use, with its own server and mute list.
    pub fn active(&self) -> Profile {
        self.load().active().clone()
    }

    /// Point the active profile at a server. Returns the normalised URL.
    pub fn save(&self, input: &str) -> Result<String, SaveError> {
        let url = normalize_server_url(input).map_err(SaveError::Invalid)?;
        let mut config = self.load();
        config.update_active(Some(&url), None);
        self.write(&config).map_err(SaveError::Io)?;
        Ok(url)
    }

    /// Forget the active profile's server, sending it back to the first-run
    /// screen.
    pub fn clear(&self) -> io::Result<()> {
        let mut config = self.load();
        config.update_active(Some(""), None);
        self.write(&config)
    }

    /// Replace the active profile's muted-workspace list. Returns what was kept.
    pub fn set_muted_workspaces(&self, ids: &[&str]) -> io::Result<Vec<String>> {
        let mut config = self.load();
        config.update_active(None, Some(clean_muted(ids)));
        self.write(&config)?;
        Ok(config.active().muted_workspaces.clone())
    }

    /// Turn notifications for one workspace on or off, for the active profile.
    pub fn set_workspace_muted(&self, workspace_id: &str, muted: bool) -> io::Result<Vec<String>> {
        let mut ids = self.active().muted_workspaces;
        if muted {
            ids.push(workspace_id.to_string());
        } else {
            ids.retain(|id| id != workspace_id);
        }
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
        self.set_muted_workspaces(&ids)
    }
}

/// What the probe got back: the status, and the body as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResponse {
    pub status: u16,
    pub body: String,
}

/// The network, for the one request `validate_server_url` makes.
pub trait ConfigProbe {
    /// `Err` carries the transport's message (refused, timed out, and so on).
    fn get(&self, url: &str) -> Result<ProbeResponse, String>;
}

/// A server that answered the probe like AgentRQ does.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidServer {
    pub url: String,
    pub config: Value,
}

/// Why a URL was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub reason: String,
    /// The transport's message, when the host could not be reached.
    pub detail: Option<String>,
}

fn rejected(reason: impl Into<String>) -> ValidationError {
    ValidationError {
        reason: reason.into(),
        detail: None,
    }
}

/// Check that a URL is actually an AgentRQ server before it is stored.
///
/// `/api/v1/auth/config` is the right probe: it is unauthenticated, cheap, and
/// its response tells the login screen which sign-in methods exist, so a URL
/// that answers it is a server this app can really use, not merely a host that
/// happens to be up. `input` is the raw user entry; it is normalised here.
pub fn validate_server_url(
    input: &str,
    probe: &dyn ConfigProbe,
) -> Result<ValidServer, ValidationError> {
    let url = normalize_server_url(input).map_err(rejected)?;

    let response = probe
        .get(&format!("{url}{CONFIG_PROBE_PATH}"))
        .map_err(|detail| ValidationError {
            reason: format!("Could not reach {url}"),
            detail: Some(detail),
        })?;

    if !(200..300).contains(&response.status) {
        return Err(rejected(format!(
            "Server answered with {}",
            response.status
        )));
    }

    let config: Value =
        serde_json::from_str(&response.body).map_err(|_| rejected(REASON_NOT_AGENTRQ))?;

    // A JSON body alone is not proof: any API might return one. The auth config
    // always carries these flags, so their absence means we are talking to
    // something else.
    let is_flag = |key: &str| config.get(key).is_some_and(Value::is_boolean);
    if !is_flag("rootLoginEnabled") && !is_flag("githubLoginEnabled") {
        return Err(rejected(REASON_NOT_AGENTRQ));
    }

    Ok(ValidServer { url, config })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::{Cell, RefCell};

    // normalizeServerUrl

    fn normalized(input: &str) -> String {
        normalize_server_url(input).unwrap()
    }

    #[test]
    fn keeps_an_explicit_http_or_https_url() {
        assert_eq!(
            normalized("https://agentrq.example.com"),
            "https://agentrq.example.com"
        );
        assert_eq!(normalized("http://localhost:3000"), "http://localhost:3000");
    }

    #[test]
    fn assumes_http_for_a_bare_host_which_is_the_self_hosted_case() {
        assert_eq!(normalized("localhost:3000"), "http://localhost:3000");
        assert_eq!(normalized("192.168.1.20:8080"), "http://192.168.1.20:8080");
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(
            normalized("  https://agentrq.example.com  "),
            "https://agentrq.example.com"
        );
    }

    #[test]
    fn strips_the_trailing_slash_so_api_v1_resolves_predictably() {
        assert_eq!(
            normalized("https://agentrq.example.com/"),
            "https://agentrq.example.com"
        );
    }

    #[test]
    fn preserves_a_base_path_which_reverse_proxied_deployments_rely_on() {
        assert_eq!(
            normalized("https://example.com/agentrq"),
            "https://example.com/agentrq"
        );
    }

    #[test]
    fn drops_a_query_or_fragment_rather_than_losing_it_silently_later() {
        assert_eq!(
            normalized("https://example.com/?a=1#x"),
            "https://example.com"
        );
    }

    #[test]
    fn rejects_an_empty_value() {
        // null and undefined collapse to the empty string, as in JS.
        assert_eq!(normalize_server_url(""), Err("Server URL is required"));
        assert_eq!(normalize_server_url("   "), Err("Server URL is required"));
    }

    #[test]
    fn rejects_a_scheme_that_is_not_http_or_https() {
        assert_eq!(
            normalize_server_url("file:///etc/passwd"),
            Err("Server URL must use http or https")
        );
        assert_eq!(
            normalize_server_url("javascript://alert(1)"),
            Err("Server URL must use http or https")
        );
    }

    #[test]
    fn rejects_a_url_with_no_host() {
        assert_eq!(normalize_server_url("http://"), Err("Not a valid URL"));
    }

    #[test]
    fn rejects_unparseable_input() {
        assert_eq!(normalize_server_url("http://["), Err("Not a valid URL"));
    }

    #[test]
    fn recognises_a_scheme_only_when_it_is_one() {
        // Not in the vitest file: the shapes the `^[a-zA-Z][a-zA-Z0-9+.-]*://`
        // test has to tell apart.
        assert!(!has_scheme("localhost:3000"));
        assert!(!has_scheme("://x"));
        assert!(!has_scheme("1http://x"));
        assert!(!has_scheme("ht tp://x"));
        assert!(has_scheme("a+b.c-d1://x"));
    }

    // resolveServerUrl

    #[test]
    fn falls_back_to_the_default_when_nothing_is_configured() {
        assert_eq!(resolve_server_url(None, ""), DEFAULT_SERVER_URL);
        assert_eq!(resolve_server_url(Some(""), ""), DEFAULT_SERVER_URL);
    }

    #[test]
    fn prefers_the_environment_override_so_a_scratch_backend_needs_no_stored_change() {
        assert_eq!(
            resolve_server_url(Some("http://localhost:3999"), "https://prod.example.com"),
            "http://localhost:3999"
        );
    }

    #[test]
    fn uses_the_stored_value_when_there_is_no_override() {
        assert_eq!(
            resolve_server_url(None, "https://prod.example.com"),
            "https://prod.example.com"
        );
    }

    #[test]
    fn normalises_whatever_it_takes() {
        assert_eq!(
            resolve_server_url(None, "example.com/"),
            "http://example.com"
        );
    }

    #[test]
    fn skips_an_invalid_candidate_rather_than_failing_the_launch() {
        assert_eq!(
            resolve_server_url(Some("file:///x"), "https://ok.example.com"),
            "https://ok.example.com"
        );
        assert_eq!(resolve_server_url(None, "file:///x"), DEFAULT_SERVER_URL);
    }

    #[test]
    fn locks_the_connection_only_to_a_usable_environment_url() {
        // Not in the vitest file: `isConnectionLocked` in index.js.
        assert_eq!(
            connection_lock(Some("localhost:3999")).as_deref(),
            Some("http://localhost:3999")
        );
        assert_eq!(connection_lock(Some("file:///x")), None);
        assert_eq!(connection_lock(Some("")), None);
        assert_eq!(connection_lock(None), None);
        assert_eq!(
            CONNECTION_LOCKED_REASON,
            "Server is pinned by AGENTRQ_SERVER_URL"
        );
    }

    // migrateConfig

    fn only_profile(raw: Value) -> Profile {
        migrate_config(&raw).profiles.remove(0)
    }

    #[test]
    fn turns_a_pre_profiles_config_into_one_profile_that_keeps_its_settings() {
        let config = migrate_config(&json!({
            "version": 2, "serverUrl": "https://example.com", "mutedWorkspaces": ["ws1"]
        }));

        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.profiles.len(), 1);
        assert_eq!(config.profiles[0].server_url, "https://example.com");
        assert_eq!(config.profiles[0].muted_workspaces, ["ws1"]);
        assert_eq!(config.active_profile_id, config.profiles[0].id);
    }

    #[test]
    fn adopts_an_unversioned_file_which_predates_the_version_field() {
        assert_eq!(
            only_profile(json!({"serverUrl": "example.com"})).server_url,
            "http://example.com"
        );
    }

    #[test]
    fn migrates_a_v1_file_forward_by_defaulting_the_new_fields() {
        let profile = only_profile(json!({"version": 1, "serverUrl": "https://example.com"}));
        assert_eq!(profile.server_url, "https://example.com");
        assert!(profile.muted_workspaces.is_empty());
    }

    #[test]
    fn discards_a_mute_list_that_is_not_a_list_of_ids() {
        for bad in [json!("ws1"), json!(42), json!({"ws1": true}), json!(null)] {
            let profile = only_profile(
                json!({"version": 2, "serverUrl": "https://e.com", "mutedWorkspaces": bad}),
            );
            assert!(profile.muted_workspaces.is_empty());
        }
        let profile = only_profile(
            json!({"version": 2, "serverUrl": "https://e.com", "mutedWorkspaces": ["ok", "", 7, null]}),
        );
        assert_eq!(profile.muted_workspaces, ["ok"]);
    }

    #[test]
    fn normalises_whatever_it_finds_rather_than_trusting_it() {
        assert_eq!(
            only_profile(json!({"version": 1, "serverUrl": "https://example.com/"})).server_url,
            "https://example.com"
        );
    }

    #[test]
    fn re_checks_every_stored_profile_url_not_just_a_legacy_one() {
        let config = migrate_config(&json!({
            "profiles": [
                {"id": "one", "label": "A", "serverUrl": "file:///etc/passwd"},
                {"id": "two", "label": "B", "serverUrl": "example.com/"},
            ]
        }));

        let urls: Vec<&str> = config
            .profiles
            .iter()
            .map(|p| p.server_url.as_str())
            .collect();
        assert_eq!(urls, ["", "http://example.com"]);
    }

    #[test]
    fn keeps_only_what_it_understands_from_a_newer_file() {
        let config = migrate_config(
            &json!({"version": 99, "serverUrl": "https://example.com", "theme": "dark"}),
        );

        assert_eq!(config.profiles[0].server_url, "https://example.com");
        assert!(!serde_json::to_string(&config).unwrap().contains("theme"));
    }

    #[test]
    fn degrades_to_unconfigured_for_anything_unusable() {
        // `undefined` has no JSON form and is covered by null.
        for raw in [
            json!(null),
            json!("a string"),
            json!(42),
            json!([]),
            json!({"serverUrl": "file:///x"}),
            json!({}),
        ] {
            let config = migrate_config(&raw);
            assert_eq!(config.profiles.len(), 1);
            assert_eq!(config.profiles[0].server_url, "");
        }
    }

    #[test]
    fn always_leaves_exactly_one_profile_active() {
        let config = migrate_config(&json!({
            "profiles": [{"id": "one", "label": "A"}, {"id": "two", "label": "B"}],
            "activeProfileId": "gone",
        }));

        assert!(config
            .profiles
            .iter()
            .any(|p| p.id == config.active_profile_id));
    }

    #[test]
    fn cleans_profiles_the_way_the_profile_model_does() {
        // Not in the vitest file (profiles.test.js is WP4's): the parts of
        // makeProfile and migrateProfiles that migrateConfig runs through.
        let config = migrate_config(&json!({
            "profiles": [
                {"id": "Bad Id", "label": "dropped"},
                {"id": "one", "label": "  Work \n  laptop  ", "partition": "persist:custom",
                 "account": {"email": "sam@example.com"}},
                {"id": "one", "label": "duplicate"},
                {"id": "two", "label": "x".repeat(60), "account": "not an object"},
                {"id": "three", "label": 7, "partition": ""},
            ],
            "activeProfileId": "two",
        }));

        let ids: Vec<&str> = config.profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["one", "two", "three"]);
        assert_eq!(config.active_profile_id, "two");
        assert_eq!(config.profiles[0].label, "Work laptop");
        assert_eq!(config.profiles[0].partition, "persist:custom");
        assert_eq!(
            config.profiles[0].account,
            Some(json!({"email": "sam@example.com"}))
        );
        assert_eq!(config.profiles[1].label.chars().count(), MAX_LABEL);
        assert_eq!(config.profiles[1].account, None);
        assert_eq!(config.profiles[2].label, DEFAULT_PROFILE_LABEL);
        assert_eq!(config.profiles[2].partition, "persist:profile-three");
    }

    #[test]
    fn accepts_only_ids_that_are_safe_in_a_directory_name() {
        for good in ["a", "0", "default", "a-b-1", &"a".repeat(64)] {
            assert!(is_valid_profile_id(good), "{good}");
        }
        for bad in ["", "-a", "A", "a_b", "a b", "a/b", "..", &"a".repeat(65)] {
            assert!(!is_valid_profile_id(bad), "{bad}");
        }
    }

    // createServerConfigStore

    #[derive(Default)]
    struct MemoryFile {
        contents: RefCell<Option<String>>,
        writes: Cell<usize>,
    }

    impl MemoryFile {
        fn with(contents: &str) -> Self {
            Self {
                contents: RefCell::new(Some(contents.to_string())),
                writes: Cell::new(0),
            }
        }
        fn read(&self) -> Value {
            serde_json::from_str(self.contents.borrow().as_ref().unwrap()).unwrap()
        }
    }

    impl ConfigFile for &MemoryFile {
        fn read_file(&self) -> io::Result<String> {
            self.contents
                .borrow()
                .clone()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "ENOENT"))
        }
        fn write_file(&self, contents: &str) -> io::Result<()> {
            self.writes.set(self.writes.get() + 1);
            *self.contents.borrow_mut() = Some(contents.to_string());
            Ok(())
        }
    }

    fn legacy(extra: Value) -> String {
        let mut value = json!({"version": 2, "serverUrl": "https://example.com"});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        value.to_string()
    }

    #[test]
    fn reads_a_missing_file_as_not_configured_the_first_run() {
        let file = MemoryFile::default();
        let config = ServerConfigStore::new(&file).load();

        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.profiles.len(), 1);
        assert_eq!(config.profiles[0].server_url, "");
    }

    #[test]
    fn reads_a_stored_server_back() {
        let file = MemoryFile::with(r#"{"version":1,"serverUrl":"https://example.com"}"#);
        assert_eq!(
            ServerConfigStore::new(&file).active().server_url,
            "https://example.com"
        );
    }

    #[test]
    fn treats_corrupt_json_as_not_configured() {
        let file = MemoryFile::with("{ not json");
        assert_eq!(ServerConfigStore::new(&file).active().server_url, "");
    }

    #[test]
    fn saves_a_normalised_url_and_stamps_the_version() {
        let file = MemoryFile::default();
        let store = ServerConfigStore::new(&file);

        assert_eq!(store.save("example.com/").unwrap(), "http://example.com");
        let written = file.read();
        assert_eq!(written["version"], json!(CONFIG_VERSION));
        assert_eq!(
            written["profiles"][0]["serverUrl"],
            json!("http://example.com")
        );
    }

    #[test]
    fn refuses_to_store_an_invalid_url() {
        let file = MemoryFile::default();
        let store = ServerConfigStore::new(&file);

        assert!(matches!(
            store.save("file:///etc/passwd"),
            Err(SaveError::Invalid("Server URL must use http or https"))
        ));
        assert_eq!(file.writes.get(), 0);
    }

    #[test]
    fn round_trips_a_save_through_a_load() {
        let file = MemoryFile::default();
        let store = ServerConfigStore::new(&file);
        store.save("https://example.com").unwrap();
        assert_eq!(store.active().server_url, "https://example.com");
    }

    #[test]
    fn keeps_mute_choices_when_the_server_changes() {
        let file = MemoryFile::with(&legacy(json!({"mutedWorkspaces": ["ws1"]})));
        let store = ServerConfigStore::new(&file);

        store.save("https://b.example.com").unwrap();

        let active = store.active();
        assert_eq!(active.server_url, "https://b.example.com");
        assert_eq!(active.muted_workspaces, ["ws1"]);
    }

    #[test]
    fn mutes_and_unmutes_a_single_workspace() {
        let file = MemoryFile::with(&legacy(json!({"mutedWorkspaces": []})));
        let store = ServerConfigStore::new(&file);

        assert_eq!(store.set_workspace_muted("ws1", true).unwrap(), ["ws1"]);
        assert_eq!(
            store.set_workspace_muted("ws2", true).unwrap(),
            ["ws1", "ws2"]
        );
        assert_eq!(store.set_workspace_muted("ws1", false).unwrap(), ["ws2"]);
        assert_eq!(store.active().muted_workspaces, ["ws2"]);
    }

    #[test]
    fn does_not_list_a_workspace_twice_when_muted_again() {
        let file = MemoryFile::with(&legacy(json!({"mutedWorkspaces": ["ws1"]})));
        let store = ServerConfigStore::new(&file);

        assert_eq!(store.set_workspace_muted("ws1", true).unwrap(), ["ws1"]);
    }

    #[test]
    fn replaces_the_whole_mute_list_dropping_junk_entries() {
        let file = MemoryFile::default();
        let store = ServerConfigStore::new(&file);

        // JS also passes null entries, which a `&[&str]` cannot hold.
        assert_eq!(
            store
                .set_muted_workspaces(&["ws1", "ws1", "", "ws2"])
                .unwrap(),
            ["ws1", "ws2"]
        );
        assert!(store.set_muted_workspaces(&[]).unwrap().is_empty());
    }

    #[test]
    fn clears_the_stored_server_sending_the_app_back_to_the_first_run_screen() {
        let file = MemoryFile::with(r#"{"version":1,"serverUrl":"https://example.com"}"#);
        let store = ServerConfigStore::new(&file);

        store.clear().unwrap();

        assert_eq!(store.active().server_url, "");
    }

    #[test]
    fn updates_only_the_active_profile() {
        // Not in the vitest file: what "settings belong to one profile" rests on.
        let file = MemoryFile::with(
            &json!({
                "profiles": [
                    {"id": "one", "serverUrl": "https://one.example.com"},
                    {"id": "two", "serverUrl": "https://two.example.com"},
                ],
                "activeProfileId": "two",
            })
            .to_string(),
        );
        let store = ServerConfigStore::new(&file);

        store.save("https://new.example.com").unwrap();
        store.set_workspace_muted("ws1", true).unwrap();

        let config = store.load();
        assert_eq!(config.profiles[0].server_url, "https://one.example.com");
        assert!(config.profiles[0].muted_workspaces.is_empty());
        assert_eq!(config.profiles[1].server_url, "https://new.example.com");
        assert_eq!(config.profiles[1].muted_workspaces, ["ws1"]);
    }

    #[test]
    fn leaves_the_config_alone_when_the_active_profile_is_missing() {
        // Not in the vitest file: `active` falls back to the first profile, and
        // an update for an id that matches nothing changes nothing, as in JS.
        let mut config = migrate_config(&json!({"profiles": [{"id": "one"}]}));
        config.active_profile_id = "gone".to_string();
        assert_eq!(config.active().id, "one");
        config.update_active(Some("https://x.example.com"), Some(vec!["ws".to_string()]));
        assert_eq!(config.profiles[0].server_url, "");
    }

    #[test]
    fn reports_a_write_that_failed() {
        // Not in the vitest file: a store whose file cannot be written.
        struct ReadOnly;
        impl ConfigFile for ReadOnly {
            fn read_file(&self) -> io::Result<String> {
                Err(io::Error::new(io::ErrorKind::NotFound, "ENOENT"))
            }
            fn write_file(&self, _: &str) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "EACCES"))
            }
        }
        let store = ServerConfigStore::new(ReadOnly);
        assert!(matches!(store.save("example.com"), Err(SaveError::Io(_))));
        assert!(store.clear().is_err());
    }

    // validateServerUrl

    struct FakeProbe {
        result: Result<ProbeResponse, String>,
        urls: RefCell<Vec<String>>,
    }

    impl FakeProbe {
        fn new(result: Result<ProbeResponse, String>) -> Self {
            Self {
                result,
                urls: RefCell::new(Vec::new()),
            }
        }
        fn answering(status: u16, body: &str) -> Self {
            Self::new(Ok(ProbeResponse {
                status,
                body: body.to_string(),
            }))
        }
    }

    impl ConfigProbe for FakeProbe {
        fn get(&self, url: &str) -> Result<ProbeResponse, String> {
            self.urls.borrow_mut().push(url.to_string());
            self.result.clone()
        }
    }

    const AUTH_CONFIG: &str =
        r#"{"basePath":"","rootLoginEnabled":true,"githubLoginEnabled":false}"#;

    #[test]
    fn probes_the_auth_config_endpoint_on_the_normalised_url() {
        let probe = FakeProbe::answering(200, AUTH_CONFIG);

        let result = validate_server_url("localhost:3000/", &probe).unwrap();

        assert_eq!(
            *probe.urls.borrow(),
            [format!("http://localhost:3000{CONFIG_PROBE_PATH}")]
        );
        assert_eq!(result.url, "http://localhost:3000");
        assert_eq!(
            result.config,
            serde_json::from_str::<Value>(AUTH_CONFIG).unwrap()
        );
    }

    #[test]
    fn rejects_a_malformed_url_without_reaching_the_network() {
        let probe = FakeProbe::answering(200, AUTH_CONFIG);

        assert_eq!(
            validate_server_url("", &probe),
            Err(rejected("Server URL is required"))
        );
        assert!(probe.urls.borrow().is_empty());
    }

    #[test]
    fn reports_an_unreachable_host() {
        let probe = FakeProbe::new(Err("ECONNREFUSED".to_string()));

        let error = validate_server_url("localhost:9", &probe).unwrap_err();

        assert_eq!(error.reason, "Could not reach http://localhost:9");
        assert_eq!(error.detail.as_deref(), Some("ECONNREFUSED"));
    }

    #[test]
    fn reports_a_thrown_value_that_is_not_an_error() {
        // JS distinguishes an Error from a bare string; the trait carries a
        // message either way.
        let probe = FakeProbe::new(Err("socket hang up".to_string()));
        let error = validate_server_url("localhost:9", &probe).unwrap_err();
        assert_eq!(error.detail.as_deref(), Some("socket hang up"));
    }

    #[test]
    fn reports_a_non_ok_status() {
        let probe = FakeProbe::answering(502, "");
        assert_eq!(
            validate_server_url("example.com", &probe),
            Err(rejected("Server answered with 502"))
        );
    }

    #[test]
    fn rejects_a_host_that_answers_but_is_not_agentrq() {
        let not_json = FakeProbe::answering(200, "not json");
        assert_eq!(
            validate_server_url("example.com", &not_json),
            Err(rejected("That URL is not an AgentRQ server"))
        );

        let wrong_shape = FakeProbe::answering(200, r#"{"hello":"world"}"#);
        assert_eq!(
            validate_server_url("example.com", &wrong_shape),
            Err(rejected("That URL is not an AgentRQ server"))
        );
    }

    #[test]
    fn accepts_a_server_that_advertises_only_github_login() {
        let probe = FakeProbe::answering(200, r#"{"githubLoginEnabled":true}"#);
        assert!(validate_server_url("example.com", &probe).is_ok());
    }

    #[test]
    fn does_not_take_a_flag_that_is_not_a_boolean() {
        // Not in the vitest file: `typeof … !== 'boolean'`.
        let probe = FakeProbe::answering(200, r#"{"rootLoginEnabled":"yes"}"#);
        assert!(validate_server_url("example.com", &probe).is_err());
        let probe = FakeProbe::answering(200, "null");
        assert!(validate_server_url("example.com", &probe).is_err());
    }
}

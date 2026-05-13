//! Shared helpers for assembling the WS-mode `/C` payload that connects
//! 1C-clients to `v8-client-session-manager` instead of the local
//! HTTP MCP server.
//!
//! The actual `/C` payload is parsed by the BSL extension `client_mcp` (see
//! `Мсп_ПараметрыЗапускаКлиент`). This module is responsible for:
//!
//! * choosing between the new WS transport and the local HTTP MCP transport,
//! * probing the manager's TCP socket when the transport is `auto`,
//! * generating per-launch `client_uid`/`corr_id` values,
//! * and serializing the final `key=value;...` snippet.
//!
//! Higher layers (`launch_app`, `run_tests`) decide where this snippet is
//! merged into the final `/C` value.

use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

use uuid::Uuid;

/// Default WS endpoint when no override is supplied.
pub const DEFAULT_MANAGER_URL: &str = "ws://127.0.0.1:4000/sessions";
/// Default log-level value when no override is supplied.
pub const DEFAULT_MCP_LOG_LEVEL: &str = "info";
/// Default WS-handshake timeout when no override is supplied.
pub const DEFAULT_MCP_WS_TIMEOUT_MS: u64 = 1000;
/// Default TCP-probe timeout for `auto` transport detection.
pub const PROBE_TIMEOUT_MS: u64 = 200;

/// Transport selector controlling how the MCP client connects to the
/// session-manager.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum McpClientTransport {
    /// Force WS-only mode. Resolution fails if the manager is unreachable.
    Ws,
    /// Force the local HTTP MCP transport (`runMcp[=...][;mcpPort=...]`).
    Mcp,
    /// Probe the manager: WS when reachable, local HTTP MCP otherwise.
    #[default]
    Auto,
}

impl McpClientTransport {
    pub fn from_str_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ws" => Some(Self::Ws),
            "mcp" => Some(Self::Mcp),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

/// Internal client kind values selected by entry-point. Never exposed via CLI
/// flags; see the task brief for the reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    V8RunnerClient,
    VanessaTestClient,
    YaxunitRunner,
}

impl ClientKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V8RunnerClient => "v8_runner_client",
            Self::VanessaTestClient => "vanessa_test_client",
            Self::YaxunitRunner => "yaxunit_runner",
        }
    }
}

/// Validated supported log-level values that the BSL devkit accepts.
const ALLOWED_LOG_LEVELS: &[&str] = &["off", "error", "warn", "info", "debug", "trace"];

/// Returns `true` when the value is one of the levels accepted by the devkit.
pub fn is_supported_log_level(level: &str) -> bool {
    ALLOWED_LOG_LEVELS
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(level))
}

/// Returns `true` when a value can be embedded into the semicolon-delimited
/// `/C` payload without changing its key/value structure.
pub fn is_payload_token_safe(value: &str) -> bool {
    !value.contains([';', '='])
}

/// Result of resolving the WS-mode connection parameters before launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsLaunchParams {
    pub manager_url: String,
    pub client_uid: String,
    pub kind: ClientKind,
    pub corr_id: String,
    pub log_level: String,
    pub ws_timeout_ms: u64,
}

impl WsLaunchParams {
    /// Builds the `mcpMode=ws;...` snippet suitable for inclusion into the
    /// platform's `/C` payload.
    pub fn payload_snippet(&self) -> String {
        format!(
            "mcpMode=ws;manager_url={};client_uid={};kind={};corr_id={};mcp_log_level={};mcp_ws_timeout_ms={}",
            encode_payload_token(&self.manager_url),
            encode_payload_token(&self.client_uid),
            self.kind.as_str(),
            encode_payload_token(&self.corr_id),
            encode_payload_token(&self.log_level),
            self.ws_timeout_ms
        )
    }
}

fn encode_payload_token(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(';', "%3B")
        .replace('=', "%3D")
}

/// Inputs to [`resolve_ws_params`]. Each field carries either an explicit
/// override (CLI takes precedence over config), or `None` to fall back to the
/// internal defaults.
#[derive(Debug, Clone, Default)]
pub struct WsResolveInputs {
    pub manager_url: Option<String>,
    pub client_uid: Option<String>,
    pub corr_id: Option<String>,
    pub log_level: Option<String>,
    pub ws_timeout_ms: Option<u64>,
}

impl WsResolveInputs {
    pub fn manager_url_or_default(&self) -> String {
        self.manager_url
            .clone()
            .unwrap_or_else(|| DEFAULT_MANAGER_URL.to_owned())
    }
}

/// Resolves the final WS launch parameters for the given client kind.
pub fn resolve_ws_params(kind: ClientKind, inputs: WsResolveInputs) -> WsLaunchParams {
    let manager_url = inputs.manager_url_or_default();
    let client_uid = inputs
        .client_uid
        .filter(|uid| !uid.trim().is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let corr_id = inputs
        .corr_id
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(|| default_corr_id(&client_uid));
    let log_level = inputs
        .log_level
        .filter(|l| !l.trim().is_empty())
        .map(|level| level.to_ascii_lowercase())
        .unwrap_or_else(|| DEFAULT_MCP_LOG_LEVEL.to_owned());
    let ws_timeout_ms = inputs.ws_timeout_ms.unwrap_or(DEFAULT_MCP_WS_TIMEOUT_MS);
    WsLaunchParams {
        manager_url,
        client_uid,
        kind,
        corr_id,
        log_level,
        ws_timeout_ms,
    }
}

fn default_corr_id(client_uid: &str) -> String {
    let mut short: String = client_uid.chars().filter(|c| *c != '-').take(8).collect();
    while short.len() < 8 {
        short.push('0');
    }
    format!("vr-{short}")
}

/// Errors that can be produced while resolving the WS endpoint or probing it.
#[derive(Debug, thiserror::Error)]
pub enum WsResolveError {
    #[error("invalid manager_url '{url}': {reason}")]
    InvalidManagerUrl { url: String, reason: String },
    #[error("session-manager unreachable at {url}")]
    Unreachable { url: String },
}

/// Decision returned by [`select_transport`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportDecision {
    Ws,
    Mcp,
}

/// Selects the effective transport given the requested mode and a `probe`
/// callback that returns `true` when the manager TCP endpoint is reachable.
pub fn select_transport<F>(
    requested: McpClientTransport,
    manager_url: &str,
    probe: F,
) -> Result<TransportDecision, WsResolveError>
where
    F: FnOnce(SocketAddr) -> bool,
{
    match requested {
        McpClientTransport::Mcp => Ok(TransportDecision::Mcp),
        McpClientTransport::Ws => {
            let addr = parse_manager_addr(manager_url)?;
            if probe(addr) {
                Ok(TransportDecision::Ws)
            } else {
                Err(WsResolveError::Unreachable {
                    url: manager_url.to_owned(),
                })
            }
        }
        McpClientTransport::Auto => {
            let addr = parse_manager_addr(manager_url)?;
            if probe(addr) {
                Ok(TransportDecision::Ws)
            } else {
                Ok(TransportDecision::Mcp)
            }
        }
    }
}

/// Default sync TCP probe used in production. Tries `connect_timeout` against
/// the resolved address.
pub fn probe_tcp(addr: SocketAddr, timeout: Duration) -> bool {
    TcpStream::connect_timeout(&addr, timeout).is_ok()
}

/// Parses the `host:port` portion of a `ws://host:port/path` URL.
///
/// Hostnames are intentionally rejected: transport auto-detection must stay
/// bounded by the 200 ms TCP probe, and synchronous DNS lookup would not share
/// that timeout budget.
pub fn parse_manager_addr(url: &str) -> Result<SocketAddr, WsResolveError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(WsResolveError::InvalidManagerUrl {
            url: url.to_owned(),
            reason: "empty url".to_owned(),
        });
    }
    let after_scheme = match trimmed.find("://") {
        Some(idx) => &trimmed[idx + 3..],
        None => trimmed,
    };
    let host_port = after_scheme.split('/').next().unwrap_or(after_scheme);
    if host_port.is_empty() {
        return Err(WsResolveError::InvalidManagerUrl {
            url: url.to_owned(),
            reason: "missing host:port".to_owned(),
        });
    }
    let Some((host, port)) = host_port.rsplit_once(':') else {
        return Err(WsResolveError::InvalidManagerUrl {
            url: url.to_owned(),
            reason: "missing :port".to_owned(),
        });
    };
    let host = host.trim_matches(['[', ']']);
    let ip = host
        .parse::<IpAddr>()
        .map_err(|_| WsResolveError::InvalidManagerUrl {
            url: url.to_owned(),
            reason: "host must be an IP address".to_owned(),
        })?;
    let port = port
        .parse::<u16>()
        .map_err(|err| WsResolveError::InvalidManagerUrl {
            url: url.to_owned(),
            reason: err.to_string(),
        })?;
    Ok(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn transport_from_str_accepts_known_values() {
        assert_eq!(
            McpClientTransport::from_str_value("ws"),
            Some(McpClientTransport::Ws)
        );
        assert_eq!(
            McpClientTransport::from_str_value("MCP"),
            Some(McpClientTransport::Mcp)
        );
        assert_eq!(McpClientTransport::from_str_value("legacy"), None);
        assert_eq!(
            McpClientTransport::from_str_value("auto"),
            Some(McpClientTransport::Auto)
        );
        assert_eq!(McpClientTransport::from_str_value("nope"), None);
    }

    #[test]
    fn parse_manager_addr_extracts_host_port() {
        let addr = parse_manager_addr("ws://127.0.0.1:4000/sessions").expect("parse");
        assert_eq!(addr.port(), 4000);
    }

    #[test]
    fn parse_manager_addr_requires_port() {
        let err = parse_manager_addr("ws://127.0.0.1/sessions").expect_err("rejected");
        assert!(matches!(err, WsResolveError::InvalidManagerUrl { .. }));
    }

    #[test]
    fn parse_manager_addr_rejects_empty() {
        let err = parse_manager_addr("").expect_err("rejected");
        assert!(matches!(err, WsResolveError::InvalidManagerUrl { .. }));
    }

    #[test]
    fn select_transport_mcp_short_circuits() {
        let decision = select_transport(
            McpClientTransport::Mcp,
            "ws://127.0.0.1:4000/sessions",
            |_| panic!("probe must not be called"),
        )
        .expect("mcp");
        assert_eq!(decision, TransportDecision::Mcp);
    }

    #[test]
    fn select_transport_auto_falls_back_to_mcp_when_unreachable() {
        let decision = select_transport(
            McpClientTransport::Auto,
            "ws://127.0.0.1:4000/sessions",
            |_| false,
        )
        .expect("auto-fallback");
        assert_eq!(decision, TransportDecision::Mcp);
    }

    #[test]
    fn select_transport_ws_errors_when_unreachable() {
        let err = select_transport(
            McpClientTransport::Ws,
            "ws://127.0.0.1:4000/sessions",
            |_| false,
        )
        .expect_err("ws-required");
        assert!(matches!(err, WsResolveError::Unreachable { .. }));
    }

    #[test]
    fn select_transport_ws_uses_probe_result() {
        let decision = select_transport(
            McpClientTransport::Ws,
            "ws://127.0.0.1:4000/sessions",
            |_| true,
        )
        .expect("ws-up");
        assert_eq!(decision, TransportDecision::Ws);
    }

    #[test]
    fn probe_tcp_succeeds_against_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let addr = listener.local_addr().expect("local addr");
        assert!(probe_tcp(addr, Duration::from_millis(500)));
    }

    #[test]
    fn probe_tcp_fails_when_no_listener() {
        let addr = SocketAddr::from(([127, 0, 0, 1], 1));
        let connected = probe_tcp(addr, Duration::from_millis(50));
        assert!(!connected);
    }

    #[test]
    fn resolve_ws_params_uses_defaults_when_inputs_empty() {
        let params = resolve_ws_params(ClientKind::V8RunnerClient, WsResolveInputs::default());
        assert_eq!(params.manager_url, DEFAULT_MANAGER_URL);
        assert_eq!(params.kind, ClientKind::V8RunnerClient);
        assert_eq!(params.log_level, DEFAULT_MCP_LOG_LEVEL);
        assert_eq!(params.ws_timeout_ms, DEFAULT_MCP_WS_TIMEOUT_MS);
        assert!(!params.client_uid.is_empty());
        assert!(params.corr_id.starts_with("vr-"));
        assert_eq!(params.corr_id.len(), "vr-".len() + 8);
    }

    #[test]
    fn resolve_ws_params_honors_overrides() {
        let inputs = WsResolveInputs {
            manager_url: Some("ws://manager:5555/sessions".to_owned()),
            client_uid: Some("00000000-0000-0000-0000-000000000001".to_owned()),
            corr_id: Some("parent/vr-deadbeef".to_owned()),
            log_level: Some("debug".to_owned()),
            ws_timeout_ms: Some(2500),
        };
        let params = resolve_ws_params(ClientKind::YaxunitRunner, inputs);
        assert_eq!(params.manager_url, "ws://manager:5555/sessions");
        assert_eq!(params.client_uid, "00000000-0000-0000-0000-000000000001");
        assert_eq!(params.corr_id, "parent/vr-deadbeef");
        assert_eq!(params.log_level, "debug");
        assert_eq!(params.ws_timeout_ms, 2500);
        assert_eq!(params.kind, ClientKind::YaxunitRunner);
    }

    #[test]
    fn resolve_ws_params_pads_default_corr_id_for_short_uid() {
        let params = resolve_ws_params(
            ClientKind::V8RunnerClient,
            WsResolveInputs {
                client_uid: Some("abc".to_owned()),
                ..Default::default()
            },
        );
        assert_eq!(params.corr_id, "vr-abc00000");
    }

    #[test]
    fn payload_snippet_contains_all_keys_in_order() {
        let params = WsLaunchParams {
            manager_url: "ws://m:1/s".to_owned(),
            client_uid: "uid".to_owned(),
            kind: ClientKind::VanessaTestClient,
            corr_id: "c".to_owned(),
            log_level: "info".to_owned(),
            ws_timeout_ms: 1000,
        };
        assert_eq!(
            params.payload_snippet(),
            "mcpMode=ws;manager_url=ws://m:1/s;client_uid=uid;kind=vanessa_test_client;corr_id=c;mcp_log_level=info;mcp_ws_timeout_ms=1000"
        );
    }

    #[test]
    fn payload_snippet_encodes_delimiters() {
        let params = WsLaunchParams {
            manager_url: "ws://127.0.0.1:1/s".to_owned(),
            client_uid: "uid=1".to_owned(),
            kind: ClientKind::V8RunnerClient,
            corr_id: "corr;1".to_owned(),
            log_level: "info".to_owned(),
            ws_timeout_ms: 1000,
        };
        assert_eq!(
            params.payload_snippet(),
            "mcpMode=ws;manager_url=ws://127.0.0.1:1/s;client_uid=uid%3D1;kind=v8_runner_client;corr_id=corr%3B1;mcp_log_level=info;mcp_ws_timeout_ms=1000"
        );
    }

    #[test]
    fn is_supported_log_level_accepts_known_values() {
        for level in ["off", "error", "warn", "info", "debug", "trace"] {
            assert!(is_supported_log_level(level), "expected {level} supported");
        }
        assert!(is_supported_log_level("INFO"));
        assert!(!is_supported_log_level("verbose"));
    }

    #[test]
    fn client_kind_strings_match_session_manager_contract() {
        assert_eq!(ClientKind::V8RunnerClient.as_str(), "v8_runner_client");
        assert_eq!(
            ClientKind::VanessaTestClient.as_str(),
            "vanessa_test_client"
        );
        assert_eq!(ClientKind::YaxunitRunner.as_str(), "yaxunit_runner");
    }
}

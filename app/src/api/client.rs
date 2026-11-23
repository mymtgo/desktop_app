//! The one HTTP client every API call goes through: one `ureq` agent (rustls,
//! Mozilla's roots, or exactly the certificates of `MYMTGO_API_CA`). `send`
//! asks the gate before a socket opens, sets the call's timeouts and reads at
//! most the call's body limit. Blocking: call it from the `api` worker thread
//! or `spawn_blocking`, never from the IPC thread or an async task.

use std::time::Duration;

use mymtgo_api::{
    ApiConfig, ApiError, ApiRequest, ApiResponse, Body, ConnectKind, Credentials, GateState,
    Method, gate, request_url, wire_headers,
};
use ureq::Agent;
use ureq::tls::{PemItem, RootCerts, TlsConfig, TlsProvider};

#[derive(Clone)]
pub struct ApiClient {
    agent: Agent,
    base_url: String,
}

impl ApiClient {
    /// The client for a configuration. Fails only when `MYMTGO_API_CA` names
    /// a file that cannot be read or holds no certificate.
    pub fn new(config: &ApiConfig) -> Result<ApiClient, String> {
        let roots = match &config.ca_pem_path {
            Some(path) => trusted_roots(path)?,
            None => RootCerts::WebPki,
        };
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(roots)
            .build();
        let agent_config = Agent::config_builder()
            .http_status_as_error(false)
            .tls_config(tls)
            // API calls never redirect; following one could carry the
            // device key to another host. A 3xx comes back as a response.
            .max_redirects(0)
            .max_redirects_will_error(false)
            .build();
        Ok(ApiClient {
            agent: Agent::new_with_config(agent_config),
            base_url: config.base_url.clone(),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Send a request. A refused gate returns its error before any
    /// connection; a non-2xx answer is a response, not an error.
    pub fn send(
        &self,
        request: &ApiRequest,
        gate_state: &GateState,
        credentials: &Credentials,
    ) -> Result<ApiResponse, ApiError> {
        if let Some(refused) = gate(request.class, gate_state).refusal() {
            return Err(refused);
        }
        let url = request_url(&self.base_url, request);
        let mut builder = ureq::http::Request::builder()
            .method(http_method(request.method))
            .uri(url.as_str());
        for (name, value) in wire_headers(request, credentials)? {
            builder = builder.header(name, value);
        }
        let timeout = Some(Duration::from_millis(request.timeout_ms));
        let connect = Some(Duration::from_millis(request.connect_timeout_ms));
        let response = if matches!(request.body, Body::Empty) {
            let http_request = builder.body(()).map_err(bad_request)?;
            let configured = self
                .agent
                .configure_request(http_request)
                .timeout_global(timeout)
                .timeout_connect(connect)
                .build();
            self.agent.run(configured)
        } else {
            let http_request = builder
                .body(request.body.bytes().to_vec())
                .map_err(bad_request)?;
            let configured = self
                .agent
                .configure_request(http_request)
                .timeout_global(timeout)
                .timeout_connect(connect)
                .build();
            self.agent.run(configured)
        };
        let mut response = response.map_err(transport_error)?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
                (name.as_str().to_string(), value)
            })
            .collect();
        let body = response
            .body_mut()
            .with_config()
            .limit(request.body_limit)
            .read_to_vec()
            .map_err(transport_error)?;
        Ok(ApiResponse {
            status,
            headers,
            body,
        })
    }
}

fn http_method(method: Method) -> ureq::http::Method {
    match method {
        Method::Get => ureq::http::Method::GET,
        Method::Post => ureq::http::Method::POST,
        Method::Put => ureq::http::Method::PUT,
        Method::Delete => ureq::http::Method::DELETE,
    }
}

/// The certificates of a PEM file, as the only trusted roots.
fn trusted_roots(path: &str) -> Result<RootCerts, String> {
    let pem = std::fs::read(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    let mut certs = Vec::new();
    for item in ureq::tls::parse_pem(&pem) {
        match item {
            Ok(PemItem::Certificate(cert)) => certs.push(cert),
            Ok(_) => {}
            Err(error) => return Err(format!("cannot read {path}: {error}")),
        }
    }
    if certs.is_empty() {
        return Err(format!("{path} holds no certificate"));
    }
    Ok(RootCerts::new_with_certs(&certs))
}

fn bad_request(error: ureq::http::Error) -> ApiError {
    ApiError::Config(format!("the request could not be built: {error}"))
}

/// A transport failure as an `ApiError`. Messages come from ureq and the OS
/// (never from headers), so they hold no secret.
fn transport_error(error: ureq::Error) -> ApiError {
    match error {
        ureq::Error::Timeout(_) => ApiError::Timeout,
        ureq::Error::BodyExceedsLimit(_) => ApiError::TooLarge,
        ureq::Error::Io(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            ApiError::Timeout
        }
        // ureq's message can hold the whole URL with its query; a later
        // builder might put a code or token there, so it is never echoed.
        ureq::Error::BadUri(_) => ApiError::Config("bad URL".to_string()),
        ureq::Error::Http(error) => bad_request(error),
        other => {
            let kind = connect_kind(&other);
            tracing::warn!(kind = kind.name(), "the connection failed");
            ApiError::Connect(kind, other.to_string())
        }
    }
}

/// The kind of a connect failure, for the log (#508).
fn connect_kind(error: &ureq::Error) -> ConnectKind {
    match error {
        ureq::Error::HostNotFound => ConnectKind::Dns,
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_) => ConnectKind::Tls,
        ureq::Error::Io(io) => ConnectKind::from_io(io.kind(), &io.to_string()),
        _ => ConnectKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::stub::{Reply, Stub};
    use mymtgo_api::{AuthMode, RequestClass};
    use std::time::Instant;

    fn client_for(stub: &Stub) -> ApiClient {
        let config = ApiConfig {
            base_url: stub.base_url.clone(),
            host: "127.0.0.1".to_string(),
            client_id: None,
            ca_pem_path: None,
        };
        ApiClient::new(&config).expect("client")
    }

    fn open_gate() -> GateState {
        GateState {
            offline: false,
            linked: false,
        }
    }

    fn device() -> Credentials {
        Credentials {
            device_id: "qdevice-id".to_string(),
            api_key: Some("qdevice-key".to_string()),
            access_token: None,
        }
    }

    #[test]
    fn the_stub_sees_method_path_query_headers_and_body_as_built() {
        let stub = Stub::start(|_| Reply::json(200, r#"{"ok":true}"#));
        let mut request = ApiRequest::new(
            Method::Put,
            "/api/sync/slots",
            RequestClass::Community,
            AuthMode::DeviceOrBearer,
        );
        request.query = vec![("name".to_string(), "Qplayer one".to_string())];
        request.headers = vec![("X-Qextra".to_string(), "yes".to_string())];
        request.body = Body::Json(br#"{"deck":"q"}"#.to_vec());
        let response = client_for(&stub)
            .send(&request, &open_gate(), &device())
            .expect("sent");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, br#"{"ok":true}"#);
        assert_eq!(response.header("content-type"), Some("application/json"));

        let seen = stub.requests();
        assert_eq!(seen.len(), 1);
        let seen = &seen[0];
        assert_eq!(seen.method, "PUT");
        assert_eq!(seen.target, "/api/sync/slots?name=Qplayer+one");
        assert_eq!(seen.body, br#"{"deck":"q"}"#);
        for (name, value) in wire_headers(&request, &device()).expect("headers") {
            assert_eq!(seen.header(&name), Some(value.as_str()), "{name}");
        }
        assert_eq!(seen.header("accept"), Some("application/json"));
        assert_eq!(seen.header("x-api-key"), Some("qdevice-key"));
    }

    #[test]
    fn a_closed_gate_opens_no_connection() {
        let stub = Stub::start(|_| Reply::json(200, "{}"));
        let request = ApiRequest::new(
            Method::Get,
            "/api/status",
            RequestClass::Status,
            AuthMode::DeviceOrBearer,
        );
        let offline = GateState {
            offline: true,
            ..open_gate()
        };
        let error = client_for(&stub)
            .send(&request, &offline, &device())
            .expect_err("gated");
        assert_eq!(error, ApiError::Offline);
        let sync = ApiRequest::new(
            Method::Post,
            "/api/sync/manifest",
            RequestClass::Sync,
            AuthMode::Bearer,
        );
        let error = client_for(&stub)
            .send(&sync, &open_gate(), &device())
            .expect_err("gated");
        assert_eq!(error, ApiError::NotLinked);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(stub.connections(), 0);
    }

    #[test]
    fn a_non_2xx_is_a_response_not_an_error() {
        let stub = Stub::start(|_| Reply::json(422, r#"{"message":"Qbad"}"#));
        let request = ApiRequest::new(
            Method::Post,
            "/api/account/players",
            RequestClass::Community,
            AuthMode::DeviceOrBearer,
        );
        let response = client_for(&stub)
            .send(&request, &open_gate(), &device())
            .expect("a response");
        assert_eq!(response.status, 422);
        assert!(!response.is_success());
        assert_eq!(response.body, br#"{"message":"Qbad"}"#);
    }

    #[test]
    fn a_silent_server_times_out_within_the_call_timeout() {
        let stub = Stub::start(|_| Reply::Silent);
        let mut request = ApiRequest::new(
            Method::Get,
            "/api/status",
            RequestClass::Status,
            AuthMode::DeviceOrBearer,
        );
        request.timeout_ms = 500;
        let started = Instant::now();
        let error = client_for(&stub)
            .send(&request, &open_gate(), &device())
            .expect_err("timed out");
        let took = started.elapsed();
        assert_eq!(error, ApiError::Timeout);
        assert!(took < Duration::from_secs(1), "took {took:?}");
        assert_eq!(stub.requests().len(), 1, "the request did go out");
    }

    #[test]
    fn an_oversized_body_is_refused() {
        let stub = Stub::start(|_| Reply::Respond {
            status: 200,
            headers: Vec::new(),
            body: vec![b'q'; 2 * 1024 * 1024],
        });
        let request = ApiRequest::new(
            Method::Get,
            "/api/status",
            RequestClass::Status,
            AuthMode::DeviceOrBearer,
        );
        assert_eq!(request.body_limit, 1024 * 1024);
        let error = client_for(&stub)
            .send(&request, &open_gate(), &device())
            .expect_err("too large");
        assert_eq!(error, ApiError::TooLarge);
    }

    #[test]
    fn malformed_json_is_a_typed_error() {
        let stub = Stub::start(|_| Reply::json(200, r#"{"api_key": "qhalf"#));
        let request = ApiRequest::new(
            Method::Post,
            "/api/devices/register",
            RequestClass::Registration,
            AuthMode::None,
        );
        let response = client_for(&stub)
            .send(&request, &open_gate(), &device())
            .expect("a response");
        let error = response.json::<serde_json::Value>().expect_err("malformed");
        assert!(matches!(error, ApiError::Malformed(_)), "{error:?}");
    }

    #[test]
    fn nothing_listening_is_a_connect_error() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("address").port()
        };
        let config = ApiConfig {
            base_url: format!("http://127.0.0.1:{port}"),
            host: "127.0.0.1".to_string(),
            client_id: None,
            ca_pem_path: None,
        };
        let request = ApiRequest::new(
            Method::Get,
            "/api/status",
            RequestClass::Status,
            AuthMode::DeviceOrBearer,
        );
        let error = ApiClient::new(&config)
            .expect("client")
            .send(&request, &open_gate(), &device())
            .expect_err("refused");
        assert!(
            matches!(error, ApiError::Connect(ConnectKind::Refused, _)),
            "{error:?}"
        );
    }

    #[test]
    fn transport_failures_are_sorted_into_kinds() {
        assert_eq!(connect_kind(&ureq::Error::HostNotFound), ConnectKind::Dns);
        assert_eq!(
            connect_kind(&ureq::Error::Tls("handshake")),
            ConnectKind::Tls
        );
        let reset = ureq::Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset));
        assert_eq!(connect_kind(&reset), ConnectKind::Reset);
        assert_eq!(
            connect_kind(&ureq::Error::ConnectionFailed),
            ConnectKind::Other
        );
    }

    #[test]
    fn a_bad_uri_error_never_echoes_the_url() {
        let error = transport_error(ureq::Error::BadUri(
            "https://mymtgo.test/oauth?code=qsecret-code".to_string(),
        ));
        assert!(matches!(&error, ApiError::Config(_)), "{error:?}");
        assert!(!format!("{error} {error:?}").contains("qsecret-code"));
    }

    #[test]
    fn a_missing_or_empty_ca_file_is_refused() {
        let dir = std::env::temp_dir().join(format!("mymtgo-api-ca-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let empty = dir.join("empty.pem");
        std::fs::write(&empty, "not a certificate\n").expect("write");
        for path in [dir.join("missing.pem"), empty] {
            let config = ApiConfig {
                base_url: "https://mymtgo.test".to_string(),
                host: "mymtgo.test".to_string(),
                client_id: None,
                ca_pem_path: Some(path.display().to_string()),
            };
            assert!(ApiClient::new(&config).is_err(), "{}", path.display());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Hermes 正式 Client Surface：JSON-RPC/WebSocket Gateway 适配器。
//!
//! 本模块只传输用户消息、原生附件、Session/模型命令与 Hermes 事件；
//! 不构造 SophoNote system prompt，也不复制 Hermes 的历史、Skill 或 Memory。

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock, RwLock};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

use crate::agent::engine::EngineError;

pub const ENV_GATEWAY_URL: &str = "SOPHONOTE_HERMES_GATEWAY_URL";
pub const ENV_GATEWAY_TOKEN: &str = "SOPHONOTE_HERMES_GATEWAY_TOKEN";

#[derive(Debug, Clone)]
pub struct HermesGatewayEndpoint {
    pub ws_url: String,
    pub token: String,
}

impl HermesGatewayEndpoint {
    pub fn from_env() -> Option<Self> {
        // Release 只允许由 SophoNote 校验并拉起的包内 Runtime。Debug 保留
        // 显式 Gateway 附着用于协议开发，但这不是产品回退。
        #[cfg(not(debug_assertions))]
        return bundled_endpoint();

        #[cfg(debug_assertions)]
        {
            endpoint_from_process_env().or_else(bundled_endpoint)
        }
    }

    pub fn bundled(ws_url: String, token: String) -> Result<Self, EngineError> {
        let endpoint = Self { ws_url, token };
        endpoint.connection_url()?;
        Ok(endpoint)
    }

    pub fn install_bundled(endpoint: Self) {
        *endpoint_registry()
            .write()
            .expect("Hermes endpoint registry poisoned") = Some(endpoint);
    }

    pub fn clear_bundled() {
        *endpoint_registry()
            .write()
            .expect("Hermes endpoint registry poisoned") = None;
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    fn connection_url(&self) -> Result<String, EngineError> {
        let url = self.ws_url.trim();
        let loopback = url.starts_with("ws://127.0.0.1:")
            || url.starts_with("ws://localhost:")
            || url.starts_with("ws://[::1]:");
        if !loopback || !url.contains("/api/ws") {
            return Err(EngineError::Setup(
                "Hermes Gateway 仅允许连接环回地址的 /api/ws".into(),
            ));
        }
        let separator = if url.contains('?') { '&' } else { '?' };
        Ok(format!(
            "{url}{separator}token={}",
            percent_encode_query(&self.token)
        ))
    }

    /// 与 WebSocket Gateway 同源的 Hermes Dashboard REST 根地址。
    ///
    /// Hermes Desktop 的 MCP 创建、OAuth、探测与启停属于管理面 REST API，
    /// 会话和运行事件仍走 WebSocket。两者必须复用同一个环回 Host 与 Token，
    /// 避免 SophoNote 维护第二份 MCP 配置或凭据。
    pub fn dashboard_base_url(&self) -> Result<reqwest::Url, EngineError> {
        let mut url = reqwest::Url::parse(self.ws_url.trim())
            .map_err(|error| EngineError::Setup(format!("Hermes Gateway URL 无效: {error}")))?;
        let scheme = match url.scheme() {
            "ws" => "http",
            "wss" => "https",
            _ => {
                return Err(EngineError::Setup(
                    "Hermes Gateway URL 必须使用 ws:// 或 wss://".into(),
                ))
            }
        };
        url.set_scheme(scheme)
            .map_err(|_| EngineError::Setup("Hermes Dashboard URL scheme 无效".into()))?;
        let host = url.host_str().unwrap_or_default();
        if !matches!(host, "127.0.0.1" | "localhost" | "::1") || url.path() != "/api/ws" {
            return Err(EngineError::Setup(
                "Hermes Dashboard 管理面仅允许连接环回地址的 /api/ws 同源服务".into(),
            ));
        }
        url.set_path("/");
        url.set_query(None);
        url.set_fragment(None);
        Ok(url)
    }
}

fn endpoint_registry() -> &'static RwLock<Option<HermesGatewayEndpoint>> {
    static ENDPOINT: OnceLock<RwLock<Option<HermesGatewayEndpoint>>> = OnceLock::new();
    ENDPOINT.get_or_init(|| RwLock::new(None))
}

fn bundled_endpoint() -> Option<HermesGatewayEndpoint> {
    endpoint_registry().read().ok()?.clone()
}

#[cfg(debug_assertions)]
fn endpoint_from_process_env() -> Option<HermesGatewayEndpoint> {
    let ws_url = std::env::var(ENV_GATEWAY_URL).ok()?.trim().to_string();
    let token = std::env::var(ENV_GATEWAY_TOKEN).ok()?.trim().to_string();
    if ws_url.is_empty() || token.is_empty() {
        return None;
    }
    Some(HermesGatewayEndpoint { ws_url, token })
}

pub fn gateway_env_configured() -> bool {
    HermesGatewayEndpoint::from_env().is_some()
}

fn percent_encode_query(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

type GatewaySocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub struct HermesGatewayConnection {
    socket: GatewaySocket,
    next_id: u64,
    queued_events: VecDeque<Value>,
}

#[derive(Debug)]
pub enum GatewayControl {
    Approval { choice: String, all: bool },
    Clarify { request_id: String, answer: String },
}

fn controls() -> &'static Mutex<HashMap<String, tokio::sync::mpsc::UnboundedSender<GatewayControl>>>
{
    static CONTROLS: OnceLock<
        Mutex<HashMap<String, tokio::sync::mpsc::UnboundedSender<GatewayControl>>>,
    > = OnceLock::new();
    CONTROLS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_run_control(run_id: &str) -> tokio::sync::mpsc::UnboundedReceiver<GatewayControl> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    controls().lock().unwrap().insert(run_id.to_string(), tx);
    rx
}

pub fn unregister_run_control(run_id: &str) {
    controls().lock().unwrap().remove(run_id);
}

pub fn send_run_control(run_id: &str, control: GatewayControl) -> bool {
    controls()
        .lock()
        .unwrap()
        .get(run_id)
        .is_some_and(|sender| sender.send(control).is_ok())
}

impl HermesGatewayConnection {
    pub async fn connect(endpoint: &HermesGatewayEndpoint) -> Result<Self, EngineError> {
        let url = endpoint.connection_url()?;
        let (socket, _) = connect_async(url)
            .await
            .map_err(|error| EngineError::Unhealthy(format!("Gateway 连接失败: {error}")))?;
        let mut connection = Self {
            socket,
            next_id: 1,
            queued_events: VecDeque::new(),
        };
        let ready = connection
            .next_frame()
            .await?
            .ok_or_else(|| EngineError::Unhealthy("Gateway 未发送 ready 事件".into()))?;
        if event_type(&ready) != Some("gateway.ready") {
            return Err(EngineError::Unhealthy(format!(
                "Gateway 首帧不是 gateway.ready: {ready}"
            )));
        }
        Ok(connection)
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, EngineError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send_request(id, method, params).await?;
        loop {
            let frame = self
                .next_frame()
                .await?
                .ok_or_else(|| EngineError::Unhealthy(format!("Gateway 在等待 {method} 时断开")))?;
            if frame.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = frame.get("error") {
                    let message = error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("Gateway RPC 失败");
                    return Err(EngineError::Setup(format!("{method}: {message}")));
                }
                return Ok(frame.get("result").cloned().unwrap_or(Value::Null));
            }
            if event_type(&frame).is_some() {
                self.queued_events.push_back(frame);
            }
        }
    }

    pub async fn next_event(&mut self) -> Result<Option<Value>, EngineError> {
        if let Some(frame) = self.queued_events.pop_front() {
            return Ok(Some(frame));
        }
        loop {
            match self.next_frame().await? {
                Some(frame) if event_type(&frame).is_some() => return Ok(Some(frame)),
                Some(_) => continue,
                None => return Ok(None),
            }
        }
    }

    async fn send_request(
        &mut self,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<(), EngineError> {
        self.socket
            .send(Message::Text(
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(|error| EngineError::Unhealthy(format!("Gateway 写入失败: {error}")))
    }

    async fn next_frame(&mut self) -> Result<Option<Value>, EngineError> {
        loop {
            let Some(message) = self.socket.next().await else {
                return Ok(None);
            };
            match message {
                Ok(Message::Text(text)) => {
                    let value = serde_json::from_str::<Value>(&text).map_err(|error| {
                        EngineError::Setup(format!("Gateway JSON 无效: {error}"))
                    })?;
                    return Ok(Some(value));
                }
                Ok(Message::Binary(bytes)) => {
                    let value = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                        EngineError::Setup(format!("Gateway JSON 无效: {error}"))
                    })?;
                    return Ok(Some(value));
                }
                Ok(Message::Ping(data)) => {
                    self.socket
                        .send(Message::Pong(data))
                        .await
                        .map_err(|error| {
                            EngineError::Unhealthy(format!("Gateway pong 失败: {error}"))
                        })?;
                }
                Ok(Message::Close(_)) => return Ok(None),
                Ok(_) => {}
                Err(error) => {
                    return Err(EngineError::Unhealthy(format!("Gateway 读取失败: {error}")))
                }
            }
        }
    }
}

pub fn event_type(frame: &Value) -> Option<&str> {
    frame.get("params")?.get("type")?.as_str()
}

pub fn event_session_id(frame: &Value) -> Option<&str> {
    frame.get("params")?.get("session_id")?.as_str()
}

pub fn event_payload(frame: &Value) -> Value {
    frame
        .get("params")
        .and_then(|params| params.get("payload"))
        .cloned()
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_gateway_token_without_leaking_query_syntax() {
        assert_eq!(percent_encode_query("a+b c"), "a%2Bb%20c");
    }

    #[test]
    fn recognizes_gateway_event_envelope() {
        let frame = json!({
            "jsonrpc":"2.0",
            "method":"event",
            "params":{"type":"message.delta","session_id":"s1","payload":{"text":"hi"}}
        });
        assert_eq!(event_type(&frame), Some("message.delta"));
        assert_eq!(event_session_id(&frame), Some("s1"));
        assert_eq!(event_payload(&frame)["text"], "hi");
    }

    #[test]
    fn derives_loopback_dashboard_rest_origin_from_gateway() {
        let endpoint = HermesGatewayEndpoint {
            ws_url: "ws://127.0.0.1:9119/api/ws".into(),
            token: "secret".into(),
        };
        assert_eq!(
            endpoint.dashboard_base_url().unwrap().as_str(),
            "http://127.0.0.1:9119/"
        );
    }

    #[test]
    fn rejects_non_loopback_dashboard_management_origin() {
        let endpoint = HermesGatewayEndpoint {
            ws_url: "wss://example.com/api/ws".into(),
            token: "secret".into(),
        };
        assert!(endpoint.dashboard_base_url().is_err());
    }
}

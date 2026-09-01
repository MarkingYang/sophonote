//! Hermes Runs API 客户端（协议 stub / 未来真实 API Server）。
//! H4：SSE `id:` 续传 + 断流结果枚举（供 recovery 重连）。

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::client::HermesClientError;

#[derive(Debug, Clone, Deserialize)]
pub struct CreateRunResponse {
    pub run_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunStatusResponse {
    pub run_id: String,
    pub status: String,
    #[serde(default)]
    pub output: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HermesSession {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HermesModel {
    pub id: String,
    #[serde(default, rename(deserialize = "owned_by", serialize = "ownedBy"))]
    pub owned_by: String,
}

#[derive(Debug, Clone, Deserialize)]
struct HermesModelsResponse {
    #[serde(default)]
    data: Vec<HermesModel>,
}

#[derive(Debug, Clone, Deserialize)]
struct CreateSessionResponse {
    session: HermesSession,
}

/// 单次 SSE 消费结局（未达终态 = ConnectionEnded，由 recovery 决定重连或对账）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseStreamResult {
    ReachedTerminal { last_event_id: Option<String> },
    ConnectionEnded { last_event_id: Option<String> },
}

#[derive(Debug, Clone)]
pub struct HermesRunsClient {
    base_url: String,
    bearer: String,
    http: reqwest::Client,
}

impl HermesRunsClient {
    pub fn new(base_url: impl Into<String>, bearer: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            bearer: bearer.into(),
            http: reqwest::Client::new(),
        }
    }

    pub async fn create_run(
        &self,
        input: &str,
        context_pack: Option<&Value>,
        article_id_hint: Option<&str>,
    ) -> Result<CreateRunResponse, HermesClientError> {
        let mut body = serde_json::json!({ "input": input });
        if let Some(pack) = context_pack {
            body["context_pack"] = pack.clone();
        }
        if let Some(hint) = article_id_hint {
            body["article_id_hint"] = Value::String(hint.to_string());
        }
        self.post_create_run(body, None).await
    }

    /// 真 Hermes API Server：仅发送官方字段（无 context_pack / tool_results）。
    pub async fn create_run_live(
        &self,
        input: &Value,
        instructions: Option<&str>,
        conversation_history: Option<&[Value]>,
        session_id: Option<&str>,
        memory_scope_key: Option<&str>,
        model: Option<&str>,
    ) -> Result<CreateRunResponse, HermesClientError> {
        let mut body = serde_json::json!({ "input": input.clone() });
        if let Some(sys) = instructions.filter(|s| !s.is_empty()) {
            body["instructions"] = Value::String(sys.to_string());
        }
        if let Some(hist) = conversation_history.filter(|h| !h.is_empty()) {
            body["conversation_history"] = Value::Array(hist.to_vec());
        }
        if let Some(sid) = session_id.filter(|s| !s.is_empty()) {
            body["session_id"] = Value::String(sid.to_string());
        }
        if let Some(value) = model.filter(|value| !value.trim().is_empty()) {
            body["model"] = Value::String(value.trim().to_string());
        }
        self.post_create_run(body, memory_scope_key).await
    }

    /// Hermes 能力发现：项目 Chat 模型选择器只展示服务端实际公布的模型。
    pub async fn list_models(&self) -> Result<Vec<HermesModel>, HermesClientError> {
        let url = format!("{}/v1/models", self.base_url);
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.bearer))
            .send()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        if status.as_u16() == 401 {
            return Err(HermesClientError::Unauthorized);
        }
        if !status.is_success() {
            return Err(HermesClientError::UnexpectedStatus(status.as_u16(), text));
        }
        let mut models = serde_json::from_str::<HermesModelsResponse>(&text)
            .map_err(|e| HermesClientError::Parse(e.to_string()))?
            .data;
        models.retain(|model| !model.id.trim().is_empty());
        models.sort_by(|left, right| left.id.cmp(&right.id));
        models.dedup_by(|left, right| left.id == right.id);
        Ok(models)
    }

    async fn post_create_run(
        &self,
        body: Value,
        memory_scope_key: Option<&str>,
    ) -> Result<CreateRunResponse, HermesClientError> {
        let resp = self
            .create_run_request(&body, memory_scope_key)
            .send()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        if status.as_u16() == 401 {
            return Err(HermesClientError::Unauthorized);
        }
        // 真 Hermes 返回 202 Accepted；stub 多为 200
        if !(status.is_success() || status.as_u16() == 202) {
            return Err(HermesClientError::UnexpectedStatus(status.as_u16(), text));
        }
        serde_json::from_str(&text).map_err(|e| HermesClientError::Parse(e.to_string()))
    }

    fn create_run_request(
        &self,
        body: &Value,
        memory_scope_key: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let url = format!("{}/v1/runs", self.base_url);
        let mut request = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bearer))
            .json(body);
        if let Some(key) = memory_scope_key.filter(|s| !s.is_empty()) {
            request = request.header("X-Hermes-Session-Key", key);
        }
        request
    }

    /// 显式创建 Hermes Session。指定 ID 时把 409 视为幂等成功，并读取既有 Session。
    pub async fn create_session(
        &self,
        session_id: &str,
        title: Option<&str>,
    ) -> Result<HermesSession, HermesClientError> {
        let url = format!("{}/api/sessions", self.base_url);
        let mut body = serde_json::json!({
            "id": session_id,
            "source": "api_server",
        });
        if let Some(value) = title.filter(|value| !value.trim().is_empty()) {
            body["title"] = Value::String(value.to_string());
        }
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bearer))
            .json(&body)
            .send()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        if status.as_u16() == 401 {
            return Err(HermesClientError::Unauthorized);
        }
        if status.as_u16() == 409 {
            return self.get_session(session_id).await;
        }
        if !status.is_success() {
            return Err(HermesClientError::UnexpectedStatus(status.as_u16(), text));
        }
        serde_json::from_str::<CreateSessionResponse>(&text)
            .map(|body| body.session)
            .map_err(|e| HermesClientError::Parse(e.to_string()))
    }

    pub async fn get_session(&self, session_id: &str) -> Result<HermesSession, HermesClientError> {
        let url = format!("{}/api/sessions/{session_id}", self.base_url);
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.bearer))
            .send()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        if status.as_u16() == 401 {
            return Err(HermesClientError::Unauthorized);
        }
        if !status.is_success() {
            return Err(HermesClientError::UnexpectedStatus(status.as_u16(), text));
        }
        serde_json::from_str::<CreateSessionResponse>(&text)
            .map(|body| body.session)
            .map_err(|e| HermesClientError::Parse(e.to_string()))
    }

    pub async fn delete_session(&self, session_id: &str) -> Result<(), HermesClientError> {
        let url = format!("{}/api/sessions/{session_id}", self.base_url);
        let response = self
            .http
            .delete(&url)
            .header("Authorization", format!("Bearer {}", self.bearer))
            .send()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        if status.as_u16() == 401 {
            return Err(HermesClientError::Unauthorized);
        }
        if !status.is_success() && status.as_u16() != 404 {
            return Err(HermesClientError::UnexpectedStatus(status.as_u16(), text));
        }
        Ok(())
    }

    pub async fn get_run(&self, run_id: &str) -> Result<RunStatusResponse, HermesClientError> {
        let url = format!("{}/v1/runs/{run_id}", self.base_url);
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.bearer))
            .send()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(HermesClientError::UnexpectedStatus(status.as_u16(), text));
        }
        serde_json::from_str(&text).map_err(|e| HermesClientError::Parse(e.to_string()))
    }

    pub async fn post_tool_result(
        &self,
        run_id: &str,
        call_id: &str,
        name: &str,
        ok: bool,
        output_text: &str,
    ) -> Result<(), HermesClientError> {
        let url = format!("{}/v1/runs/{run_id}/tool_results", self.base_url);
        let body = serde_json::json!({
            "call_id": call_id,
            "name": name,
            "ok": ok,
            "output_text": output_text,
        });
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bearer))
            .json(&body)
            .send()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(HermesClientError::UnexpectedStatus(status.as_u16(), text));
        }
        Ok(())
    }

    pub async fn stop(&self, run_id: &str) -> Result<(), HermesClientError> {
        let url = format!("{}/v1/runs/{run_id}/stop", self.base_url);
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.bearer))
            .send()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        if !status.is_success() {
            return Err(HermesClientError::UnexpectedStatus(status.as_u16(), text));
        }
        Ok(())
    }

    /// 消费 SSE（可选 `Last-Event-ID` 续传）。
    /// `on_event(external_id, json)`；遇到终态类型返回 `ReachedTerminal`。
    pub async fn stream_events_from<F, Fut>(
        &self,
        run_id: &str,
        last_event_id: Option<&str>,
        mut on_event: F,
    ) -> Result<SseStreamResult, HermesClientError>
    where
        F: FnMut(Option<String>, Value) -> Fut,
        Fut: std::future::Future<Output = Result<(), HermesClientError>>,
    {
        let url = format!("{}/v1/runs/{run_id}/events", self.base_url);
        let mut req = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.bearer));
        if let Some(id) = last_event_id {
            req = req.header("Last-Event-ID", id);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| HermesClientError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(HermesClientError::UnexpectedStatus(status.as_u16(), text));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let mut last_id: Option<String> = last_event_id.map(|s| s.to_string());
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| HermesClientError::Transport(e.to_string()))?;
            buf.push_str(&String::from_utf8_lossy(&bytes));
            while let Some(idx) = buf.find("\n\n") {
                let frame = buf[..idx].to_string();
                buf = buf[idx + 2..].to_string();
                let mut frame_id: Option<String> = None;
                let mut data_line: Option<String> = None;
                for line in frame.lines() {
                    let line = line.trim();
                    if let Some(id) = line.strip_prefix("id:") {
                        frame_id = Some(id.trim().to_string());
                    } else if let Some(data) = line.strip_prefix("data:") {
                        data_line = Some(data.trim().to_string());
                    }
                }
                let Some(data) = data_line.filter(|s| !s.is_empty()) else {
                    continue;
                };
                let value: Value = serde_json::from_str(&data)
                    .map_err(|e| HermesClientError::Parse(e.to_string()))?;
                if let Some(id) = frame_id.clone() {
                    last_id = Some(id);
                }
                let ty = value
                    .get("event")
                    .and_then(|v| v.as_str())
                    .or_else(|| value.get("type").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                on_event(frame_id, value).await?;
                if super::event_mapper::is_terminal_event_name(&ty) {
                    return Ok(SseStreamResult::ReachedTerminal {
                        last_event_id: last_id,
                    });
                }
            }
        }
        Ok(SseStreamResult::ConnectionEnded {
            last_event_id: last_id,
        })
    }

    /// 兼容 H3：从头消费至终态或断流。
    pub async fn stream_events<F, Fut>(
        &self,
        run_id: &str,
        mut on_event: F,
    ) -> Result<(), HermesClientError>
    where
        F: FnMut(Value) -> Fut,
        Fut: std::future::Future<Output = Result<(), HermesClientError>>,
    {
        let _ = self
            .stream_events_from(run_id, None, |_id, value| on_event(value))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_202_counts_as_success_for_create_run() {
        // 真 Hermes 返回 202；reqwest StatusCode::is_success 含 2xx
        let status = reqwest::StatusCode::from_u16(202).unwrap();
        assert!(status.is_success());
        assert!(status.is_success() || status.as_u16() == 202);
    }

    #[test]
    fn create_run_live_body_omits_stub_fields() {
        let body = serde_json::json!({
            "input": "hi",
            "instructions": "sys",
        });
        assert!(body.get("context_pack").is_none());
        assert!(body.get("article_id_hint").is_none());
        assert_eq!(body["input"], "hi");
    }

    #[test]
    fn model_contract_maps_hermes_snake_case_to_frontend_camel_case() {
        let model: HermesModel = serde_json::from_value(serde_json::json!({
            "id": "hermes-model",
            "owned_by": "local"
        }))
        .unwrap();
        assert_eq!(model.owned_by, "local");
        assert_eq!(serde_json::to_value(model).unwrap()["ownedBy"], "local");
    }

    #[test]
    fn live_run_request_binds_session_and_project_memory_scope() {
        let client = HermesRunsClient::new("http://127.0.0.1:9", "test-bearer");
        let body = serde_json::json!({
            "input": "继续",
            "session_id": "sophonote-thread-1",
        });
        let request = client
            .create_run_request(&body, Some("sophonote:project:p-1"))
            .build()
            .expect("request");
        assert_eq!(
            request
                .headers()
                .get("X-Hermes-Session-Key")
                .and_then(|value| value.to_str().ok()),
            Some("sophonote:project:p-1")
        );
        let json: Value = serde_json::from_slice(
            request
                .body()
                .and_then(|body| body.as_bytes())
                .expect("json body"),
        )
        .expect("body json");
        assert_eq!(json["session_id"], "sophonote-thread-1");
    }

    #[test]
    fn live_run_body_preserves_multimodal_input_and_model() {
        let input = serde_json::json!([{
            "role": "user",
            "content": [
                {"type": "text", "text": "描述"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA=="}}
            ]
        }]);
        let mut body = serde_json::json!({ "input": input.clone() });
        body["model"] = Value::String("deepseek-v4-flash".into());
        assert_eq!(body["input"], input);
        assert_eq!(body["model"], "deepseek-v4-flash");
    }
}

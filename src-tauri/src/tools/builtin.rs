// ============================================================
// Track B · 智能体演进（AG-06 追加）：Spike 内置假工具
// 用途：Phase 1 Spike 采纳门禁的确定性工具面（§〇.3 第 2/4 条）——
// 让 RunController 在无真实领域服务可挂时即可跑通
// 「两轮工具调用 / 畸形参数 / 工具失败文本回填」路径。
//
// 铁律：假工具零 IO、零副作用、确定性输出；不触达 SQLite/.md（硬性限制④）。
// Phase 2 起被真实只读工具（item.get/document.search 等）逐步替换。
// ============================================================
use std::sync::Arc;

use async_trait::async_trait;

use super::{
    SophoNoteTool, ProvenanceRef, ToolDescriptor, ToolError, ToolOutput, ToolRegistry, UiArtifact,
};

/// 假工具一：查天气（固定返回，验「模型调用 → 结构化结果回填」链路）
pub struct GetWeatherTool;

#[async_trait]
impl SophoNoteTool for GetWeatherTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "get_weather".into(),
            description: "查询指定城市的当前天气（Spike 假数据，固定返回多云 26°C）".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string", "description": "城市名，如 杭州" }
                },
                "required": ["city"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let city = arguments
            .get("city")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| ToolError::InvalidArguments("缺少必填字符串参数 city".to_string()))?;
        let structured = serde_json::json!({
            "city": city,
            "condition": "多云",
            "temperature_c": 26
        });
        // AG-21：key-value 卡片 + Markdown 回退（前端不解析 model_text）
        let provenance = vec![ProvenanceRef::new("tool").with_id("get_weather")];
        let artifact = UiArtifact::new(
            "key-value",
            serde_json::json!({
                "rows": [
                    ["city", city],
                    ["condition", "多云"],
                    ["temperature_c", 26]
                ]
            }),
            format!("**{}**当前天气：多云，气温 26°C", city),
            provenance.clone(),
        )?;
        Ok(ToolOutput {
            model_text: format!("{}当前天气：多云，气温 26°C", city),
            structured,
            ui_artifact: Some(artifact),
            provenance,
            truncated: false,
        })
    }
}

/// 假工具二：计算器（真实运算，验「畸形参数拒绝 + 执行失败文本回填」两条护栏）
pub struct CalculatorTool;

#[async_trait]
impl SophoNoteTool for CalculatorTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "calculator".into(),
            description: "对两个数字做四则运算，op 取 add/subtract/multiply/divide".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["add", "subtract", "multiply", "divide"] },
                    "a": { "type": "number" },
                    "b": { "type": "number" }
                },
                "required": ["op", "a", "b"]
            }),
        }
    }

    async fn execute(&self, arguments: serde_json::Value) -> Result<ToolOutput, ToolError> {
        let op = arguments
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("缺少必填参数 op".to_string()))?;
        let a = take_number(&arguments, "a")?;
        let b = take_number(&arguments, "b")?;
        let (result, symbol) = match op {
            "add" => (a + b, "+"),
            "subtract" => (a - b, "-"),
            "multiply" => (a * b, "×"),
            "divide" => {
                if b == 0.0 {
                    return Err(ToolError::Execution("除数不能为零".to_string()));
                }
                (a / b, "÷")
            }
            other => {
                return Err(ToolError::InvalidArguments(format!(
                    "op 必须是 add/subtract/multiply/divide，收到: {}",
                    other
                )))
            }
        };
        // AG-21：key-value 卡片（表达式 + 结果）+ Markdown 回退
        let provenance = vec![ProvenanceRef::new("tool").with_id("calculator")];
        let artifact = UiArtifact::new(
            "key-value",
            serde_json::json!({
                "rows": [
                    ["expression", format!("{} {} {}", a, symbol, b)],
                    ["result", result]
                ]
            }),
            format!("{} {} {} = {}", a, symbol, b, result),
            provenance.clone(),
        )?;
        Ok(ToolOutput {
            model_text: format!("{} {} {} = {}", a, symbol, b, result),
            structured: serde_json::json!({ "op": op, "a": a, "b": b, "result": result }),
            ui_artifact: Some(artifact),
            provenance,
            truncated: false,
        })
    }
}

/// 数字参数提取：拒绝缺失与非数字（含字符串形态——模型常见畸形输出）
fn take_number(arguments: &serde_json::Value, key: &str) -> Result<f64, ToolError> {
    arguments
        .get(key)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ToolError::InvalidArguments(format!("参数 {} 缺失或不是数字", key)))
}

/// Spike 工具集装配（RunController 调试命令与单测共用同一入口）
pub fn spike_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(GetWeatherTool));
    registry.register(Arc::new(CalculatorTool));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn weather_returns_fixed_structured_payload() {
        let out = GetWeatherTool
            .execute(serde_json::json!({ "city": "杭州" }))
            .await
            .expect("执行成功");
        assert_eq!(out.structured["city"], "杭州");
        assert_eq!(out.structured["temperature_c"], 26);
        assert!(out.model_text.contains("杭州"));
        // AG-21：key-value 卡片 + 回退文本随五件套贯通
        let artifact = out.ui_artifact.expect("weather 应产卡片");
        assert_eq!(artifact.kind, "key-value");
        assert!(artifact.fallback_markdown.contains("杭州"));
        assert_eq!(out.provenance[0].source, "tool");
        assert!(!out.truncated);
    }

    #[tokio::test]
    async fn weather_rejects_missing_city() {
        let err = GetWeatherTool
            .execute(serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn calculator_computes_and_rejects_string_numbers() {
        let ok = CalculatorTool
            .execute(serde_json::json!({ "op": "add", "a": 2, "b": 3 }))
            .await
            .expect("执行成功");
        assert_eq!(ok.structured["result"], 5.0);

        let bad = CalculatorTool
            .execute(serde_json::json!({ "op": "add", "a": "not-a-number", "b": 2 }))
            .await
            .unwrap_err();
        assert!(matches!(bad, ToolError::InvalidArguments(_)));
    }

    #[tokio::test]
    async fn calculator_rejects_divide_by_zero_and_unknown_op() {
        let zero = CalculatorTool
            .execute(serde_json::json!({ "op": "divide", "a": 1, "b": 0 }))
            .await
            .unwrap_err();
        assert!(matches!(zero, ToolError::Execution(_)));

        let op = CalculatorTool
            .execute(serde_json::json!({ "op": "modulo", "a": 1, "b": 2 }))
            .await
            .unwrap_err();
        assert!(matches!(op, ToolError::InvalidArguments(_)));
    }
}

use super::{PayloadFilter, PipelineContext};
use serde_json::{json, Value};
use tracing::info;

/// 2단계: Hermes의 reasoning_effort 및 thinking 설정을 Qwen/MLX 템플릿 표준으로 정규화
pub struct ThinkingNormalizerFilter;

impl PayloadFilter for ThinkingNormalizerFilter {
    fn name(&self) -> &'static str {
        "ThinkingNormalizer"
    }

    fn apply(&self, ctx: &mut PipelineContext, payload: &mut Value) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let obj = match payload.as_object_mut() {
            Some(o) => o,
            None => return Ok(()),
        };

        // chat_template_kwargs 및 extra_body 파싱
        let mut chat_template_kwargs = obj
            .get("chat_template_kwargs")
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();

        if chat_template_kwargs.is_empty() {
            if let Some(extra) = obj.get("extra_body").and_then(|v| v.as_object()) {
                if let Some(extra_kwargs) = extra.get("chat_template_kwargs").and_then(|v| v.as_object()) {
                    chat_template_kwargs = extra_kwargs.clone();
                }
            }
        }

        // Hermes reasoning_effort 추출 및 표준화
        let raw_effort = obj
            .get("reasoning_effort")
            .and_then(|v| v.as_str())
            .or_else(|| {
                obj.get("extra_body")
                    .and_then(|e| e.get("reasoning_effort"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("")
            .to_string();

        let effort_lower = raw_effort.to_lowercase();
        if effort_lower == "none" || effort_lower == "off" || effort_lower == "false" || effort_lower == "0" {
            chat_template_kwargs.insert("enable_thinking".to_string(), json!(false));
            chat_template_kwargs.remove("reasoning_effort");
            obj.remove("reasoning_effort");
            info!(target: "pipeline", "🧠 [ThinkingNormalizer] Thinking OFF explicitly requested");
        } else if matches!(effort_lower.as_str(), "low" | "medium" | "high" | "minimal" | "extended" | "xhigh" | "max" | "ultra") {
            chat_template_kwargs.insert("enable_thinking".to_string(), json!(true));
            let normalized = match effort_lower.as_str() {
                "minimal" | "low" => "low",
                "xhigh" | "max" | "ultra" => "xhigh",
                _ => "medium",
            };
            chat_template_kwargs.insert("reasoning_effort".to_string(), json!(normalized));
            obj.insert("reasoning_effort".to_string(), json!(normalized));
            info!(target: "pipeline", "🧠 [ThinkingNormalizer] Thinking BALANCED (Hermes effort='{}' -> normalized='{}')", raw_effort, normalized);
        } else {
            // [기본값] 별도 지정 없으면 medium (약 150~300 토큰 CoT 후 즉답)
            chat_template_kwargs.insert("enable_thinking".to_string(), json!(true));
            chat_template_kwargs.insert("reasoning_effort".to_string(), json!("medium"));
            obj.insert("reasoning_effort".to_string(), json!("medium"));
            info!(target: "pipeline", "🧠 [ThinkingNormalizer] Thinking BALANCED (Default fallback -> reasoning_effort='medium')");
        }

        obj.insert("chat_template_kwargs".to_string(), Value::Object(chat_template_kwargs));
        ctx.record_modified(self.name());
        Ok(())
    }
}

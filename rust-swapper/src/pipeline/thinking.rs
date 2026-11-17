use super::{PayloadFilter, PipelineContext};
use serde_json::{json, Value};
use tracing::info;

/// 2단계: Client/Agent의 reasoning_effort 및 thinking 설정을 Qwen/MLX 템플릿 표준으로 정규화
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

        // 1. Agent의 빈 응답/추론 복구 넛지 턴 감지 시 Thinking 즉시 OFF (이미 끝난 추론을 바탕으로 즉시 본문 합성)
        let is_recovery_nudge = obj.get("messages")
            .and_then(|m| m.as_array())
            .and_then(|arr| arr.last())
            .map(|last_msg| {
                let role = last_msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
                let content = last_msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
                role == "user" && (
                    content.contains("추론이 완료되었습니다")
                    || content.contains("최종 답변 본문을 명확하게 작성")
                    || content.contains("_empty_recovery_synthetic")
                )
            })
            .unwrap_or(false);

        if is_recovery_nudge {
            chat_template_kwargs.insert("enable_thinking".to_string(), json!(false));
            chat_template_kwargs.remove("reasoning_effort");
            obj.remove("reasoning_effort");
            obj.insert("chat_template_kwargs".to_string(), Value::Object(chat_template_kwargs));
            info!(target: "pipeline", "🧠 [ThinkingNormalizer] Recovery nudge detected -> Force thinking OFF for direct answer synthesis");
            ctx.record_modified(self.name());
            return Ok(());
        }

        // 2. chat_template_kwargs에 enable_thinking: false가 명시된 경우 존중
        let explicit_thinking_off = chat_template_kwargs
            .get("enable_thinking")
            .and_then(|v| v.as_bool())
            == Some(false);

        if explicit_thinking_off {
            chat_template_kwargs.insert("enable_thinking".to_string(), json!(false));
            chat_template_kwargs.remove("reasoning_effort");
            obj.remove("reasoning_effort");
            obj.insert("chat_template_kwargs".to_string(), Value::Object(chat_template_kwargs));
            info!(target: "pipeline", "🧠 [ThinkingNormalizer] Thinking OFF via chat_template_kwargs");
            ctx.record_modified(self.name());
            return Ok(());
        }

        // reasoning_effort 추출 및 표준화
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
            info!(target: "pipeline", "🧠 [ThinkingNormalizer] Thinking BALANCED (Client effort='{}' -> normalized='{}')", raw_effort, normalized);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thinking_normalizer_recovery_nudge() {
        let filter = ThinkingNormalizerFilter;
        let mut ctx = PipelineContext::new("qwen");

        let mut payload = json!({
            "model": "any",
            "reasoning_effort": "medium",
            "messages": [
                {"role": "user", "content": "질문"},
                {"role": "assistant", "content": "(empty)"},
                {"role": "user", "content": "추론이 완료되었습니다. 위 추론 내용을 바탕으로 사용자에게 전달할 최종 답변 본문을 명확하게 작성해주세요."}
            ]
        });

        filter.apply(&mut ctx, &mut payload).unwrap();

        assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], false);
        assert!(payload.get("reasoning_effort").is_none());
        assert!(ctx.modified_layers.contains(&"ThinkingNormalizer"));
    }

    #[test]
    fn test_thinking_normalizer_explicit_chat_template_kwargs_off() {
        let filter = ThinkingNormalizerFilter;
        let mut ctx = PipelineContext::new("qwen");

        let mut payload = json!({
            "model": "any",
            "chat_template_kwargs": {
                "enable_thinking": false
            },
            "messages": [
                {"role": "user", "content": "질문"}
            ]
        });

        filter.apply(&mut ctx, &mut payload).unwrap();

        assert_eq!(payload["chat_template_kwargs"]["enable_thinking"], false);
        assert!(payload.get("reasoning_effort").is_none());
    }
}

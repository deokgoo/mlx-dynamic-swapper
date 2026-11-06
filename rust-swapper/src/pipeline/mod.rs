//! [Inbound Payload Pipeline Architecture]
//! Hermes Agent ➔ MLX 모델 사이에서 요청 페이로드를 A ➔ B ➔ C ➔ D ➔ E 순차 통과시키며
//! 안전하게 정규화, 오타 교정, 안티-루프 가이드 주입을 수행하는 파이프라인 엔진입니다.

pub mod anti_loop;
pub mod compaction;
pub mod model_route;
pub mod thinking;
pub mod tool_guard;

use serde_json::Value;
use tracing::{error, info};

pub use anti_loop::AntiLoopGuardFilter;
pub use compaction::CompactionInterceptorFilter;
pub use model_route::ModelRouteFilter;
pub use thinking::ThinkingNormalizerFilter;
pub use tool_guard::UniversalToolGuardFilter;

/// 파이프라인 실행 중 각 레이어 간에 공유되는 컨텍스트
#[derive(Debug, Clone)]
pub struct PipelineContext {
    pub target_model: String,
    pub is_compaction: bool,
    pub modified_layers: Vec<&'static str>,
}

impl PipelineContext {
    pub fn new(target_model: &str) -> Self {
        Self {
            target_model: target_model.to_string(),
            is_compaction: false,
            modified_layers: Vec::new(),
        }
    }

    pub fn record_modified(&mut self, layer_name: &'static str) {
        if !self.modified_layers.contains(&layer_name) {
            self.modified_layers.push(layer_name);
        }
    }
}

/// 모든 파이프라인 레이어가 구현하는 표준 인터페이스
pub trait PayloadFilter: Send + Sync {
    /// 레이어 명칭 (로그 및 관측성용)
    fn name(&self) -> &'static str;

    /// 레이어 활성화 여부
    fn is_enabled(&self, _ctx: &PipelineContext, _payload: &Value) -> bool {
        true
    }

    /// 페이로드 변환 실행
    fn apply(&self, ctx: &mut PipelineContext, payload: &mut Value) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

/// 등록된 필터들을 A ➔ B ➔ C 순서로 통과시키는 파이프라인 실행기
pub struct RequestPipeline {
    filters: Vec<Box<dyn PayloadFilter>>,
}

impl RequestPipeline {
    /// 기본 프로덕션 파이프라인 빌드
    pub fn build_default() -> Self {
        Self {
            filters: vec![
                Box::new(ModelRouteFilter),
                Box::new(ThinkingNormalizerFilter),
                Box::new(CompactionInterceptorFilter),
                Box::new(UniversalToolGuardFilter),
                Box::new(AntiLoopGuardFilter),
            ],
        }
    }

    /// 파이프라인 순차 실행
    pub fn execute(&self, target_model: &str, payload: &mut Value) {
        let mut ctx = PipelineContext::new(target_model);

        for filter in &self.filters {
            if filter.is_enabled(&ctx, payload) {
                if let Err(e) = filter.apply(&mut ctx, payload) {
                    error!(target: "pipeline", "❌ Layer '{}' execution failed: {}", filter.name(), e);
                }
            }
        }

        let chain_summary: Vec<&'static str> = self.filters.iter().map(|f| f.name()).collect();
        info!(
            target: "pipeline",
            "🚀 [Pipeline Passed] {} | Modified by: {:?}",
            chain_summary.join(" ➔ "),
            ctx.modified_layers
        );
    }
}

/// 레거시 및 간편 호출용 헬퍼 함수
#[allow(dead_code)]
pub fn process_chat_payload(payload: &mut Value, target_model: &str) {
    let pipeline = RequestPipeline::build_default();
    pipeline.execute(target_model, payload);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_pipeline_thinking_on() {
        let mut data = json!({
            "model": "any",
            "reasoning_effort": "high",
            "messages": [{"role": "user", "content": "hello"}]
        });

        process_chat_payload(&mut data, "/path/to/target/model");

        assert_eq!(data["model"], "/path/to/target/model");
        assert_eq!(data["reasoning_effort"], "medium");
        assert_eq!(data["chat_template_kwargs"]["enable_thinking"], true);
        assert_eq!(data["chat_template_kwargs"]["reasoning_effort"], "medium");
    }

    #[test]
    fn test_pipeline_thinking_off_explicit() {
        let mut data = json!({
            "model": "any",
            "reasoning_effort": "none",
            "messages": [{"role": "user", "content": "hello"}]
        });

        process_chat_payload(&mut data, "/path/to/target/model");

        assert_eq!(data["chat_template_kwargs"]["enable_thinking"], false);
        assert!(data.get("reasoning_effort").is_none());
    }

    #[test]
    fn test_pipeline_compaction() {
        let mut data = json!({
            "model": "any",
            "reasoning_effort": "high",
            "messages": [
                {"role": "user", "content": "Please summarize and compact the conversation state."}
            ]
        });

        process_chat_payload(&mut data, "/path/to/compact/model");

        assert_eq!(data["chat_template_kwargs"]["enable_thinking"], false);
        let content = data["messages"][0]["content"].as_str().unwrap();
        assert!(content.contains("[CRITICAL ARCHITECTURAL CONSTRAINTS FOR COMPACTION"));
    }

    #[test]
    fn test_pipeline_tool_guard_and_anti_loop() {
        let mut data = json!({
            "model": "any",
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "web_search",
                        "parameters": {
                            "properties": {
                                "query": { "type": "string" }
                            }
                        }
                    }
                }
            ],
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": "tc1",
                            "type": "function",
                            "function": {
                                "name": "web_search",
                                "arguments": "{\"queries\": \"[\\\"일본 면세\\\", \\\"환급\\\"]\"}"
                            }
                        }
                    ]
                },
                {
                    "role": "tool",
                    "name": "web_search",
                    "content": "{\"success\": false, \"error\": \"DDGSException: query is mandatory.\"}"
                },
                {
                    "role": "user",
                    "content": "계속 진행해줘"
                }
            ]
        });

        process_chat_payload(&mut data, "/path/to/target/model");

        // 1. 도구 인자 오타 교정 확인
        let args_str = data["messages"][0]["tool_calls"][0]["function"]["arguments"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(args_str).unwrap();
        assert_eq!(parsed["query"], "일본 면세");
        assert!(parsed.get("queries").is_none());

        // 2. 동적 안티-루프 가이드 주입 확인
        let last_content = data["messages"][2]["content"].as_str().unwrap();
        assert!(last_content.contains("[CRITICAL RUNTIME GUIDANCE — ANTI-LOOP GUARD]"));
        assert!(last_content.contains("web_search"));
    }

    #[test]
    fn test_pipeline_fuzzy_parameter_mapping_read_file() {
        let mut data = json!({
            "model": "any",
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "parameters": {
                            "properties": {
                                "path": { "type": "string" },
                                "offset": { "type": "integer" }
                            }
                        }
                    }
                }
            ],
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "id": "tc_rf",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"paths\": \"/Users/deokgoo/blog/index.ts\"}"
                            }
                        }
                    ]
                }
            ]
        });

        process_chat_payload(&mut data, "/path/to/target/model");

        let args_str = data["messages"][0]["tool_calls"][0]["function"]["arguments"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(args_str).unwrap();
        assert_eq!(parsed["path"], "/Users/deokgoo/blog/index.ts");
        assert!(parsed.get("paths").is_none());
    }
}

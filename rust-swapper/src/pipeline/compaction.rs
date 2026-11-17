use super::{PayloadFilter, PipelineContext};
use serde_json::{json, Value};
use tracing::info;

pub const STRUCTURED_COMPACTION_CONSTRAINTS: &str = "\n\n========================================\n\
[CRITICAL ARCHITECTURAL CONSTRAINTS FOR COMPACTION — HIGHEST PRIORITY]\n\
1. ABSOLUTELY FORBIDDEN: Do NOT output turn-by-turn logs or long numbered action lists (e.g. '1. READ...', '2. PATCH...', 'Turn 1...', 'Turn 80...').\n\
2. MANDATORY COLLAPSE OF HISTORICAL ACTIONS: If the previous summary contains a long list of numbered actions (e.g. 1..80), DO NOT PRESERVE OR REPRODUCE THEM. You MUST collapse all historical actions into at most 3-5 concise bullet points.\n\
3. MANDATORY 4-SECTION SYNTHESIS: Distill the active conversation state directly into these 4 concise sections:\n\
   ## 1. Core Goal & User Intent\n\
   - Primary objectives, user preferences, and scope of work.\n\
   ## 2. Current Architecture & Code State\n\
   - Concrete list of modified files, created files, active services, config parameters, and environment state. (Include exact paths).\n\
   ## 3. Discovered Facts & Issues\n\
   - Root causes identified, bugs resolved, and any active blockers or errors still requiring resolution.\n\
   ## 4. Next Immediate Steps\n\
   - Prioritized bullet points of the exact next actions to take.\n\
4. STRICT TOKEN BUDGET: Total output MUST be under 500 words (~600 tokens). High density, zero fluff, zero timeline duplication.\n\
========================================\n";

/// 3단계: 컨텍스트 압축(Compaction) 감지 시 Thinking 비활성화 및 4섹션 고밀도 압축 제약조건 주입
pub struct CompactionInterceptorFilter;

impl PayloadFilter for CompactionInterceptorFilter {
    fn name(&self) -> &'static str {
        "CompactionInterceptor"
    }

    fn apply(&self, ctx: &mut PipelineContext, payload: &mut Value) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let obj = match payload.as_object_mut() {
            Some(o) => o,
            None => return Ok(()),
        };

        let has_tools = obj.get("tools").and_then(|t| t.as_array()).map_or(false, |a| !a.is_empty());
        let mut is_compaction = false;

        if !has_tools {
            if let Some(messages) = obj.get("messages").and_then(|m| m.as_array()) {
                for msg in messages {
                    let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
                    let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
                    if (role == "system" || role == "user") && (content.contains("summarize") || content.contains("compact") || content.contains("요약")) {
                        is_compaction = true;
                        break;
                    }
                }
            }
        }

        if is_compaction {
            ctx.is_compaction = true;

            // 압축 모드에서는 Thinking 강제 해제 (순수 요약 집중)
            if let Some(kwargs) = obj.get_mut("chat_template_kwargs").and_then(|k| k.as_object_mut()) {
                kwargs.insert("enable_thinking".to_string(), json!(false));
                kwargs.remove("reasoning_effort");
            }
            obj.remove("reasoning_effort");

            info!(target: "pipeline", "🗜️ [CompactionInterceptor] Compaction detected -> Force thinking OFF & inject structured constraints");

            if let Some(messages) = obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
                if let Some(last_msg) = messages.last_mut().and_then(|m| m.as_object_mut()) {
                    if let Some(content) = last_msg.get_mut("content") {
                        if let Some(s) = content.as_str() {
                            let new_content = format!("{}{}", s, STRUCTURED_COMPACTION_CONSTRAINTS);
                            *content = json!(new_content);
                            ctx.record_modified(self.name());
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

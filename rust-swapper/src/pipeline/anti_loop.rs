use super::{PayloadFilter, PipelineContext};
use serde_json::{json, Value};
use tracing::info;

/// 5단계: [Anti-Loop Guard] 직전 도구 실패 시 동일 실패 앵무새 복제를 차단하는 동적 가이드 주입
pub struct AntiLoopGuardFilter;

impl PayloadFilter for AntiLoopGuardFilter {
    fn name(&self) -> &'static str {
        "AntiLoopGuard"
    }

    fn apply(&self, ctx: &mut PipelineContext, payload: &mut Value) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let obj = match payload.as_object_mut() {
            Some(o) => o,
            None => return Ok(()),
        };

        let messages = match obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
            Some(m) => m,
            None => return Ok(()),
        };

        if messages.is_empty() {
            return Ok(());
        }

        // 최근 메시지 중 마지막 tool 실행 결과 검사
        let mut failed_tool_info = None;
        for msg in messages.iter().rev().take(4) {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role == "tool" {
                let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let is_error = content.contains("\"success\": false")
                    || content.contains("Exception")
                    || content.contains("failed")
                    || content.contains("Error")
                    || content.contains("is mandatory");

                if is_error {
                    let tool_name = msg.get("name").and_then(|n| n.as_str()).unwrap_or("tool");
                    let error_snippet: String = content.chars().take(140).collect();
                    failed_tool_info = Some((tool_name.to_string(), error_snippet));
                    break;
                }
            }
        }

        if let Some((tool_name, error_snippet)) = failed_tool_info {
            let anti_loop_notice = format!(
                "\n\n[CRITICAL RUNTIME GUIDANCE — ANTI-LOOP GUARD]\n\
                Your previous call to '{}' encountered an error: {}\n\
                ⚠️ DO NOT repeat the exact same tool call or identical arguments.\n\
                You MUST either correct your parameter names and format, or switch to a different tool/strategy.\n",
                tool_name, error_snippet.replace('\n', " ")
            );

            // 마지막 사용자 메시지 또는 시스템 프롬프트에 주입
            if let Some(last_msg) = messages.last_mut().and_then(|m| m.as_object_mut()) {
                if let Some(content_val) = last_msg.get_mut("content") {
                    if let Some(s) = content_val.as_str() {
                        if !s.contains("[CRITICAL RUNTIME GUIDANCE — ANTI-LOOP GUARD]") {
                            *content_val = json!(format!("{}{}", s, anti_loop_notice));
                            info!(target: "pipeline", "🛡️ [AntiLoopGuard] Injected dynamic Anti-Loop guidance for recently failed tool '{}'", tool_name);
                            ctx.record_modified(self.name());
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

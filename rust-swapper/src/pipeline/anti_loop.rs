use super::{PayloadFilter, PipelineContext};
use serde_json::{json, Value};
use tracing::info;

/// 5단계: [Anti-Loop Guard]
/// 1) 도구가 성공했음에도 동일한 도구와 인자를 연속 호출하는 멱등 루프(Duplicate Tool-Call Loop) 감지 및 즉시 답변 강제
/// 2) 직전 도구 실패 시 동일 실패 앵무새 복제를 차단하는 동적 가이드 주입
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

        // 1. 중복 도구 호출(Duplicate Tool Calls) 루프 감지
        // 최근 assistant 메시지들의 tool_calls 수집 (최근 순서)
        let mut assistant_tool_calls: Vec<Vec<(String, String)>> = Vec::new();
        for msg in messages.iter().rev() {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
            if role == "assistant" {
                if let Some(tool_calls) = msg.get("tool_calls").and_then(|tc| tc.as_array()) {
                    let mut calls = Vec::new();
                    for call in tool_calls {
                        if let Some(func) = call.get("function") {
                            let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                            let raw_args = func.get("arguments").and_then(|a| a.as_str()).unwrap_or("");
                            // JSON 정규화 (공백/키 순서 무시 비교를 위함)
                            let normalized_args = match serde_json::from_str::<Value>(raw_args) {
                                Ok(v) => serde_json::to_string(&v).unwrap_or_else(|_| raw_args.trim().to_string()),
                                Err(_) => raw_args.trim().to_string(),
                            };
                            if !name.is_empty() {
                                calls.push((name, normalized_args));
                            }
                        }
                    }
                    if !calls.is_empty() {
                        assistant_tool_calls.push(calls);
                    }
                }
            }
            if assistant_tool_calls.len() >= 6 {
                break;
            }
        }

        let mut duplicate_tool_detected = None;

        // 패턴 A: 가장 최근 assistant 턴의 호출 중 하나가 바로 직전(그 이전) assistant 턴의 호출과 완전히 동일한 경우
        if assistant_tool_calls.len() >= 2 {
            let latest_calls = &assistant_tool_calls[0];
            let prev_calls = &assistant_tool_calls[1];

            for (name, args) in latest_calls {
                if prev_calls.iter().any(|(p_name, p_args)| p_name == name && p_args == args) {
                    duplicate_tool_detected = Some(name.clone());
                    break;
                }
            }
        }

        // 패턴 B: 최근 assistant 턴들 전체에서 동일한 (name, args)가 2회 이상 발견된 경우
        if duplicate_tool_detected.is_none() && !assistant_tool_calls.is_empty() {
            if let Some(latest_calls) = assistant_tool_calls.first() {
                for (name, args) in latest_calls {
                    let occurrences = assistant_tool_calls
                        .iter()
                        .filter(|turn| turn.iter().any(|(n, a)| n == name && a == args))
                        .count();
                    if occurrences >= 2 {
                        duplicate_tool_detected = Some(name.clone());
                        break;
                    }
                }
            }
        }

        // 2. 도구 실행 에러(Tool Failure) 감지
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

        // 동적 가이드 프롬프트 작성 (중복 호출 감지 우선)
        let guidance = if let Some(dup_tool) = duplicate_tool_detected {
            Some(format!(
                "\n\n[CRITICAL RUNTIME GUIDANCE — DUPLICATE TOOL-CALL DETECTED]\n\
                You have ALREADY called '{}' with the exact same arguments and received valid results above.\n\
                ⚠️ DO NOT call '{}' with the same arguments again.\n\
                You MUST now synthesize the information from the results above and deliver your final answer to the user immediately.\n",
                dup_tool, dup_tool
            ))
        } else if let Some((tool_name, error_snippet)) = failed_tool_info {
            Some(format!(
                "\n\n[CRITICAL RUNTIME GUIDANCE — ANTI-LOOP GUARD]\n\
                Your previous call to '{}' encountered an error: {}\n\
                ⚠️ DO NOT repeat the exact same tool call or identical arguments.\n\
                You MUST either correct your parameter names and format, or switch to a different tool/strategy.\n",
                tool_name, error_snippet.replace('\n', " ")
            ))
        } else {
            None
        };

        if let Some(notice) = guidance {
            // 마지막 메시지의 content 끝에 주입
            if let Some(last_msg) = messages.last_mut().and_then(|m| m.as_object_mut()) {
                if let Some(content_val) = last_msg.get_mut("content") {
                    if let Some(s) = content_val.as_str() {
                        if !s.contains("[CRITICAL RUNTIME GUIDANCE") {
                            *content_val = json!(format!("{}{}", s, notice));
                            info!(target: "pipeline", "🛡️ [AntiLoopGuard] Injected guidance: {}", notice.lines().nth(1).unwrap_or(""));
                            ctx.record_modified(self.name());
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anti_loop_duplicate_call_detection() {
        let filter = AntiLoopGuardFilter;
        let mut ctx = PipelineContext::new("qwen");

        let mut payload = json!({
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": {
                            "name": "web_search",
                            "arguments": "{\"query\":\"AI news\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "c1",
                    "name": "web_search",
                    "content": "{\"success\": true, \"results\": [\"headline 1\"]}"
                },
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "c2",
                        "type": "function",
                        "function": {
                            "name": "web_search",
                            "arguments": "{\"query\": \"AI news\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "c2",
                    "name": "web_search",
                    "content": "{\"success\": true, \"results\": [\"headline 1\"]}"
                }
            ]
        });

        filter.apply(&mut ctx, &mut payload).unwrap();

        let last_content = payload["messages"][3]["content"].as_str().unwrap();
        assert!(last_content.contains("[CRITICAL RUNTIME GUIDANCE — DUPLICATE TOOL-CALL DETECTED]"));
        assert!(last_content.contains("You have ALREADY called 'web_search'"));
        assert!(ctx.modified_layers.contains(&"AntiLoopGuard"));
    }

    #[test]
    fn test_anti_loop_error_detection() {
        let filter = AntiLoopGuardFilter;
        let mut ctx = PipelineContext::new("qwen");

        let mut payload = json!({
            "messages": [
                {
                    "role": "tool",
                    "name": "execute_code",
                    "content": "Exception: SyntaxError at line 1"
                }
            ]
        });

        filter.apply(&mut ctx, &mut payload).unwrap();

        let last_content = payload["messages"][0]["content"].as_str().unwrap();
        assert!(last_content.contains("[CRITICAL RUNTIME GUIDANCE — ANTI-LOOP GUARD]"));
        assert!(last_content.contains("SyntaxError"));
    }

    #[test]
    fn test_anti_loop_no_duplicate_when_different_args() {
        let filter = AntiLoopGuardFilter;
        let mut ctx = PipelineContext::new("qwen");

        let mut payload = json!({
            "messages": [
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": {
                            "name": "web_search",
                            "arguments": "{\"query\":\"AI news\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "c1",
                    "name": "web_search",
                    "content": "{\"success\": true}"
                },
                {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "c2",
                        "type": "function",
                        "function": {
                            "name": "web_search",
                            "arguments": "{\"query\":\"Rust 2024\"}"
                        }
                    }]
                },
                {
                    "role": "tool",
                    "tool_call_id": "c2",
                    "name": "web_search",
                    "content": "{\"success\": true}"
                }
            ]
        });

        filter.apply(&mut ctx, &mut payload).unwrap();

        let last_content = payload["messages"][3]["content"].as_str().unwrap();
        assert!(!last_content.contains("[CRITICAL RUNTIME GUIDANCE"));
        assert!(!ctx.modified_layers.contains(&"AntiLoopGuard"));
    }
}

//! [Payload Processing & Hermes Adaptation Module]
//! Hermes Agent의 reasoning_effort, thinking 파라미터 매핑 및 컨텍스트 압축 시 프롬프트 주입 로직을 담당합니다.
//! 
//! 💡 [Python vs Rust 비교 학습]
//! - Python:
//!     chat_template_kwargs = _data.get("chat_template_kwargs") or {}
//!     chat_template_kwargs["enable_thinking"] = True
//!   -> 파이썬은 dict에 임의의 키/값을 자유롭게 넣고 뺄 수 있어 편리하지만, KeyError나 타입 불일치 버그가 런타임에 발생합니다.
//! - Rust:
//!     serde_json::Value의 enum(Object, Array, String 등)을 패턴 매칭하거나
//!     `value.as_object_mut()`를 통해 안전하게 가변 참조를 얻어 조작합니다.

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
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

/// Hermes 페이로드의 reasoning / thinking 설정 파싱 및 컨텍스트 압축 가공
pub fn process_chat_payload(data: &mut Value, target_model_path: &str) {
    let obj = match data.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // 1. 모델 명칭을 타겟 모델 경로로 일치화
    obj.insert("model".to_string(), json!(target_model_path));

    // 2. chat_template_kwargs 및 extra_body 파싱
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

    // 3. Hermes reasoning_effort 추출 및 표준화
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
        info!(target: "payload", "🧠 Thinking OFF explicitly requested");
    } else if matches!(effort_lower.as_str(), "low" | "medium" | "high" | "minimal" | "extended" | "xhigh" | "max" | "ultra") {
        chat_template_kwargs.insert("enable_thinking".to_string(), json!(true));
        let normalized = match effort_lower.as_str() {
            "minimal" | "low" => "low",
            "xhigh" | "max" | "ultra" => "xhigh",
            // [Speed & Intelligence Balance] 일상적 대화/코딩에서는 과도한 대기(35초+)를 막기 위해 medium으로 집중!
            _ => "medium",
        };
        chat_template_kwargs.insert("reasoning_effort".to_string(), json!(normalized));
        obj.insert("reasoning_effort".to_string(), json!(normalized));
        info!(target: "payload", "🧠 Thinking BALANCED (Hermes effort='{}' -> normalized='{}')", raw_effort, normalized);
    } else {
        // [Thinking 기본값] 별도 지정 없으면 황금 밸런스인 medium (약 150~300 토큰 생각 후 즉답)
        chat_template_kwargs.insert("enable_thinking".to_string(), json!(true));
        chat_template_kwargs.insert("reasoning_effort".to_string(), json!("medium"));
        obj.insert("reasoning_effort".to_string(), json!("medium"));
        info!(target: "payload", "🧠 Thinking BALANCED (Default fallback -> reasoning_effort='medium')");
    }

    // 4. 컨텍스트 압축(Compaction / Summarization) 감지
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
        chat_template_kwargs.insert("enable_thinking".to_string(), json!(false));
        chat_template_kwargs.remove("reasoning_effort");
        obj.remove("reasoning_effort");
        info!(target: "payload", "🗜️ Context compaction detected -> Force thinking OFF & inject structured constraints");

        if let Some(messages) = obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
            if let Some(last_msg) = messages.last_mut().and_then(|m| m.as_object_mut()) {
                if let Some(content) = last_msg.get_mut("content") {
                    if let Some(s) = content.as_str() {
                        let new_content = format!("{}{}", s, STRUCTURED_COMPACTION_CONSTRAINTS);
                        *content = json!(new_content);
                    }
                }
            }
        }
    }

    obj.insert("chat_template_kwargs".to_string(), Value::Object(chat_template_kwargs));

    // 5. [Universal Tool-Call Guard & Self-Correction Engine]
    // 5-1. 도구 스키마의 필수 단수/복수 파라미터 경고 및 가이드 강화
    cleanse_tool_schemas(obj);

    // 5-2. 대화 히스토리 내의 도구 호출 인자 오타/단복수 불일치 자동 정규화
    cleanse_message_history_universal(obj);

    // 5-3. 직전 도구 실패 시 동일 실패 앵무새 복제를 차단하는 동적 안티-루프 가이드 주입
    inject_dynamic_anti_loop_guidance(obj);
}

/// 도구 스키마 목록에서 각 도구 이름과 유효한 파라미터 집합을 추출
fn extract_valid_tool_params(obj: &serde_json::Map<String, Value>) -> HashMap<String, HashSet<String>> {
    let mut map = HashMap::new();
    if let Some(tools) = obj.get("tools").and_then(|t| t.as_array()) {
        for tool in tools {
            if let Some(func) = tool.get("function").and_then(|f| f.as_object()) {
                if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                    let mut params_set = HashSet::new();
                    if let Some(params) = func.get("parameters").and_then(|p| p.as_object()) {
                        if let Some(props) = params.get("properties").and_then(|p| p.as_object()) {
                            for prop_key in props.keys() {
                                params_set.insert(prop_key.clone());
                            }
                        }
                    }
                    map.insert(name.to_string(), params_set);
                }
            }
        }
    }
    map
}

/// Hermes 도구 스키마에서 단/복수 혼동이 잦은 파라미터 가이드 강화
fn cleanse_tool_schemas(obj: &mut serde_json::Map<String, Value>) {
    if let Some(tools) = obj.get_mut("tools").and_then(|t| t.as_array_mut()) {
        for tool in tools {
            if let Some(func) = tool.get_mut("function").and_then(|f| f.as_object_mut()) {
                let name = func.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                if let Some(params) = func.get_mut("parameters").and_then(|p| p.as_object_mut()) {
                    if let Some(props) = params.get_mut("properties").and_then(|p| p.as_object_mut()) {
                        if name == "web_search" {
                            if let Some(query_prop) = props.get_mut("query").and_then(|q| q.as_object_mut()) {
                                query_prop.insert(
                                    "description".to_string(),
                                    json!("Search query string. CRITICAL: The argument MUST be named 'query' (string), NOT 'queries' or an array.")
                                );
                            }
                        } else if name == "read_file" || name == "patch" || name == "write_file" {
                            if let Some(path_prop) = props.get_mut("path").and_then(|p| p.as_object_mut()) {
                                path_prop.insert(
                                    "description".to_string(),
                                    json!("Target file path. CRITICAL: The argument MUST be named 'path' (string), NOT 'paths' or a list.")
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

/// 범용 파라미터 매핑: 스키마와 인자를 대조하여 단/복수 오타(-s, -es, _list 등)를 자동 교정
fn resolve_fuzzy_arguments(tool_name: &str, args_map: &mut serde_json::Map<String, Value>, valid_params: &HashSet<String>) -> bool {
    let mut renames = Vec::new();

    // 스키마에 정의된 정규 파라미터 P에 대해 매칭 시도
    for expected in valid_params {
        // 이미 올바른 파라미터가 유효한 값으로 존재하면 스킵
        if let Some(val) = args_map.get(expected) {
            if !val.is_null() && val.as_str().map_or(true, |s| !s.is_empty()) {
                continue;
            }
        }

        // 흔한 오타 후보들: queries -> query, paths -> path, commands -> command 등
        let plural_s = format!("{}s", expected);
        let plural_es = format!("{}es", expected);
        let plural_list = format!("{}_list", expected);

        let candidate_key = if args_map.contains_key(&plural_s) {
            Some(plural_s)
        } else if args_map.contains_key(&plural_es) {
            Some(plural_es)
        } else if args_map.contains_key(&plural_list) {
            Some(plural_list)
        } else {
            None
        };

        if let Some(cand) = candidate_key {
            renames.push((cand, expected.clone()));
        }
    }

    if renames.is_empty() {
        return false;
    }

    for (bad_key, expected_key) in renames {
        if let Some(val) = args_map.remove(&bad_key) {
            let unwrapped = unwrap_scalar_or_first(&val);
            args_map.insert(expected_key.clone(), unwrapped);
            info!(target: "payload", "🔧 [Universal Resolver] Tool '{}': corrected parameter '{}' -> '{}'", tool_name, bad_key, expected_key);
        }
    }
    true
}

/// 과거 대화록에 기록된 도구 호출의 오타 및 잘못된 복수형 인자를 올바른 스키마로 범용 교정
fn cleanse_message_history_universal(obj: &mut serde_json::Map<String, Value>) {
    let valid_tool_params = extract_valid_tool_params(obj);

    let messages = match obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
        Some(m) => m,
        None => return,
    };

    let mut fixed_count = 0;
    for msg in messages {
        if let Some(tool_calls) = msg.get_mut("tool_calls").and_then(|tc| tc.as_array_mut()) {
            for tc in tool_calls {
                if let Some(func) = tc.get_mut("function").and_then(|f| f.as_object_mut()) {
                    let tool_name = func.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                    let valid_params = valid_tool_params.get(&tool_name);

                    if let Some(args_val) = func.get_mut("arguments") {
                        if let Some(args_str) = args_val.as_str() {
                            if let Ok(mut args_obj) = serde_json::from_str::<Value>(args_str) {
                                if let Some(map) = args_obj.as_object_mut() {
                                    let mut changed = false;

                                    // 1. 스키마 기반 범용 매핑
                                    if let Some(v_params) = valid_params {
                                        if resolve_fuzzy_arguments(&tool_name, map, v_params) {
                                            changed = true;
                                        }
                                    }

                                    // 2. 알려진 Fallback 매핑 (스키마가 tools에 명시되지 않은 경우 대비)
                                    if tool_name == "web_search" && map.contains_key("queries") && (!map.contains_key("query") || map["query"].as_str().map_or(true, |s| s.is_empty())) {
                                        let q = unwrap_scalar_or_first(&map["queries"]);
                                        map.remove("queries");
                                        map.insert("query".to_string(), q);
                                        changed = true;
                                    }

                                    if changed {
                                        if let Ok(new_args_str) = serde_json::to_string(&args_obj) {
                                            *args_val = json!(new_args_str);
                                            fixed_count += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if fixed_count > 0 {
        info!(target: "payload", "🧹 [Universal Guard] Normalized {} tool calls in conversation history to prevent error cloning", fixed_count);
    }
}

/// 문자열 배열, 직렬화된 JSON 배열, 또는 단일 문자열에서 스칼라 값 또는 첫 번째 항목 추출
fn unwrap_scalar_or_first(val: &Value) -> Value {
    match val {
        Value::String(s) => {
            let trimmed = s.trim();
            // "[ \"a\", \"b\" ]" 형태의 직렬화된 JSON 배열 파싱
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(trimmed) {
                    if let Some(first) = arr.first() {
                        return first.clone();
                    }
                }
            }
            // "{\"a\": 1}" 형태의 직렬화된 객체 파싱
            if trimmed.starts_with('{') && trimmed.ends_with('}') {
                if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                    return parsed;
                }
            }
            Value::String(trimmed.to_string())
        }
        Value::Array(arr) => {
            if let Some(first) = arr.first() {
                first.clone()
            } else {
                json!("")
            }
        }
        other => other.clone(),
    }
}

/// 직전 도구 실행에서 에러가 발생했을 때, 모델이 동일한 실패를 앵무새처럼 복제하지 못하도록 안티-루프 가이드 주입
fn inject_dynamic_anti_loop_guidance(obj: &mut serde_json::Map<String, Value>) {
    let messages = match obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
        Some(m) => m,
        None => return,
    };

    if messages.is_empty() {
        return;
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

        // 마지막 사용자 메시지 또는 시스템 프롬프트에 주입하여 강력한 주의 집중 유도
        if let Some(last_msg) = messages.last_mut().and_then(|m| m.as_object_mut()) {
            if let Some(content_val) = last_msg.get_mut("content") {
                if let Some(s) = content_val.as_str() {
                    if !s.contains("[CRITICAL RUNTIME GUIDANCE — ANTI-LOOP GUARD]") {
                        *content_val = json!(format!("{}{}", s, anti_loop_notice));
                        info!(target: "payload", "🛡️ Injected dynamic Anti-Loop guidance for recently failed tool '{}'", tool_name);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_hermes_thinking_on() {
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
    fn test_hermes_thinking_default_unleashed() {
        let mut data = json!({
            "model": "any",
            "messages": [{"role": "user", "content": "hello"}]
        });

        process_chat_payload(&mut data, "/path/to/target/model");

        assert_eq!(data["reasoning_effort"], "medium");
        assert_eq!(data["chat_template_kwargs"]["enable_thinking"], true);
        assert_eq!(data["chat_template_kwargs"]["reasoning_effort"], "medium");
    }

    #[test]
    fn test_hermes_thinking_off_explicit() {
        let mut data = json!({
            "model": "any",
            "reasoning_effort": "none",
            "messages": [{"role": "user", "content": "hello"}]
        });

        process_chat_payload(&mut data, "/path/to/target/model");

        assert_eq!(data["chat_template_kwargs"]["enable_thinking"], false);
        assert!(data.get("reasoning_effort").is_none());
        assert!(data["chat_template_kwargs"].get("reasoning_effort").is_none());
    }

    #[test]
    fn test_compaction_prompt_injection() {
        let mut data = json!({
            "model": "any",
            "reasoning_effort": "high",
            "messages": [
                {"role": "user", "content": "Please summarize and compact the conversation state."}
            ]
        });

        process_chat_payload(&mut data, "/path/to/compact/model");

        // 컨텍스트 압축 감지 시 thinking은 강제로 꺼져야 함
        assert_eq!(data["chat_template_kwargs"]["enable_thinking"], false);
        assert!(data.get("reasoning_effort").is_none());

        // 마지막 메시지에 구조적 압축 제약조건이 주입되어야 함
        let content = data["messages"][0]["content"].as_str().unwrap();
        assert!(content.contains("[CRITICAL ARCHITECTURAL CONSTRAINTS FOR COMPACTION"));
    }

    #[test]
    fn test_cleanse_web_search_history_and_tools() {
        let mut data = json!({
            "model": "any",
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "web_search",
                        "parameters": {
                            "properties": {
                                "query": {
                                    "type": "string",
                                    "description": "old desc"
                                }
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
                                "arguments": "{\"queries\": \"[\\\"네이버 디스커버리\\\", \\\"언론사 노출\\\"]\"}"
                            }
                        }
                    ]
                }
            ]
        });

        process_chat_payload(&mut data, "/path/to/target/model");

        // 1. 도구 스키마 설명이 보강되었는지 검증
        let query_desc = data["tools"][0]["function"]["parameters"]["properties"]["query"]["description"].as_str().unwrap();
        assert!(query_desc.contains("CRITICAL: The argument MUST be named 'query'"));

        // 2. 과거 tool_calls의 잘못된 'queries'가 'query'로 치환되었는지 검증
        let args_str = data["messages"][0]["tool_calls"][0]["function"]["arguments"].as_str().unwrap();
        let parsed_args: Value = serde_json::from_str(args_str).unwrap();
        assert_eq!(parsed_args["query"], "네이버 디스커버리");
        assert!(parsed_args.get("queries").is_none());
    }

    #[test]
    fn test_fuzzy_parameter_mapping_read_file() {
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
        let parsed_args: Value = serde_json::from_str(args_str).unwrap();
        assert_eq!(parsed_args["path"], "/Users/deokgoo/blog/index.ts");
        assert!(parsed_args.get("paths").is_none());
    }

    #[test]
    fn test_dynamic_anti_loop_guidance_injection() {
        let mut data = json!({
            "model": "any",
            "messages": [
                {
                    "role": "assistant",
                    "content": "Searching..."
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

        let last_content = data["messages"][2]["content"].as_str().unwrap();
        assert!(last_content.contains("[CRITICAL RUNTIME GUIDANCE — ANTI-LOOP GUARD]"));
        assert!(last_content.contains("web_search"));
        assert!(last_content.contains("DO NOT repeat the exact same tool call"));
    }
}

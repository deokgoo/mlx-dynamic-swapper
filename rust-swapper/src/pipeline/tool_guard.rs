use super::{PayloadFilter, PipelineContext};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use tracing::info;

/// 4단계: [Universal Tool-Call Guard] 스키마 기반 파라미터 단/복수형 오타 자동 교정 및 이중 JSON 언패킹
pub struct UniversalToolGuardFilter;

impl PayloadFilter for UniversalToolGuardFilter {
    fn name(&self) -> &'static str {
        "UniversalToolGuard"
    }

    fn apply(&self, ctx: &mut PipelineContext, payload: &mut Value) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let obj = match payload.as_object_mut() {
            Some(o) => o,
            None => return Ok(()),
        };

        // 1. 도구 스키마의 필수 단수/복수 파라미터 경고 및 가이드 강화
        Self::cleanse_tool_schemas(obj);

        // 2. 대화 히스토리 내의 도구 호출 인자 오타/단복수 불일치 자동 정규화
        if Self::cleanse_message_history(obj) {
            ctx.record_modified(self.name());
        }

        Ok(())
    }
}

impl UniversalToolGuardFilter {
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

    /// 과거 대화록에 기록된 도구 호출의 오타 및 잘못된 복수형 인자를 올바른 스키마로 범용 교정
    fn cleanse_message_history(obj: &mut serde_json::Map<String, Value>) -> bool {
        let valid_tool_params = Self::extract_valid_tool_params(obj);

        let messages = match obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
            Some(m) => m,
            None => return false,
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
                                            if Self::resolve_fuzzy_arguments(&tool_name, map, v_params) {
                                                changed = true;
                                            }
                                        }

                                        // 2. 알려진 Fallback 매핑 (web_search)
                                        if tool_name == "web_search" && map.contains_key("queries") && (!map.contains_key("query") || map["query"].as_str().map_or(true, |s| s.is_empty())) {
                                            let q = Self::unwrap_scalar_or_first(&map["queries"]);
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
            info!(target: "pipeline", "🧹 [UniversalToolGuard] Normalized {} tool calls in conversation history to prevent error cloning", fixed_count);
            true
        } else {
            false
        }
    }

    /// 범용 파라미터 매핑: 스키마와 인자를 대조하여 단/복수 오타(-s, -es, _list 등)를 자동 교정
    fn resolve_fuzzy_arguments(tool_name: &str, args_map: &mut serde_json::Map<String, Value>, valid_params: &HashSet<String>) -> bool {
        let mut renames = Vec::new();

        for expected in valid_params {
            if let Some(val) = args_map.get(expected) {
                if !val.is_null() && val.as_str().map_or(true, |s| !s.is_empty()) {
                    continue;
                }
            }

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
                let unwrapped = Self::unwrap_scalar_or_first(&val);
                args_map.insert(expected_key.clone(), unwrapped);
                info!(target: "pipeline", "🔧 [UniversalToolGuard] Tool '{}': corrected parameter '{}' -> '{}'", tool_name, bad_key, expected_key);
            }
        }
        true
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
}

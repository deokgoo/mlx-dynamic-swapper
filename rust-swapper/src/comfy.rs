//! [ComfyUI Workflow & Execution Module]
//! ComfyUI 템플릿 로드, 파라미터(prompt, seed, steps 등) 주입 및 비동기 완료 감지 폴링을 수행합니다.

use reqwest::Client;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::info;

pub fn build_workflow(
    action: &str,
    params: &Value,
    workflows_dir: &Path,
    input_dir: &Path,
) -> Result<Value, String> {
    if action == "t2i" || action == "generate" {
        let template_path = workflows_dir.join("qwen_image_2_1_t2i.json");
        let content = std::fs::read_to_string(&template_path)
            .map_err(|e| format!("Failed to read t2i workflow template {:?}: {}", template_path, e))?;
        let mut wf: Value = serde_json::from_str(&content)
            .map_err(|e| format!("Invalid JSON in t2i template: {}", e))?;

        let prompt = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("A high quality artistic image");
        let neg_prompt = params
            .get("negative_prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("blurry, ugly, distorted, noisy, artifacts");
        let seed = params
            .get("seed")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() % 2147483647) as i64);
        let steps = params.get("steps").and_then(|v| v.as_i64()).unwrap_or(14);
        let resolution = params.get("resolution").and_then(|v| v.as_i64()).unwrap_or(1024);

        if let Some(node5) = wf.get_mut("5").and_then(|v| v.get_mut("inputs")) {
            node5["prompt"] = json!(prompt);
            node5["negative_prompt"] = json!(neg_prompt);
            node5["resolution"] = json!(resolution);
        }
        if let Some(node6) = wf.get_mut("6").and_then(|v| v.get_mut("inputs")) {
            node6["seed"] = json!(seed);
            node6["steps"] = json!(steps);
        }
        Ok(wf)
    } else if action == "edit" || action == "i2i" {
        let template_path = workflows_dir.join("qwen_image_2_1_edit.json");
        let content = std::fs::read_to_string(&template_path)
            .map_err(|e| format!("Failed to read edit workflow template {:?}: {}", template_path, e))?;
        let mut wf: Value = serde_json::from_str(&content)
            .map_err(|e| format!("Invalid JSON in edit template: {}", e))?;

        let input_image = params
            .get("image")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "'image' parameter is required for edit/i2i workflow".to_string())?;

        let image_path = PathBuf::from(input_image);
        let file_name = image_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "Invalid image file name".to_string())?;

        let dest_path = input_dir.join(file_name);
        if image_path.exists() && image_path != dest_path {
            let _ = std::fs::create_dir_all(input_dir);
            let _ = std::fs::copy(&image_path, &dest_path);
        }

        let mut prompt = params
            .get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("In <image1>, edit the image according to prompt")
            .to_string();
        if !prompt.contains("<image1>") {
            prompt = format!("In <image1>, {}", prompt);
        }

        let neg_prompt = params
            .get("negative_prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("blurry, ugly, distorted, noisy, artifacts");
        let seed = params
            .get("seed")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() % 2147483647) as i64);
        let steps = params.get("steps").and_then(|v| v.as_i64()).unwrap_or(14);

        if let Some(node4) = wf.get_mut("4").and_then(|v| v.get_mut("inputs")) {
            node4["image"] = json!(file_name);
        }
        if let Some(node5) = wf.get_mut("5").and_then(|v| v.get_mut("inputs")) {
            node5["prompt"] = json!(prompt);
            node5["negative_prompt"] = json!(neg_prompt);
        }
        if let Some(node6) = wf.get_mut("6").and_then(|v| v.get_mut("inputs")) {
            node6["seed"] = json!(seed);
            node6["steps"] = json!(steps);
        }
        Ok(wf)
    } else if action == "raw" || params.get("workflow").is_some() {
        Ok(params.get("workflow").cloned().unwrap_or_else(|| params.clone()))
    } else {
        Err(format!("Unknown workflow action: {}. Supported: t2i, edit, raw", action))
    }
}

pub async fn execute_workflow(
    client: &Client,
    comfy_port: u16,
    output_dir: &Path,
    prompt_payload: Value,
    timeout_secs: f64,
) -> Result<Value, String> {
    let client_id = format!("mlx-swapper-{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() % 0xffffffff);
    let queue_url = format!("http://127.0.0.1:{}/prompt", comfy_port);

    let body = json!({
        "prompt": prompt_payload,
        "client_id": client_id
    });

    // 1. 프롬프트 큐 제출
    let resp = client
        .post(&queue_url)
        .json(&body)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Failed to queue ComfyUI workflow: {}", e))?;

    if !resp.status().is_success() {
        let err_text = resp.text().await.unwrap_or_default();
        return Err(format!("ComfyUI returned error: {}", err_text));
    }

    let resp_data: Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse ComfyUI response: {}", e))?;

    let prompt_id = resp_data
        .get("prompt_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "ComfyUI response missing prompt_id".to_string())?
        .to_string();

    info!(target: "comfy", "✔ ComfyUI workflow queued: prompt_id={}", prompt_id);

    // 2. 완료 감지 폴링 루프
    let start_time = tokio::time::Instant::now();
    let timeout = Duration::from_secs_f64(timeout_secs);
    let history_url = format!("http://127.0.0.1:{}/history/{}", comfy_port, prompt_id);

    while start_time.elapsed() < timeout {
        tokio::time::sleep(Duration::from_millis(1000)).await;

        if let Ok(hist_resp) = client.get(&history_url).send().await {
            if hist_resp.status().is_success() {
                if let Ok(hist_data) = hist_resp.json::<Value>().await {
                    if let Some(task_info) = hist_data.get(&prompt_id) {
                        let status = task_info.get("status").cloned().unwrap_or(Value::Null);
                        let completed = status.get("completed").and_then(|v| v.as_bool()).unwrap_or(false);
                        let status_str = status.get("status_str").and_then(|v| v.as_str()).unwrap_or("");

                        if status_str == "error" {
                            return Err(format!("ComfyUI workflow execution error: {:?}", status));
                        }

                        if completed {
                            let mut images = Vec::new();
                            if let Some(outputs) = task_info.get("outputs").and_then(|v| v.as_object()) {
                                for (_node_id, node_out) in outputs {
                                    if let Some(img_list) = node_out.get("images").and_then(|v| v.as_array()) {
                                        for img in img_list {
                                            if let Some(fn_str) = img.get("filename").and_then(|v| v.as_str()) {
                                                let subfolder = img.get("subfolder").and_then(|v| v.as_str()).unwrap_or("");
                                                let full_path = output_dir.join(subfolder).join(fn_str);
                                                images.push(json!(full_path.to_string_lossy()));
                                            }
                                        }
                                    }
                                }
                            }

                            let elapsed = start_time.elapsed().as_secs_f64();
                            info!(target: "comfy", "✔ ComfyUI workflow completed in {:.2}s! Generated {} image(s)", elapsed, images.len());

                            return Ok(json!({
                                "prompt_id": prompt_id,
                                "completed": true,
                                "elapsed_seconds": (elapsed * 100.0).round() / 100.0,
                                "images": images
                            }));
                        }
                    }
                }
            }
        }
    }

    Err(format!("ComfyUI workflow timed out after {:.1}s", timeout_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raw_workflow_passthrough() {
        let raw_wf = json!({
            "3": {
                "class_type": "KSampler",
                "inputs": {"seed": 12345}
            }
        });
        let params = json!({
            "action": "raw",
            "workflow": raw_wf.clone()
        });
        let dummy_dir = PathBuf::from("/tmp");
        let result = build_workflow("raw", &params, &dummy_dir, &dummy_dir).unwrap();
        assert_eq!(result, raw_wf);
    }
}

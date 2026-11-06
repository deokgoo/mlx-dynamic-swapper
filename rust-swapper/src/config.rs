//! [Config Module]
//! Python의 전역 환경 변수 파싱 로직을 Rust의 타입 안전한 구조체(`Config`)로 캡슐화합니다.
//! 
//! 💡 [Python vs Rust 비교 학습]
//! - Python:
//!     MODEL_MAIN_PATH = os.environ.get("MODEL_MAIN_PATH", os.path.expanduser("~/.mlx-models/Qwen3.8-27B-4bit"))
//!     PORT_MAIN_INTERNAL = int(os.environ.get("PORT_MAIN_INTERNAL", 18234))
//!   -> 런타임에 언제든 타입 에러(ValueError)나 None 참조 위험이 존재합니다.
//! - Rust:
//!     std::env::var를 통해 불러오고, .unwrap_or_else() 또는 .parse()를 통해
//!     컴파일 타임에 철저하게 타입을 보장합니다.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub python_bin: PathBuf,
    pub model_main_path: PathBuf,
    pub model_mtp_path: PathBuf,
    pub model_compact_path: PathBuf,

    pub port_main_internal: u16,
    pub port_compact_internal: u16,

    pub proxy_port_main: u16,
    pub proxy_port_compact: u16,

    pub comfy_python_bin: PathBuf,
    pub comfy_main_py: PathBuf,
    pub comfy_port: u16,
    pub comfy_input_dir: PathBuf,
    pub comfy_output_dir: PathBuf,
    pub comfy_workflows_dir: PathBuf,

    pub log_dir: PathBuf,
    pub prefill_step_size: String,
    pub web_dist_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/deokgoo".to_string());
        let home_path = PathBuf::from(&home);

        let python_bin = std::env::var("PYTHON_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_path.join(".mlx-server/bin/python"));

        let model_main_path = std::env::var("MODEL_MAIN_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_path.join(".mlx-models/Qwen3.8-27B-4bit"));

        let model_mtp_path = std::env::var("MODEL_MTP_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_path.join(".mlx-models/Qwen3.8-27B-MTP-4bit"));

        let model_compact_path = std::env::var("MODEL_COMPACT_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_path.join(".mlx-models/gemma-4-26b-a4b-it-4bit"));

        let port_main_internal = std::env::var("PORT_MAIN_INTERNAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(18234);

        let port_compact_internal = std::env::var("PORT_COMPACT_INTERNAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(18235);

        let proxy_port_main = std::env::var("PROXY_PORT_MAIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1234);

        let proxy_port_compact = std::env::var("PROXY_PORT_COMPACT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1235);

        let comfy_python_bin = std::env::var("COMFY_PYTHON_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_path.join("ComfyUI-Installs/ComfyUI/ComfyUI/.venv/bin/python3"));

        let comfy_main_py = std::env::var("COMFY_MAIN_PY")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_path.join("ComfyUI-Installs/ComfyUI/ComfyUI/main.py"));

        let comfy_port = std::env::var("COMFY_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8188);

        let comfy_input_dir = std::env::var("COMFY_INPUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_path.join("ComfyUI-Shared/input"));

        let comfy_output_dir = std::env::var("COMFY_OUTPUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_path.join("ComfyUI-Shared/output"));

        let comfy_workflows_dir = std::env::var("COMFY_WORKFLOWS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_path.join(".hermes/skills/creative/comfyui/workflows"));

        let log_dir = std::env::var("MLX_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| home_path.join(".mlx-server"));

        let prefill_step_size = std::env::var("PREFILL_STEP_SIZE")
            .unwrap_or_else(|_| "1024".to_string());

        let web_dist_dir = std::env::var("WEB_DIST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("/Users/deokgoo/Documents/develop/mlx-dynamic-swapper/web/dist"));

        Self {
            python_bin,
            model_main_path,
            model_mtp_path,
            model_compact_path,
            port_main_internal,
            port_compact_internal,
            proxy_port_main,
            proxy_port_compact,
            comfy_python_bin,
            comfy_main_py,
            comfy_port,
            comfy_input_dir,
            comfy_output_dir,
            comfy_workflows_dir,
            log_dir,
            prefill_step_size,
            web_dist_dir,
        }
    }
}

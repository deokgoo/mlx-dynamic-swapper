//! [Config Module]
//! Configuration loader for mlx-dynamic-swapper.
//! Supports `config.toml`, environment variables, and sensible defaults.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use tracing::info;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TomlConfig {
    pub server: Option<ServerSection>,
    pub models: Option<ModelsSection>,
    pub comfyui: Option<ComfySection>,
    pub paths: Option<PathsSection>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServerSection {
    pub proxy_port_main: Option<u16>,
    pub proxy_port_compact: Option<u16>,
    pub port_main_internal: Option<u16>,
    pub port_compact_internal: Option<u16>,
    pub prefill_step_size: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ModelsSection {
    pub main_path: Option<String>,
    pub qwen_path: Option<String>,
    pub mtp_path: Option<String>,
    pub compact_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ComfySection {
    pub python_bin: Option<String>,
    pub main_py: Option<String>,
    pub port: Option<u16>,
    pub input_dir: Option<String>,
    pub output_dir: Option<String>,
    pub workflows_dir: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PathsSection {
    pub python_bin: Option<String>,
    pub log_dir: Option<String>,
    pub web_dist_dir: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub python_bin: PathBuf,
    pub model_main_path: PathBuf,
    pub model_qwen_path: PathBuf,
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

fn expand_tilde(path_str: &str, home: &Path) -> PathBuf {
    if path_str == "~" {
        home.to_path_buf()
    } else if let Some(stripped) = path_str.strip_prefix("~/") {
        home.join(stripped)
    } else {
        PathBuf::from(path_str)
    }
}

impl Config {
    /// Load configuration with priority:
    /// 1. Environment variables (highest priority)
    /// 2. `config.toml` (from $MLX_CONFIG_PATH, ~/.mlx-server/config.toml, or ./config.toml)
    /// 3. Sensible defaults based on $HOME
    pub fn from_env() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let home_path = PathBuf::from(&home);

        // Find optional config.toml
        let config_candidates = vec![
            std::env::var("MLX_CONFIG_PATH").ok().map(PathBuf::from),
            Some(home_path.join(".mlx-server/config.toml")),
            Some(PathBuf::from("./config.toml")),
        ];

        let mut toml_cfg = TomlConfig::default();
        for candidate in config_candidates.into_iter().flatten() {
            if candidate.exists() {
                if let Ok(content) = std::fs::read_to_string(&candidate) {
                    if let Ok(parsed) = toml::from_str::<TomlConfig>(&content) {
                        info!("📄 Loaded configuration from {:?}", candidate);
                        toml_cfg = parsed;
                        break;
                    }
                }
            }
        }

        let srv = toml_cfg.server.unwrap_or_default();
        let mdl = toml_cfg.models.unwrap_or_default();
        let cmf = toml_cfg.comfyui.unwrap_or_default();
        let pts = toml_cfg.paths.unwrap_or_default();

        // 1. Python Binary
        let python_bin = std::env::var("PYTHON_BIN")
            .ok()
            .map(PathBuf::from)
            .or_else(|| pts.python_bin.map(|p| expand_tilde(&p, &home_path)))
            .unwrap_or_else(|| home_path.join(".mlx-server/bin/python"));

        // 2. Models
        let model_qwen_path = std::env::var("MODEL_QWEN_PATH")
            .ok()
            .map(PathBuf::from)
            .or_else(|| mdl.qwen_path.map(|p| expand_tilde(&p, &home_path)))
            .unwrap_or_else(|| home_path.join(".mlx-models/Qwen3.8-27B-4bit"));

        let model_main_path = std::env::var("MODEL_MAIN_PATH")
            .ok()
            .map(PathBuf::from)
            .or_else(|| mdl.main_path.map(|p| expand_tilde(&p, &home_path)))
            .unwrap_or_else(|| model_qwen_path.clone());

        let model_mtp_path = std::env::var("MODEL_MTP_PATH")
            .ok()
            .map(PathBuf::from)
            .or_else(|| mdl.mtp_path.map(|p| expand_tilde(&p, &home_path)))
            .unwrap_or_else(|| home_path.join(".mlx-models/Qwen3.8-27B-MTP-4bit"));

        let model_compact_path = std::env::var("MODEL_COMPACT_PATH")
            .ok()
            .map(PathBuf::from)
            .or_else(|| mdl.compact_path.map(|p| expand_tilde(&p, &home_path)))
            .unwrap_or_else(|| home_path.join(".mlx-models/gemma-4-26b-a4b-it-4bit"));

        // 3. Ports
        let port_main_internal = std::env::var("PORT_MAIN_INTERNAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(srv.port_main_internal)
            .unwrap_or(18234);

        let port_compact_internal = std::env::var("PORT_COMPACT_INTERNAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(srv.port_compact_internal)
            .unwrap_or(18235);

        let proxy_port_main = std::env::var("PROXY_PORT_MAIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(srv.proxy_port_main)
            .unwrap_or(1234);

        let proxy_port_compact = std::env::var("PROXY_PORT_COMPACT")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(srv.proxy_port_compact)
            .unwrap_or(1235);

        // 4. ComfyUI
        let comfy_python_bin = std::env::var("COMFY_PYTHON_BIN")
            .ok()
            .map(PathBuf::from)
            .or_else(|| cmf.python_bin.map(|p| expand_tilde(&p, &home_path)))
            .unwrap_or_else(|| home_path.join("ComfyUI-Installs/ComfyUI/ComfyUI/.venv/bin/python3"));

        let comfy_main_py = std::env::var("COMFY_MAIN_PY")
            .ok()
            .map(PathBuf::from)
            .or_else(|| cmf.main_py.map(|p| expand_tilde(&p, &home_path)))
            .unwrap_or_else(|| home_path.join("ComfyUI-Installs/ComfyUI/ComfyUI/main.py"));

        let comfy_port = std::env::var("COMFY_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .or(cmf.port)
            .unwrap_or(8188);

        let comfy_input_dir = std::env::var("COMFY_INPUT_DIR")
            .ok()
            .map(PathBuf::from)
            .or_else(|| cmf.input_dir.map(|p| expand_tilde(&p, &home_path)))
            .unwrap_or_else(|| home_path.join("ComfyUI-Shared/input"));

        let comfy_output_dir = std::env::var("COMFY_OUTPUT_DIR")
            .ok()
            .map(PathBuf::from)
            .or_else(|| cmf.output_dir.map(|p| expand_tilde(&p, &home_path)))
            .unwrap_or_else(|| home_path.join("ComfyUI-Shared/output"));

        let comfy_workflows_dir = std::env::var("COMFY_WORKFLOWS_DIR")
            .ok()
            .map(PathBuf::from)
            .or_else(|| cmf.workflows_dir.map(|p| expand_tilde(&p, &home_path)))
            .unwrap_or_else(|| home_path.join("ComfyUI/workflows"));

        // 5. System / Logs
        let log_dir = std::env::var("MLX_LOG_DIR")
            .ok()
            .map(PathBuf::from)
            .or_else(|| pts.log_dir.map(|p| expand_tilde(&p, &home_path)))
            .unwrap_or_else(|| home_path.join(".mlx-server"));

        let prefill_step_size = std::env::var("PREFILL_STEP_SIZE")
            .ok()
            .or(srv.prefill_step_size)
            .unwrap_or_else(|| "1024".to_string());

        let web_dist_dir = std::env::var("WEB_DIST_DIR")
            .ok()
            .map(PathBuf::from)
            .or_else(|| pts.web_dist_dir.map(|p| expand_tilde(&p, &home_path)))
            .unwrap_or_else(|| home_path.join("Documents/develop/mlx-dynamic-swapper/web/dist"));

        Self {
            python_bin,
            model_main_path,
            model_qwen_path,
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

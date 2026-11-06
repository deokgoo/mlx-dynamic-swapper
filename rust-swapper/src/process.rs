//! [Process Lifecycle & Metal VRAM Management Module]
//! 자식 프로세스(MLX 서버, ComfyUI)의 생성, 포트 헬스체크, 프로세스 그룹 단위 강제 종료(Metal VRAM 100% 반환)를 담당합니다.
//! 
//! 💡 [Python vs Rust 비교 학습]
//! - Python:
//!     proc = subprocess.Popen(cmd, preexec_fn=os.setsid)
//!     os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
//!   -> Python에서는 전역 변수(process_main)에 보관하며, 다중 스레드/비동기 동시 접근 시 경쟁 상태(Race Condition)가 발생하기 쉽습니다.
//! - Rust:
//!     Arc<tokio::sync::Mutex<Option<ChildProcess>>>를 통해 멀티스레드 안전성(Thread-safety)을 컴파일 타임에 강제합니다.
//!     `nix::sys::signal::killpg`를 사용해 POSIX 레벨에서 프로세스 그룹 전체를 확실히 정리합니다.

use std::fs::OpenOptions;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{info, warn};

pub struct ManagedProcess {
    pub child: Child,
    pub pgid: i32,
    pub port: u16,
}

#[derive(Clone)]
pub struct ProcessManager {
    main_proc: Arc<Mutex<Option<ManagedProcess>>>,
    compact_proc: Arc<Mutex<Option<ManagedProcess>>>,
    comfy_proc: Arc<Mutex<Option<ManagedProcess>>>,
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            main_proc: Arc::new(Mutex::new(None)),
            compact_proc: Arc::new(Mutex::new(None)),
            comfy_proc: Arc::new(Mutex::new(None)),
        }
    }

    /// 포트가 열릴 때까지 비동기로 대기합니다 (Python의 wait_for_port() 대응)
    pub async fn wait_for_port(&self, port: u16, timeout_secs: f32) -> bool {
        let start = tokio::time::Instant::now();
        let timeout = Duration::from_secs_f32(timeout_secs);
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

        while start.elapsed() < timeout {
            if TcpStream::connect(addr).await.is_ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// 포트가 완전히 닫힐 때까지 대기합니다 (Python의 wait_for_port_release() 대응)
    pub async fn wait_for_port_release(&self, port: u16, timeout_secs: f32) -> bool {
        let start = tokio::time::Instant::now();
        let timeout = Duration::from_secs_f32(timeout_secs);
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();

        while start.elapsed() < timeout {
            if TcpStream::connect(addr).await.is_err() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// 프로세스 그룹에 SIGTERM을 전송하고, 필요 시 SIGKILL로 확실하게 Metal 메모리를 반환합니다.
    pub async fn terminate_process(proc_opt: &mut Option<ManagedProcess>) {
        if let Some(mut proc) = proc_opt.take() {
            let pid = proc.child.id().unwrap_or(0) as i32;
            let pgid = proc.pgid;
            info!(target: "process", "🛑 Terminating process group pgid={} (port={})", pgid, proc.port);

            // 1. SIGTERM 전송
            let _ = signal::killpg(Pid::from_raw(pgid), Signal::SIGTERM);

            // 2. 최대 3초간 종료 대기
            let wait_future = proc.child.wait();
            match tokio::time::timeout(Duration::from_secs(3), wait_future).await {
                Ok(Ok(status)) => {
                    info!(target: "process", "✔ Process PID={} exited normally: {:?}", pid, status);
                }
                _ => {
                    // 타임아웃 시 SIGKILL 강제 종료
                    warn!(target: "process", "⚠️ Force killing process group pgid={}", pgid);
                    let _ = signal::killpg(Pid::from_raw(pgid), Signal::SIGKILL);
                    let _ = proc.child.wait().await;
                }
            }
        }
    }

    /// Main 모델(Qwen 27B + MTP) 기동
    pub async fn start_main(
        &self,
        python_bin: &Path,
        model_path: &Path,
        mtp_path: &Path,
        port: u16,
        prefill_step_size: &str,
        log_dir: &Path,
    ) -> Result<(), String> {
        let mut lock = self.main_proc.lock().await;
        if lock.is_some() {
            return Ok(()); // 이미 실행 중
        }

        self.wait_for_port_release(port, 5.0).await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        info!(target: "process", "🚀 Starting Main Model ({:?}) on port {}...", model_path.file_name().unwrap_or_default(), port);

        // 로그 디렉토리 및 파일 설정
        let _ = std::fs::create_dir_all(log_dir);
        let out_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("main.log"))
            .map_err(|e| e.to_string())?;
        let err_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("main.error.log"))
            .map_err(|e| e.to_string())?;

        let mut cmd = Command::new(python_bin);
        cmd.arg("-m")
            .arg("mlx_vlm.server")
            .arg("--model")
            .arg(model_path)
            .arg("--port")
            .arg(port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--prefill-step-size")
            .arg(prefill_step_size)
            .arg("--max-tokens")
            .arg("12288")
            .stdout(out_file)
            .stderr(err_file);

        if mtp_path.exists() {
            cmd.arg("--draft-model").arg(mtp_path);
        }

        // 새 프로세스 세션/그룹 생성 (Python의 preexec_fn=os.setsid)
        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                Ok(())
            });
        }

        let child = cmd.spawn().map_err(|e| format!("Failed to spawn main model: {}", e))?;
        let pid = child.id().unwrap_or(0) as i32;

        *lock = Some(ManagedProcess {
            child,
            pgid: pid, // setsid로 인해 pid가 pgid가 됩니다
            port,
        });

        drop(lock);

        if self.wait_for_port(port, 25.0).await {
            info!(target: "process", "✔ Main model is READY on port {}", port);
            Ok(())
        } else {
            Err(format!("Main model timed out waiting for port {}", port))
        }
    }

    /// Main 모델 중지
    pub async fn stop_main(&self) {
        let mut lock = self.main_proc.lock().await;
        Self::terminate_process(&mut *lock).await;
    }

    /// Compact 모델(Gemma 4 26B) 기동
    pub async fn start_compact(
        &self,
        python_bin: &Path,
        model_path: &Path,
        port: u16,
        prefill_step_size: &str,
        log_dir: &Path,
    ) -> Result<(), String> {
        let mut lock = self.compact_proc.lock().await;
        if lock.is_some() {
            return Ok(());
        }

        self.wait_for_port_release(port, 5.0).await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        info!(target: "process", "🗜️ Starting Compact Model ({:?}) on port {}...", model_path.file_name().unwrap_or_default(), port);

        let _ = std::fs::create_dir_all(log_dir);
        let out_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("compaction.log"))
            .map_err(|e| e.to_string())?;
        let err_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("compaction.error.log"))
            .map_err(|e| e.to_string())?;

        let mut cmd = Command::new(python_bin);
        cmd.arg("-m")
            .arg("mlx_lm.server")
            .arg("--model")
            .arg(model_path)
            .arg("--port")
            .arg(port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--prefill-step-size")
            .arg(prefill_step_size)
            .arg("--max-tokens")
            .arg("8192")
            .stdout(out_file)
            .stderr(err_file);

        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                Ok(())
            });
        }

        let child = cmd.spawn().map_err(|e| format!("Failed to spawn compact model: {}", e))?;
        let pid = child.id().unwrap_or(0) as i32;

        *lock = Some(ManagedProcess {
            child,
            pgid: pid,
            port,
        });

        drop(lock);

        if self.wait_for_port(port, 20.0).await {
            info!(target: "process", "✔ Compact model is READY on port {}", port);
            Ok(())
        } else {
            Err(format!("Compact model timed out waiting for port {}", port))
        }
    }

    /// Compact 모델 중지
    pub async fn stop_compact(&self) {
        let mut lock = self.compact_proc.lock().await;
        Self::terminate_process(&mut *lock).await;
    }

    /// ComfyUI 백엔드 기동
    pub async fn start_comfy(
        &self,
        python_bin: &Path,
        main_py: &Path,
        port: u16,
        input_dir: &Path,
        output_dir: &Path,
        log_dir: &Path,
    ) -> Result<(), String> {
        let mut lock = self.comfy_proc.lock().await;
        if lock.is_some() {
            return Ok(());
        }

        self.wait_for_port_release(port, 5.0).await;
        tokio::time::sleep(Duration::from_millis(300)).await;

        info!(target: "process", "🎨 Starting ComfyUI Backend on port {}...", port);

        let _ = std::fs::create_dir_all(log_dir);
        let out_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("comfyui.log"))
            .map_err(|e| e.to_string())?;
        let err_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("comfyui.error.log"))
            .map_err(|e| e.to_string())?;

        let mut cmd = Command::new(python_bin);
        cmd.arg(main_py)
            .arg("--listen")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--input-directory")
            .arg(input_dir)
            .arg("--output-directory")
            .arg(output_dir)
            .stdout(out_file)
            .stderr(err_file);

        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                Ok(())
            });
        }

        let child = cmd.spawn().map_err(|e| format!("Failed to spawn ComfyUI: {}", e))?;
        let pid = child.id().unwrap_or(0) as i32;

        *lock = Some(ManagedProcess {
            child,
            pgid: pid,
            port,
        });

        drop(lock);

        if self.wait_for_port(port, 40.0).await {
            info!(target: "process", "✔ ComfyUI backend is READY on port {}", port);
            Ok(())
        } else {
            Err(format!("ComfyUI backend timed out waiting for port {}", port))
        }
    }

    /// ComfyUI 백엔드 중지 (Metal VRAM 즉각 반환)
    pub async fn stop_comfy(&self) {
        let mut lock = self.comfy_proc.lock().await;
        Self::terminate_process(&mut *lock).await;
    }

    /// 상태 조회
    pub async fn is_main_running(&self) -> bool {
        self.main_proc.lock().await.is_some()
    }

    pub async fn is_compact_running(&self) -> bool {
        self.compact_proc.lock().await.is_some()
    }

    pub async fn is_comfy_running(&self) -> bool {
        self.comfy_proc.lock().await.is_some()
    }
}

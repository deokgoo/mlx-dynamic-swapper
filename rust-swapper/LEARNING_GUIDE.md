# 🦀 Python 개발자를 위한 Rust Swapper 1:1 비교 학습 가이드

이 문서는 기존의 Python 기반 `mlx_manager.py`가 Rust 네이티브 데몬(`rust-swapper`)으로 어떻게 재작성되었는지, 문법과 아키텍처 관점에서 1:1로 비교하며 공부할 수 있도록 작성된 가이드입니다.

---

## 📑 목차
1. [핵심 변경 사항 요약 (Architecture Diff)](#1-핵심-변경-사항-요약)
2. [개념별 1:1 코드 비교](#2-개념별-11-코드-비교)
   - [비동기 런타임 (asyncio vs tokio)](#-비동기-런타임-asyncio-vs-tokio)
   - [웹 프레임워크 (FastAPI vs Axum)](#-웹-프레임워크-fastapi-vs-axum)
   - [프로세스 관리 및 Metal VRAM 강제 회수 (os.killpg vs nix::killpg)](#-프로세스-관리-및-metal-vram-강제-회수)
   - [JSON 및 페이로드 조작 (Python dict vs serde_json::Value)](#-json-및-페이로드-조작)
   - [상태 공유 및 동시성 락 (Global variables vs Arc<Mutex<T>>)](#-상태-공유-및-동시성-락)
3. [Rust 빌드 및 실행 방법](#3-rust-빌드-및-실행-방법)

---

## 1. 핵심 변경 사항 요약

| 항목 | Python (`mlx_manager.py`) | Rust (`rust-swapper`) | 학습 포인트 |
| :--- | :--- | :--- | :--- |
| **상주 메모리(RAM)** | ~80MB ~ 100MB | **~8MB ~ 15MB** | 런타임/인터프리터 제거 |
| **비동기 엔진** | `asyncio` (단일 스레드 이벤트 루프) | `tokio` (고성능 작업 훔치기 멀티스레드 런타임) | 스케줄러 구조 차이 |
| **웹 프레임워크** | `FastAPI` + `Uvicorn` | `Axum` + `Tower` | 타입 기반 라우팅 |
| **HTTP 프록시 클라이언트** | `httpx.AsyncClient` | `reqwest::Client` (비동기 스트리밍) | Zero-copy I/O 중계 |
| **프로세스 시그널** | `os.killpg(pid, signal.SIGTERM)` | `nix::sys::signal::killpg` | POSIX 시스템 콜 바인딩 |
| **동시성 안전성** | 개발자의 주의에 의존 (경쟁 상태 위험) | **컴파일러가 강제 (`Send + Sync`, `Mutex`)** | 소유권 & 스레드 안전성 |

---

## 2. 개념별 1:1 코드 비교

### 🔹 비동기 런타임 (`asyncio` vs `tokio`)

* **Python:**
  ```python
  import asyncio

  async def main():
      server = uvicorn.Server(config)
      await server.serve()

  if __name__ == "__main__":
      asyncio.run(main())
  ```
* **Rust:**
  ```rust
  #[tokio::main] // 매크로가 main 함수를 tokio 멀티스레드 런타임 안으로 감싸줍니다.
  async fn main() -> Result<(), Box<dyn std::error::Error>> {
      axum::serve(listener, router).await?;
      Ok(())
  }
  ```
  > 💡 **학습 포인트**: Rust의 `async` 함수는 호출한다고 바로 실행되지 않고 `Future`를 반환합니다. 반드시 `.await`를 붙여야 실제로 실행됩니다.

---

### 🔹 웹 프레임워크 (`FastAPI` vs `Axum`)

* **Python (FastAPI):**
  ```python
  app = FastAPI()

  @app.get("/status")
  async def get_status():
      return {"status": "online"}
  ```
* **Rust (Axum):**
  ```rust
  let app = Router::new()
      .route("/status", get(get_status));

  async fn get_status() -> impl IntoResponse {
      Json(json!({"status": "online"}))
  }
  ```
  > 💡 **학습 포인트**: Axum에서는 데코레이터(`@app.get`) 대신 라우터 체이닝(`.route("/path", get(handler))`)을 사용합니다. 반환 타입은 `impl IntoResponse`로 선언하여 JSON, HTML, 바이트 스트림 등 무엇이든 유연하게 반환할 수 있습니다.

---

### 🔹 프로세스 관리 및 Metal VRAM 강제 회수

스와퍼의 가장 중요한 기능: **프로세스 그룹 전체를 확실히 종료시켜 Metal 메모리를 반환**하는 것입니다.

* **Python:**
  ```python
  # 기동 시 새 프로세스 그룹 생성
  proc = subprocess.Popen(cmd, preexec_fn=os.setsid)

  # 종료 시 SIGTERM 후 SIGKILL
  pgid = os.getpgid(proc.pid)
  os.killpg(pgid, signal.SIGTERM)
  ```
* **Rust:**
  ```rust
  // 기동 시 setsid
  let mut cmd = Command::new(python_bin);
  unsafe {
      cmd.pre_exec(|| {
          nix::unistd::setsid().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
          Ok(())
      });
  }
  let child = cmd.spawn()?;

  // 종료 시 nix 크레이트로 확실하게 시그널 전송
  let _ = nix::sys::signal::killpg(Pid::from_raw(pgid), Signal::SIGTERM);
  ```
  > 💡 **학습 포인트**: Rust에서는 OS 시스템 콜이나 저수준 C API를 직접 호출할 때 `unsafe` 블록을 명시적으로 요구합니다. 이를 통해 시스템 프로그래밍 레벨의 권한을 안전하게 격리합니다.

---

### 🔹 JSON 및 페이로드 조작 (Hermes thinking 제어)

Hermes의 `reasoning_effort`나 컨텍스트 압축 프롬프트를 주입하는 로직:

* **Python:**
  ```python
  if "messages" in data:
      data["messages"][-1]["content"] += "\n[COMPACT CONSTRAINTS]"
  ```
* **Rust:**
  ```rust
  if let Some(messages) = data.get_mut("messages").and_then(|m| m.as_array_mut()) {
      if let Some(last_msg) = messages.last_mut().and_then(|m| m.as_object_mut()) {
          if let Some(content) = last_msg.get_mut("content") {
              if let Some(s) = content.as_str() {
                  let new_content = format!("{}\n[COMPACT CONSTRAINTS]", s);
                  *content = json!(new_content);
              }
          }
      }
  }
  ```
  > 💡 **학습 포인트**: 파이썬은 런타임에 딕셔너리를 그냥 바꾸지만 키가 없으면 `KeyError`나 `TypeError`가 납니다. Rust는 `if let Some(...)` 패턴 매칭과 `Option` 타입을 통해 "값이 없을 수도 있는 상황"을 컴파일 타임에 안전하게 처리합니다.

---

### 🔹 상태 공유 및 동시성 락 (`Arc<Mutex<T>>`)

* **Python:**
  ```python
  # 전역 변수
  swap_lock = asyncio.Lock()
  last_active_model = "main"

  async def swap():
      global last_active_model
      async with swap_lock:
          last_active_model = "compact"
  ```
* **Rust:**
  ```rust
  #[derive(Clone)]
  pub struct AppState {
      pub swap_lock: Arc<Mutex<()>>,
      pub last_active_model: Arc<Mutex<String>>,
  }

  async fn swap(state: AppState) {
      let _lock = state.swap_lock.lock().await;
      let mut active = state.last_active_model.lock().await;
      *active = "compact".to_string();
  }
  ```
  > 💡 **학습 포인트**:
  > - `Arc` (Atomic Reference Counting): 여러 비동기 태스크에서 동일한 데이터를 안전하게 공유(참조 카운팅)하기 위한 스마트 포인터입니다.
  > - `Mutex`: 여러 스레드나 비동기 태스크가 동시에 쓰기 작업을 하지 못하도록 상호 배제(Lock)를 겁니다.

---

## 3. Rust 빌드 및 실행 방법

### 1) Rust 툴체인 설치 (아직 설치되지 않은 경우)
```bash
# 공식 권장 설치 (rustup)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# 또는 Homebrew 설치
brew install rust
```

### 2) 빌드 및 검사
```bash
cd /Users/deokgoo/Documents/develop/mlx-dynamic-swapper/rust-swapper

# 1. 문법 및 타입 체크 (매우 빠름)
cargo check

# 2. 디버그 빌드 및 실행
cargo run

# 3. 최적화된 릴리즈 바이너리 컴파일 (RAM 점유 극소화)
cargo build --release
# 생성 위치: target/release/mlx-swapper
```

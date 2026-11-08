# MLX Dynamic Model Swapper - 작업 인수인계 및 진행 상태 (Handover)

- **작성 일시:** 2026-11-08
- **브랜치:** `feat/pipeline-and-cleanup`
- **프로젝트 경로:** `/Users/deokgoo/Documents/develop/mlx-dynamic-swapper/`

---

## 1. 아키텍처 개선 핵심 (EAFP / Retry on Failure)
- 복잡한 OS 레벨의 좀비 프로세스 감시(`reap_zombies`, `reap_slot`), 슬롯 검사, 사전 포트 탐침 루프(`wait_for_port`) 등 **과잉 엔지니어링을 전부 걷어냄**.
- **"EAFP(Easier to ask for forgiveness than permission)"** 패턴 적용:
  - 프록시 요청 시 사전 검사 없이 백엔드로 즉시 전송 (정상 시 레이턴시 0ms 추가 지연 없음).
  - 백엔드가 크래시/OOM 등으로 다운되어 연결 실패(`502 Bad Gateway / Connection Refused`)가 발생하면, 즉시 감지하여 모델 프로세스를 재시작(`restart_main` / `restart_compact`)하고 1회 재시도(Retry).

---

## 2. 반영된 구현 상세

### A. `rust-swapper/src/process.rs`
- 불필요한 `reap_zombies`, `reap_slot`, `liveness_check` 완전 제거 (단순한 슬롯 유무 확인으로 원복).
- `restart_main(...)`: 기존 프로세스를 정리(`stop_main`)하고 새로 기동(`start_main`).
- `restart_compact(...)`: 기존 프로세스를 정리(`stop_compact`)하고 새로 기동(`start_compact`).

### B. `rust-swapper/src/main.rs`
- `handle_main_proxy`:
  - 사전 포트 탐침 루프 제거.
  - 1차 요청 전송 후 `StatusCode::BAD_GATEWAY` 발생 시 `restart_main` 후 1회 재시도.
- `handle_compact_proxy`:
  - 사전 포트 탐침 루프 제거.
  - 1차 요청 전송 후 `StatusCode::BAD_GATEWAY` 발생 시 `restart_compact` 후 1회 재시도.
- `system_stats_handler`, `status_check`:
  - 불필요한 self-heal 호출을 걷어내고 순수 상태 조회 엔드포인트로 복원.

---

## 3. 검증 결과
- `cargo check`: 컴파일 성공 (경고 0개, 에러 0개)
- `cargo test`: 전체 12개 단위/통합 테스트 통과 (12 passed; 0 failed)

# ADR-0001: AntiLoopGuard 추론 모델 교착(Deadlock) 해소 및 복구 사다리 정밀화

## Status
- **Date**: 2026-11-14
- **Status**: Accepted & Applied to Production

## Context
1. **문제 현상 (2026-11-13 23:15 ~ 23:21)**:
   - 텔레그램 세션에서 Hermes가 7회 연속 빈 응답(Empty Response) 및 Reasoning 전용 출력을 내며 최종적으로 `empty_response_exhausted` 에러에 도달함.
   - Hermes의 비상 폴백 로직이 동작하여 본문 대신 `[모델이 본문 응답을 생성하지 않아 추론 내용을 대신 전달합니다]` 형태로 내부 추론 내용이 전송됨.
2. **원인 규명**:
   - 도구 실행 중 `FileNotFoundError` 발생 후, 모델이 동일 스크립트를 2회 연속 호출(Call #3, Call #4).
   - 하부 프록시인 `mlx-dynamic-swapper`의 `AntiLoopGuardFilter`가 이를 무한 루프로 감지하여 `tool_choice: "none"`을 강제 설정함.
   - 그러나 Qwen3.8 추론(Thinking) 모델은 내부 사고 끝에 "도구를 호출해야겠다"로 결론지었으나 도구가 물리적으로 압수되어 본문 작성으로 넘어가지 못하고 즉시 종료 토큰(`finish_reason=stop`, 210 토큰)을 출력.
   - Hermes가 이를 빈 응답으로 인지하고 복구 사다리(Nudge: *"추론이 완료되었습니다. 위 추론 내용을 바탕으로 사용자에게 전달할 최종 답변 본문을 명확하게 작성해주세요."*)를 보냈으나, 대화 히스토리의 과거 도구 호출 때문에 `AntiLoopGuard`가 모든 재시도 턴에서 계속 `tool_choice: "none"`을 고정하는 교착 상태(Deadlock) 발생.
   - 동시에 `ThinkingNormalizer`가 복구 턴에서도 다시 `enable_thinking: true`를 강제하여 모델이 헛돌게 됨.

## Decisions & Changes

### 1. `mlx-dynamic-swapper` 파이프라인 정밀화 (`rust-swapper`)
- **`AntiLoopGuardFilter` 루프 평가 범위 한정**:
  - 중복 도구 호출 감지 및 `tool_choice="none"` 강제 적용 조건을 **`last_role == "tool"` (도구 실행 직후 LLM이 다음 행동을 결정하는 턴)**으로 엄격히 한정.
  - 히스토리 역순 탐색 시 `role == "user"`를 만나면 즉시 중단(`break`)하여, 이전 턴의 도구 호출이 현재 사용자 턴/복구 턴에 간섭하지 않도록 격리.
  - 사용자 복구 넛지 턴(`추론이 완료되었습니다` 등)에서는 중복 도구 차단을 면제.
- **`AntiLoopGuardFilter` 추론 모델 보호 가이드 강화**:
  - 도구 차단 시 주입되는 긴급 가이드에 추론 모델 전용 지침 추가: `<think>` 내부 사고를 즉시 종료하고 `<think>` 블록 외부에 사용자용 완성된 본문 텍스트를 필수 출력하도록 명시.
- **`ThinkingNormalizerFilter` 복구 넛지 자동 감지 및 Thinking OFF**:
  - 마지막 메시지가 Hermes의 복구 넛지(`추론이 완료되었습니다` 등)인 경우, 이미 이전 호출에서 추론이 끝난 상태이므로 `enable_thinking: false`를 강제 설정.
  - 모델이 불필요한 2차 사고 루프에 빠지지 않고 컨텍스트의 추론 내용을 바탕으로 즉시 사용자 답변 본문을 합성하도록 유도.

### 2. Hermes 에이전트 지침 보강 (`~/.hermes/SOUL.md`)
- **실행 규칙**: 도구 실행 중 `FileNotFoundError`, `Traceback` 등 오류 발생 시 동일한 인자/스크립트로 재호출하는 행위를 엄격히 금지하고, 오류 메시지를 분석해 경로 교정 또는 대안 도구 사용을 명시.
- **사고 규칙**: 사고(thinking) 종료 후 반드시 사용자에게 보여줄 최종 답변 본문을 명확하게 작성해야 하며, 사고 블록만 남기고 종료(EOS)하는 행위 엄격 금지.

## Verification
- `rust-swapper` 단위 테스트 전체 15건 통과 (중복 도구 감지 격리 및 복구 넛지 Thinking OFF 검증).
- Release 바이너리 빌드 및 `~/.mlx-server/bin/mlx-swapper` 배포.
- LaunchAgent 서비스 재기동 및 상태 정상 동작 확인.

use super::{PayloadFilter, PipelineContext};
use serde_json::{json, Value};

/// 1단계: 모델 명칭을 타겟 백엔드 로컬 모델 경로로 강제 일치화
pub struct ModelRouteFilter;

impl PayloadFilter for ModelRouteFilter {
    fn name(&self) -> &'static str {
        "ModelRoute"
    }

    fn apply(&self, ctx: &mut PipelineContext, payload: &mut Value) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("model".to_string(), json!(ctx.target_model));
            ctx.record_modified(self.name());
        }
        Ok(())
    }
}

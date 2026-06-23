use super::*;

impl Engine {
    /// Select a local file in the native platform file chooser. When a trigger
    /// is provided, this asks the platform backend to real-click inside the
    /// target window first because browser `input[type=file]` controls often
    /// reject AX/background clicks.
    pub fn select_file(
        &mut self,
        path: &str,
        trigger: Option<FileSelectTrigger>,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        let file = canonical_file_path(path)?;
        let trigger_point = self.file_select_trigger_point(trigger.as_ref())?;
        let target_id = format!("file@{}", file.display());
        let risk = RiskAssessment {
            level: RiskLevel::High,
            requires_approval: true,
            reasons: vec![
                "selects a local file for upload".to_string(),
                "drives a native file chooser through the platform backend".to_string(),
            ],
        };
        if let Some(entry) = self.gate_raw_input(
            &target_id,
            SemanticAction::Type,
            Some(file.display().to_string()),
            reasoning.or(Some("select local file for upload")),
            risk.clone(),
        ) {
            return Ok(entry);
        }
        let outcome = retry_user_active_guard(|| {
            dunst_platform::select_file(&file, trigger_point, self.target.pid)
        });
        self.audit_raw_input(
            target_id,
            SemanticAction::Type,
            Some(file.display().to_string()),
            reasoning.or(Some("select local file for upload")),
            risk,
            outcome,
        )
    }

    pub(super) fn file_select_trigger_point(
        &self,
        trigger: Option<&FileSelectTrigger>,
    ) -> dunst_core::Result<Option<(f64, f64)>> {
        match trigger {
            None => Ok(None),
            Some(FileSelectTrigger::Point { x, y }) => {
                self.ensure_point_in_target_window(*x, *y, "select_file trigger")?;
                Ok(Some((*x, *y)))
            }
            Some(FileSelectTrigger::ElementId(id)) => {
                let node = self
                    .scene_graph()
                    .get(id)
                    .ok_or_else(|| DunstError::ElementNotFound(id.clone()))?;
                let bbox = node.bbox.ok_or_else(|| {
                    DunstError::Execution(format!("element {id:?} has no screen bbox"))
                })?;
                if bbox.w <= 0.0 || bbox.h <= 0.0 {
                    return Err(DunstError::Execution(format!(
                        "element {id:?} has an empty screen bbox"
                    )));
                }
                let point = (bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0);
                self.ensure_point_in_target_window(point.0, point.1, "select_file trigger")?;
                Ok(Some(point))
            }
        }
    }
}

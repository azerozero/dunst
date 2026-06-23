use super::*;

impl Engine {
    pub(super) fn resolve_option_candidate(
        &self,
        query: &str,
        visible_only: bool,
    ) -> dunst_core::Result<OptionCandidate> {
        let matches: Vec<String> = self
            .find_element_filtered(query, visible_only)
            .into_iter()
            .map(|n| n.id.clone())
            .collect();

        for matched_id in matches {
            if let Ok((action_id, action)) = self.resolve_action_target(
                &matched_id,
                &[
                    SemanticAction::Pick,
                    SemanticAction::Click,
                    SemanticAction::Toggle,
                ],
            ) {
                return Ok(OptionCandidate {
                    matched_id,
                    action_id,
                    action,
                });
            }
        }

        Err(DunstError::Execution(format!(
            "no clickable option found for query {query:?}"
        )))
    }

    pub(super) fn resolve_action_target_refreshing_missing(
        &mut self,
        id: &str,
        preferred: &[SemanticAction],
    ) -> dunst_core::Result<(String, SemanticAction)> {
        match self.resolve_action_target(id, preferred) {
            Err(err) if is_element_not_found(&err) => {
                self.refresh()?;
                self.resolve_action_target(id, preferred)
            }
            other => other,
        }
    }

    pub(super) fn resolve_action_target(
        &self,
        id: &str,
        preferred: &[SemanticAction],
    ) -> dunst_core::Result<(String, SemanticAction)> {
        self.scene_graph()
            .get(id)
            .ok_or_else(|| DunstError::ElementNotFound(id.into()))?;

        let mut actions = Vec::new();
        for action in preferred {
            push_unique_action(&mut actions, *action);
            if *action == SemanticAction::Click {
                push_unique_action(&mut actions, SemanticAction::Pick);
                push_unique_action(&mut actions, SemanticAction::Toggle);
                push_unique_action(&mut actions, SemanticAction::OpenMenu);
            }
        }

        if let Some(action) = self.first_supported_action(id, &actions) {
            return Ok((id.to_string(), action));
        }

        let requested_risk = self
            .affordance_graph()
            .affordances
            .get(id)
            .map(|a| a.risk.clone())
            .unwrap_or_else(RiskAssessment::low);
        if requested_risk.requires_approval {
            return Err(DunstError::ActionUnavailable {
                id: id.into(),
                action: preferred
                    .first()
                    .map(|action| format!("{action:?}"))
                    .unwrap_or_else(|| "action".to_string()),
            });
        }

        let mut current = self.scene_graph().get(id).and_then(|n| n.parent.as_deref());
        while let Some(parent_id) = current {
            if let Some(action) = self.first_supported_action(parent_id, &actions) {
                return Ok((parent_id.to_string(), action));
            }
            current = self
                .scene_graph()
                .get(parent_id)
                .and_then(|n| n.parent.as_deref());
        }

        Err(DunstError::ActionUnavailable {
            id: id.into(),
            action: preferred
                .first()
                .map(|action| format!("{action:?}"))
                .unwrap_or_else(|| "action".to_string()),
        })
    }

    pub(super) fn first_supported_action(
        &self,
        id: &str,
        actions: &[SemanticAction],
    ) -> Option<SemanticAction> {
        let affordance = self.affordance_graph().affordances.get(id)?;
        actions
            .iter()
            .copied()
            .find(|action| affordance.actions.contains(action))
    }

    pub(super) fn option_selected(&self, action_id: &str, matched_id: &str) -> Option<bool> {
        self.scene_graph()
            .get(action_id)
            .and_then(option_selected_state)
            .or_else(|| {
                self.scene_graph()
                    .get(matched_id)
                    .and_then(option_selected_state)
            })
    }

    pub(super) fn push_entry(&mut self, mut entry: AuditEntry) -> AuditEntry {
        if entry.caller.is_none() {
            entry.caller.clone_from(&self.session_identity);
        }
        self.trace.push(entry.clone());
        entry
    }

    // --- audit --------------------------------------------------------------

    /// Public accessor over the audit trail; exercised by the gating tests and
    /// part of the engine API the MCP layer may surface.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "public audit-trail accessor, exercised only by tests"
        )
    )]
    pub fn trace(&self) -> &[AuditEntry] {
        &self.trace
    }

    pub fn export_trace(&self) -> dunst_core::Result<String> {
        Ok(serde_json::to_string_pretty(&self.trace)?)
    }
}

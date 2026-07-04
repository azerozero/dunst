use super::*;

impl Engine {
    // --- action tools -------------------------------------------------------

    /// Clicks the element resolved from `id`.
    ///
    /// # Errors
    ///
    /// Returns an error if `id` does not resolve to a live element, if no
    /// clickable affordance is available on it or an ancestor, if the risk gate
    /// requires operator approval, or if the click actuation fails.
    pub fn click_element(
        &mut self,
        id: &str,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        let (target_id, action) =
            self.resolve_action_target_refreshing_missing(id, &[SemanticAction::Click])?;
        self.act_refreshing_missing(&target_id, action, None, reasoning, None)
    }

    /// Raises the element identified by `id` to the front.
    ///
    /// # Errors
    ///
    /// Returns an error if `id` does not resolve to a live element, if it
    /// exposes no raise affordance, if the risk gate requires operator approval,
    /// or if the raise actuation fails.
    pub fn raise_element(
        &mut self,
        id: &str,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        self.act_refreshing_missing(id, SemanticAction::Raise, None, reasoning, None)
    }

    /// Picks the option matching `query` and reports the resulting selection.
    ///
    /// # Errors
    ///
    /// Returns an error if no clickable option matches `query`, if the risk gate
    /// requires operator approval, or if the pick actuation fails.
    pub fn pick_option(
        &mut self,
        query: &str,
        visible_only: bool,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<OptionPickResult> {
        let candidate = self.resolve_option_candidate(query, visible_only)?;
        let selected_before = self.option_selected(&candidate.action_id, &candidate.matched_id);
        let action_role = self
            .scene_graph()
            .get(&candidate.action_id)
            .map(|n| n.role.as_str())
            .unwrap_or("unknown");
        let audit = self.act(
            &candidate.action_id,
            candidate.action,
            None,
            reasoning.or(Some("pick option")),
            None,
        )?;
        let (selected_after, closed_after) = if audit.result == ActionResult::Success {
            let after = self.option_selected(&candidate.action_id, &candidate.matched_id);
            let still_visible = self
                .find_element_filtered(query, true)
                .into_iter()
                .any(|n| n.id == candidate.action_id || n.id == candidate.matched_id);
            (after, Some(!still_visible))
        } else {
            (selected_before, None)
        };
        Ok(OptionPickResult {
            query: query.to_string(),
            matched_id: candidate.matched_id,
            action_id: candidate.action_id,
            action_role,
            action: candidate.action,
            selected_before,
            selected_after,
            closed_after,
            audit,
        })
    }

    /// Types `text` into the element identified by `id`.
    ///
    /// # Errors
    ///
    /// Returns an error if `text` exceeds 100,000 bytes, if `id` does not
    /// resolve to a live element, if it accepts no typing, if the risk gate
    /// requires operator approval, or if the keystroke actuation fails.
    pub fn type_into(
        &mut self,
        id: &str,
        text: &str,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        // Guard the synchronous keystroke path against a multi-MB payload (audit C9).
        const MAX_TYPE_LEN: usize = 100_000;
        if text.len() > MAX_TYPE_LEN {
            return Err(dunst_core::DunstError::Execution(format!(
                "type text too long: {} bytes (max {MAX_TYPE_LEN})",
                text.len()
            )));
        }
        self.act_refreshing_missing(id, SemanticAction::Type, Some(text), reasoning, None)
    }

    /// Hovers over the element identified by `id`.
    ///
    /// # Errors
    ///
    /// Returns an error if `id` does not resolve to a live element, if it
    /// exposes no hover affordance, if the risk gate requires operator approval,
    /// or if the hover actuation fails.
    pub fn hover_probe(&mut self, id: &str) -> dunst_core::Result<AuditEntry> {
        self.act_refreshing_missing(id, SemanticAction::Hover, None, Some("hover probe"), None)
    }

    /// Drag `source_id` onto `target_id`. The drop point handed to the executor
    /// is the **target** node's bbox centre in screen coordinates, formatted as
    /// `"x,y"` (the frozen WP-F drag mini-contract). This is a thin wrapper over
    /// the gated action path — `act` checks the *source* exposes `Drag`, gates
    /// on risk, runs the executor, re-perceives, diffs and audits.
    ///
    /// Audit #3 — **composite risk**: a drop is as dangerous as the riskier of its
    /// source and its target (dropping a file onto "Supprimer" is a delete, even
    /// though the file row is harmless). The drop target's risk is folded in here
    /// and `act` gates on the max, so a high-risk target forces approval even when
    /// the source is low-risk.
    ///
    /// # Errors
    ///
    /// Returns an error if `target_id` does not resolve to a live element, if
    /// the target has no bbox to derive a drop point, if `source_id` does not
    /// resolve or exposes no drag affordance, if the composite source/target
    /// risk gate requires operator approval, or if the drag actuation fails.
    pub fn drag_element(
        &mut self,
        source_id: &str,
        target_id: &str,
        reasoning: Option<&str>,
    ) -> dunst_core::Result<AuditEntry> {
        let target = self
            .scene_graph()
            .get(target_id)
            .ok_or_else(|| DunstError::ElementNotFound(target_id.into()))?;
        let bbox = target.bbox.ok_or_else(|| {
            DunstError::Execution(format!(
                "target {target_id} has no bbox; a drop needs a concrete point"
            ))
        })?;
        let x = bbox.x + bbox.w / 2.0;
        let y = bbox.y + bbox.h / 2.0;
        // Fold the drop target's risk into the gate (audit #3). Every node has an
        // affordance entry; default to low if one is somehow missing.
        let target_risk = self
            .affordance_graph()
            .affordances
            .get(target_id)
            .map(|a| a.risk.clone())
            .unwrap_or_else(RiskAssessment::low);
        let co_target = CoTarget {
            id: target_id.to_string(),
            risk: target_risk,
        };
        self.act_refreshing_missing(
            source_id,
            SemanticAction::Drag,
            Some(&format!("{x},{y}")),
            reasoning,
            Some(co_target),
        )
    }
}

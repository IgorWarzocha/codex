//! Keeps configured token-budget preferences separate from session startup activation.
//! Fresh child sessions restore this snapshot before validating their context strategy;
//! history forks retain configured preferences but revalidate remote storage availability.

use super::Config;
use super::ConstraintResult;
use super::TokenBudgetConfig;
use codex_features::Feature;

/// Token-budget preferences before a session validates strategy-owned activation.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenBudgetStartupConfig {
    enabled: bool,
    token_budget: Option<TokenBudgetConfig>,
}

impl Config {
    /// Captures configured preferences, removing any inherited startup activation first.
    pub(crate) fn prepare_token_budget_for_startup(&mut self) -> ConstraintResult<()> {
        if let Some(snapshot) = self.token_budget_startup_config.as_ref() {
            self.features
                .set_enabled(Feature::TokenBudget, snapshot.enabled)?;
            self.token_budget = snapshot.token_budget.clone();
        }
        self.token_budget_startup_config = Some(TokenBudgetStartupConfig {
            enabled: self.features.enabled(Feature::TokenBudget),
            token_budget: self.token_budget.clone(),
        });
        Ok(())
    }
}

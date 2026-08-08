use serde::Deserialize;
use serde::Serialize;

use crate::WorkflowModelRoute;
use crate::WorkflowModelRoutingConstraints;
use crate::WorkflowModelRoutingDecisionStatus;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum WorkflowProviderCreditControl {
    NotRequested,
    #[default]
    Unavailable,
    Reserved {
        reservation_id: String,
        ceiling_usd: String,
        spent_usd: String,
        remaining_usd: String,
        exhausted: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowEffectiveModelRoute {
    pub model_gateway: String,
    pub provider: String,
    pub model: String,
    pub reasoning: String,
    pub service_tier: Option<String>,
    pub auth_profile: Option<String>,
    pub approval_policy: Option<String>,
    pub permission_profile: Option<String>,
    pub worktree_mode: String,
    pub context_ceiling_tokens: Option<u64>,
    pub fallback_used: bool,
    pub credit_control: WorkflowProviderCreditControl,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRouteRuntime {
    pub model_gateway: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub service_tier: Option<String>,
    pub auth_profile: Option<String>,
    pub approval_policy: Option<String>,
    pub permission_profile: Option<String>,
    pub context_window_tokens: Option<u64>,
    pub credit_control: WorkflowProviderCreditControl,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum WorkflowProviderCreditTerminalAccounting {
    NotRequested,
    ProviderReadback {
        reservation_id: String,
        ceiling_usd: String,
        spent_usd: String,
        remaining_usd: String,
        exhausted: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRouteReceipt {
    pub requested: WorkflowModelRoute,
    pub effective: WorkflowEffectiveModelRoute,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct WorkflowRouteEnforcementError {
    code: &'static str,
    message: String,
}

impl WorkflowRouteEnforcementError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

pub fn admit_workflow_model_route(
    requested: &WorkflowModelRoute,
    effective: &WorkflowEffectiveModelRoute,
) -> Result<WorkflowRouteReceipt, WorkflowRouteEnforcementError> {
    enforce_exact(
        "workflow_route_gateway_mismatch",
        "model gateway",
        requested.model_gateway.as_str(),
        effective.model_gateway.as_str(),
    )?;
    enforce_exact(
        "workflow_route_provider_mismatch",
        "provider",
        requested.provider.as_str(),
        effective.provider.as_str(),
    )?;
    enforce_exact(
        "workflow_route_model_mismatch",
        "model",
        requested.model.as_str(),
        effective.model.as_str(),
    )?;
    enforce_exact(
        "workflow_route_reasoning_mismatch",
        "reasoning",
        requested.reasoning.as_str(),
        effective.reasoning.as_str(),
    )?;
    enforce_optional_exact(
        "workflow_route_service_tier_mismatch",
        "service tier",
        requested.service_tier.as_deref(),
        effective.service_tier.as_deref(),
    )?;
    enforce_optional_exact(
        "workflow_route_approval_profile_mismatch",
        "approval profile",
        requested.approval_policy.as_deref(),
        effective.approval_policy.as_deref(),
    )?;
    enforce_optional_exact(
        "workflow_route_permission_profile_mismatch",
        "permission profile",
        requested.permission_profile.as_deref(),
        effective.permission_profile.as_deref(),
    )?;

    if let Some(routing) = requested.routing.as_ref() {
        let Some(decision) = routing.decision.as_ref() else {
            return Err(route_error(
                "workflow_route_decision_missing",
                "routing contract has no immutable decision",
            ));
        };
        if decision.status == WorkflowModelRoutingDecisionStatus::Error {
            return Err(route_error(
                "workflow_route_decision_error",
                "routing decision is an error and cannot be executed",
            ));
        }
        enforce_optional_exact(
            "workflow_route_gateway_mismatch",
            "routing decision model gateway",
            decision.model_gateway.as_deref(),
            Some(effective.model_gateway.as_str()),
        )?;
        enforce_optional_exact(
            "workflow_route_provider_mismatch",
            "routing decision provider",
            decision.provider.as_deref(),
            Some(effective.provider.as_str()),
        )?;
        enforce_optional_exact(
            "workflow_route_model_mismatch",
            "routing decision model",
            decision.model.as_deref(),
            Some(effective.model.as_str()),
        )?;
        enforce_optional_exact(
            "workflow_route_reasoning_mismatch",
            "routing decision reasoning",
            decision.reasoning.as_deref(),
            Some(effective.reasoning.as_str()),
        )?;
        enforce_optional_exact(
            "workflow_route_service_tier_mismatch",
            "routing decision service tier",
            decision.service_tier.as_deref(),
            effective.service_tier.as_deref(),
        )?;
        enforce_optional_exact(
            "workflow_route_auth_profile_mismatch",
            "routing decision auth profile",
            decision.auth_profile.as_deref(),
            effective.auth_profile.as_deref(),
        )?;
        let context = &routing.request.context;
        enforce_optional_exact(
            "workflow_route_auth_profile_mismatch",
            "routing context auth profile",
            context.auth_profile.as_deref(),
            effective.auth_profile.as_deref(),
        )?;
        enforce_optional_exact(
            "workflow_route_approval_profile_mismatch",
            "routing context approval profile",
            context.approval_policy.as_deref(),
            effective.approval_policy.as_deref(),
        )?;
        enforce_optional_exact(
            "workflow_route_permission_profile_mismatch",
            "routing context permission profile",
            context.permission_profile.as_deref(),
            effective.permission_profile.as_deref(),
        )?;
        enforce_optional_exact(
            "workflow_route_worktree_mode_mismatch",
            "routing context worktree mode",
            context.worktree_mode.as_deref(),
            Some(effective.worktree_mode.as_str()),
        )?;

        let constraints = &routing.request.constraints;
        enforce_constraints(constraints, decision.status, effective)?;
        let decision_fallback_used = decision
            .fallback
            .as_ref()
            .is_some_and(|fallback| fallback.used);
        if decision_fallback_used != effective.fallback_used {
            return Err(route_error(
                "workflow_route_fallback_mismatch",
                "effective fallback decision does not match the persisted routing decision",
            ));
        }
        if constraints.fallback_required && !effective.fallback_used {
            return Err(route_error(
                "workflow_route_required_fallback_missing",
                "routing requires an explicit fallback decision",
            ));
        }
        enforce_credit_control(constraints.budget_usd.as_deref(), &effective.credit_control)?;
    } else if !matches!(
        effective.credit_control,
        WorkflowProviderCreditControl::NotRequested
    ) {
        return Err(route_error(
            "workflow_route_credit_control_unexpected",
            "credit control was supplied without a routing budget contract",
        ));
    }

    Ok(WorkflowRouteReceipt {
        requested: requested.clone(),
        effective: effective.clone(),
    })
}

pub fn admit_workflow_model_route_for_runtime(
    requested: &WorkflowModelRoute,
    runtime: &WorkflowRouteRuntime,
    worktree_mode: &str,
) -> Result<WorkflowRouteReceipt, WorkflowRouteEnforcementError> {
    let constraints = requested
        .routing
        .as_ref()
        .map(|routing| &routing.request.constraints);
    let context_ceiling_tokens = constraints
        .and_then(|constraints| constraints.max_context_tokens)
        .map(|ceiling| {
            let available = runtime.context_window_tokens.ok_or_else(|| {
                route_error(
                    "workflow_route_context_ceiling_unavailable",
                    "runtime cannot prove the requested context ceiling before launch",
                )
            })?;
            if available < ceiling {
                return Err(route_error(
                    "workflow_route_context_ceiling_unavailable",
                    "runtime context window is smaller than the requested immutable ceiling",
                ));
            }
            Ok(ceiling)
        })
        .transpose()?;
    let fallback_used = requested
        .routing
        .as_ref()
        .and_then(|routing| routing.decision.as_ref())
        .and_then(|decision| decision.fallback.as_ref())
        .is_some_and(|fallback| fallback.used);
    let finite_budget_requested = constraints
        .and_then(|constraints| constraints.budget_usd.as_ref())
        .is_some();
    let effective = WorkflowEffectiveModelRoute {
        model_gateway: required_runtime_value(
            "workflow_route_gateway_unavailable",
            "model gateway",
            runtime.model_gateway.as_deref(),
        )?,
        provider: required_runtime_value(
            "workflow_route_provider_unavailable",
            "provider",
            runtime.provider.as_deref(),
        )?,
        model: required_runtime_value(
            "workflow_route_model_unavailable",
            "model",
            runtime.model.as_deref(),
        )?,
        reasoning: required_runtime_value(
            "workflow_route_reasoning_unavailable",
            "reasoning",
            runtime.reasoning.as_deref(),
        )?,
        service_tier: runtime.service_tier.clone(),
        auth_profile: runtime.auth_profile.clone(),
        approval_policy: runtime.approval_policy.clone(),
        permission_profile: runtime.permission_profile.clone(),
        worktree_mode: worktree_mode.to_string(),
        context_ceiling_tokens,
        fallback_used,
        credit_control: if finite_budget_requested {
            runtime.credit_control.clone()
        } else {
            WorkflowProviderCreditControl::NotRequested
        },
    };
    admit_workflow_model_route(requested, &effective)
}

impl WorkflowRouteReceipt {
    pub fn enforce_provider_attempt(
        &self,
        effective: &WorkflowEffectiveModelRoute,
    ) -> Result<(), WorkflowRouteEnforcementError> {
        let repeated = admit_workflow_model_route(&self.requested, effective)?;
        if repeated.effective != self.effective {
            return Err(route_error(
                "workflow_route_receipt_mismatch",
                "provider attempt route differs from its immutable admission receipt",
            ));
        }
        Ok(())
    }

    pub fn enforce_descendant(
        &self,
        effective: &WorkflowEffectiveModelRoute,
    ) -> Result<(), WorkflowRouteEnforcementError> {
        self.enforce_provider_attempt(effective)
    }

    pub fn terminal_credit_accounting(&self) -> WorkflowProviderCreditTerminalAccounting {
        match &self.effective.credit_control {
            WorkflowProviderCreditControl::NotRequested => {
                WorkflowProviderCreditTerminalAccounting::NotRequested
            }
            WorkflowProviderCreditControl::Reserved {
                reservation_id,
                ceiling_usd,
                spent_usd,
                remaining_usd,
                exhausted,
            } => WorkflowProviderCreditTerminalAccounting::ProviderReadback {
                reservation_id: reservation_id.clone(),
                ceiling_usd: ceiling_usd.clone(),
                spent_usd: spent_usd.clone(),
                remaining_usd: remaining_usd.clone(),
                exhausted: *exhausted,
            },
            WorkflowProviderCreditControl::Unavailable => {
                unreachable!("unavailable credit control cannot be admitted")
            }
        }
    }
}

fn enforce_constraints(
    constraints: &WorkflowModelRoutingConstraints,
    decision_status: WorkflowModelRoutingDecisionStatus,
    effective: &WorkflowEffectiveModelRoute,
) -> Result<(), WorkflowRouteEnforcementError> {
    enforce_list(
        "workflow_route_gateway_mismatch",
        "model gateway",
        effective.model_gateway.as_str(),
        &constraints.allowed_model_gateways,
    )?;
    enforce_list(
        "workflow_route_provider_mismatch",
        "provider",
        effective.provider.as_str(),
        &constraints.allowed_providers,
    )?;
    enforce_list(
        "workflow_route_model_mismatch",
        "model",
        effective.model.as_str(),
        &constraints.allowed_models,
    )?;
    enforce_list(
        "workflow_route_reasoning_mismatch",
        "reasoning",
        effective.reasoning.as_str(),
        &constraints.allowed_reasoning,
    )?;
    enforce_optional_list(
        "workflow_route_service_tier_mismatch",
        "service tier",
        effective.service_tier.as_deref(),
        &constraints.allowed_service_tiers,
    )?;
    enforce_optional_list(
        "workflow_route_auth_profile_mismatch",
        "auth profile",
        effective.auth_profile.as_deref(),
        &constraints.allowed_auth_profiles,
    )?;
    enforce_optional_list(
        "workflow_route_approval_profile_mismatch",
        "approval profile",
        effective.approval_policy.as_deref(),
        &constraints.allowed_approval_policies,
    )?;
    enforce_optional_list(
        "workflow_route_permission_profile_mismatch",
        "permission profile",
        effective.permission_profile.as_deref(),
        &constraints.allowed_permission_profiles,
    )?;
    enforce_list(
        "workflow_route_worktree_mode_mismatch",
        "worktree mode",
        effective.worktree_mode.as_str(),
        &constraints.allowed_worktree_modes,
    )?;

    if decision_status == WorkflowModelRoutingDecisionStatus::Selected {
        enforce_list(
            "workflow_route_gateway_not_preferred",
            "preferred model gateway",
            effective.model_gateway.as_str(),
            &constraints.preferred_model_gateways,
        )?;
        enforce_list(
            "workflow_route_provider_not_preferred",
            "preferred provider",
            effective.provider.as_str(),
            &constraints.preferred_providers,
        )?;
        enforce_list(
            "workflow_route_model_not_preferred",
            "preferred model",
            effective.model.as_str(),
            &constraints.preferred_models,
        )?;
        enforce_list(
            "workflow_route_reasoning_not_preferred",
            "preferred reasoning",
            effective.reasoning.as_str(),
            &constraints.preferred_reasoning,
        )?;
        enforce_optional_list(
            "workflow_route_service_tier_not_preferred",
            "preferred service tier",
            effective.service_tier.as_deref(),
            &constraints.preferred_service_tiers,
        )?;
    }

    if let Some(max_context_tokens) = constraints.max_context_tokens {
        let Some(context_ceiling_tokens) = effective.context_ceiling_tokens else {
            return Err(route_error(
                "workflow_route_context_ceiling_unavailable",
                "runtime cannot establish the requested context ceiling",
            ));
        };
        if context_ceiling_tokens > max_context_tokens {
            return Err(route_error(
                "workflow_route_context_ceiling_exceeded",
                "effective context ceiling exceeds the routing constraint",
            ));
        }
    }
    Ok(())
}

fn enforce_credit_control(
    budget_usd: Option<&str>,
    control: &WorkflowProviderCreditControl,
) -> Result<(), WorkflowRouteEnforcementError> {
    match (budget_usd, control) {
        (None, WorkflowProviderCreditControl::NotRequested) => Ok(()),
        (Some(_), WorkflowProviderCreditControl::Unavailable) => Err(route_error(
            "workflow_route_credit_ceiling_unavailable",
            "provider cannot establish a pre-launch credit reservation and accounting readback",
        )),
        (
            Some(budget_usd),
            WorkflowProviderCreditControl::Reserved {
                ceiling_usd,
                remaining_usd,
                exhausted,
                ..
            },
        ) => {
            if ceiling_usd != budget_usd {
                return Err(route_error(
                    "workflow_route_credit_ceiling_mismatch",
                    "provider reservation ceiling differs from the requested budget",
                ));
            }
            if *exhausted || decimal_is_zero(remaining_usd) {
                return Err(route_error(
                    "workflow_route_credit_ceiling_exhausted",
                    "provider reservation has no credit remaining",
                ));
            }
            Ok(())
        }
        (Some(_), WorkflowProviderCreditControl::NotRequested) => Err(route_error(
            "workflow_route_credit_ceiling_unavailable",
            "finite routing budget has no provider credit reservation",
        )),
        (None, _) => Err(route_error(
            "workflow_route_credit_control_unexpected",
            "provider credit control was supplied without a finite routing budget",
        )),
    }
}

fn decimal_is_zero(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !value.starts_with('-')
        && value
            .chars()
            .all(|character| character.is_ascii_digit() || character == '.')
        && value.chars().any(|character| character.is_ascii_digit())
        && value
            .chars()
            .filter(|character| character.is_ascii_digit())
            .all(|character| character == '0')
}

fn enforce_exact(
    code: &'static str,
    field: &str,
    expected: &str,
    actual: &str,
) -> Result<(), WorkflowRouteEnforcementError> {
    if expected.is_none() || expected == actual {
        Ok(())
    } else {
        Err(route_error(
            code,
            format!("effective {field} does not match the exact requested value"),
        ))
    }
}

fn enforce_optional_exact(
    code: &'static str,
    field: &str,
    expected: Option<&str>,
    actual: Option<&str>,
) -> Result<(), WorkflowRouteEnforcementError> {
    if expected == actual {
        Ok(())
    } else {
        Err(route_error(
            code,
            format!("effective {field} does not match the exact requested value"),
        ))
    }
}

fn enforce_list(
    code: &'static str,
    field: &str,
    actual: &str,
    allowed: &[String],
) -> Result<(), WorkflowRouteEnforcementError> {
    if allowed.is_empty() || allowed.iter().any(|value| value == actual) {
        Ok(())
    } else {
        Err(route_error(
            code,
            format!("effective {field} is not allowed by the routing contract"),
        ))
    }
}

fn enforce_optional_list(
    code: &'static str,
    field: &str,
    actual: Option<&str>,
    allowed: &[String],
) -> Result<(), WorkflowRouteEnforcementError> {
    if allowed.is_empty() || actual.is_some_and(|actual| allowed.iter().any(|value| value == actual))
    {
        Ok(())
    } else {
        Err(route_error(
            code,
            format!("effective {field} is missing or not allowed by the routing contract"),
        ))
    }
}

fn route_error(code: &'static str, message: impl Into<String>) -> WorkflowRouteEnforcementError {
    WorkflowRouteEnforcementError::new(code, message)
}

fn required_runtime_value(
    code: &'static str,
    field: &str,
    value: Option<&str>,
) -> Result<String, WorkflowRouteEnforcementError> {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| route_error(code, format!("runtime {field} is unavailable")))
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::WorkflowEffectiveModelRoute;
    use crate::WorkflowModelRoute;
    use crate::WorkflowModelRouter;
    use crate::WorkflowModelRoutingCapability;
    use crate::WorkflowModelRoutingConstraints;
    use crate::WorkflowModelRoutingContext;
    use crate::WorkflowModelRoutingContract;
    use crate::WorkflowModelRoutingDecision;
    use crate::WorkflowModelRoutingDecisionStatus;
    use crate::WorkflowModelRoutingFallback;
    use crate::WorkflowModelRoutingRequest;
    use crate::WorkflowProviderCreditControl;
    use crate::admit_workflow_model_route;

    fn exact_route() -> WorkflowModelRoute {
        WorkflowModelRoute {
            model_gateway: "openrouter".to_string(),
            provider: "openrouter".to_string(),
            model: "openai/gpt-5.4".to_string(),
            reasoning: "high".to_string(),
            service_tier: Some("priority".to_string()),
            approval_policy: Some("never".to_string()),
            permission_profile: Some(":workspace".to_string()),
            routing: Some(WorkflowModelRoutingContract {
                contract_version: "openrouter.route/v1".to_string(),
                router: WorkflowModelRouter::OpenRouter,
                request: WorkflowModelRoutingRequest {
                    requested_capability: WorkflowModelRoutingCapability::AgentWorker,
                    context: WorkflowModelRoutingContext {
                        auth_profile: Some("account007".to_string()),
                        approval_policy: Some("never".to_string()),
                        permission_profile: Some(":workspace".to_string()),
                        worktree_mode: Some("isolated_worktree".to_string()),
                        ..Default::default()
                    },
                    constraints: WorkflowModelRoutingConstraints {
                        allowed_model_gateways: vec!["openrouter".to_string()],
                        preferred_model_gateways: vec!["openrouter".to_string()],
                        allowed_providers: vec!["openrouter".to_string()],
                        preferred_providers: vec!["openrouter".to_string()],
                        allowed_models: vec!["openai/gpt-5.4".to_string()],
                        preferred_models: vec!["openai/gpt-5.4".to_string()],
                        allowed_reasoning: vec!["high".to_string()],
                        preferred_reasoning: vec!["high".to_string()],
                        allowed_service_tiers: vec!["priority".to_string()],
                        preferred_service_tiers: vec!["priority".to_string()],
                        allowed_auth_profiles: vec!["account007".to_string()],
                        allowed_approval_policies: vec!["never".to_string()],
                        allowed_permission_profiles: vec![":workspace".to_string()],
                        allowed_worktree_modes: vec!["isolated_worktree".to_string()],
                        max_context_tokens: Some(128_000),
                        budget_usd: None,
                        fallback_required: false,
                    },
                },
                decision: Some(WorkflowModelRoutingDecision {
                    status: WorkflowModelRoutingDecisionStatus::Selected,
                    model_gateway: Some("openrouter".to_string()),
                    provider: Some("openrouter".to_string()),
                    model: Some("openai/gpt-5.4".to_string()),
                    reasoning: Some("high".to_string()),
                    service_tier: Some("priority".to_string()),
                    auth_profile: Some("account007".to_string()),
                    explanation: Some("exact route selected".to_string()),
                    fallback: Some(WorkflowModelRoutingFallback {
                        used: false,
                        reason: None,
                    }),
                    warnings: Vec::new(),
                    errors: Vec::new(),
                }),
            }),
        }
    }

    fn effective_route() -> WorkflowEffectiveModelRoute {
        WorkflowEffectiveModelRoute {
            model_gateway: "openrouter".to_string(),
            provider: "openrouter".to_string(),
            model: "openai/gpt-5.4".to_string(),
            reasoning: "high".to_string(),
            service_tier: Some("priority".to_string()),
            auth_profile: Some("account007".to_string()),
            approval_policy: Some("never".to_string()),
            permission_profile: Some(":workspace".to_string()),
            worktree_mode: "isolated_worktree".to_string(),
            context_ceiling_tokens: Some(128_000),
            fallback_used: false,
            credit_control: WorkflowProviderCreditControl::NotRequested,
        }
    }

    #[test]
    fn fully_supported_exact_route_persists_immutable_requested_and_effective_receipt() {
        let route = exact_route();
        let effective = effective_route();

        let receipt = admit_workflow_model_route(&route, &effective).expect("route is supported");

        assert_eq!(receipt.requested, route);
        assert_eq!(receipt.effective, effective);
        assert_eq!(receipt.enforce_provider_attempt(&effective), Ok(()));
        assert_eq!(receipt.enforce_descendant(&effective), Ok(()));
        assert_eq!(
            receipt.terminal_credit_accounting(),
            crate::WorkflowProviderCreditTerminalAccounting::NotRequested
        );
    }

    #[test]
    fn route_receipt_is_identical_across_restart_and_descendant_revalidation() {
        let route = exact_route();
        let effective = effective_route();
        let first = admit_workflow_model_route(&route, &effective).expect("first admission");
        let restarted = admit_workflow_model_route(&route, &effective).expect("restart admission");

        assert_eq!(restarted, first);
        assert_eq!(restarted.enforce_descendant(&effective), Ok(()));
    }

    #[test]
    fn every_disallowed_or_mismatched_route_dimension_fails_closed() {
        let cases: Vec<(&str, Box<dyn FnOnce(&mut WorkflowEffectiveModelRoute)>)> = vec![
            ("workflow_route_gateway_mismatch", Box::new(|route| route.model_gateway = "direct".to_string())),
            ("workflow_route_provider_mismatch", Box::new(|route| route.provider = "openai".to_string())),
            ("workflow_route_model_mismatch", Box::new(|route| route.model = "openai/gpt-4.1".to_string())),
            ("workflow_route_reasoning_mismatch", Box::new(|route| route.reasoning = "medium".to_string())),
            ("workflow_route_service_tier_mismatch", Box::new(|route| route.service_tier = Some("default".to_string()))),
            ("workflow_route_auth_profile_mismatch", Box::new(|route| route.auth_profile = Some("account008".to_string()))),
            ("workflow_route_approval_profile_mismatch", Box::new(|route| route.approval_policy = Some("on-request".to_string()))),
            ("workflow_route_permission_profile_mismatch", Box::new(|route| route.permission_profile = Some(":read-only".to_string()))),
            ("workflow_route_worktree_mode_mismatch", Box::new(|route| route.worktree_mode = "shared_repository".to_string())),
            ("workflow_route_context_ceiling_exceeded", Box::new(|route| route.context_ceiling_tokens = Some(128_001))),
        ];

        for (expected_code, mutate) in cases {
            let mut effective = effective_route();
            mutate(&mut effective);
            let error = admit_workflow_model_route(&exact_route(), &effective)
                .expect_err("mismatched route must fail closed");
            assert_eq!(error.code(), expected_code);
        }
    }

    #[test]
    fn missing_required_fallback_fails_closed() {
        let mut route = exact_route();
        route
            .routing
            .as_mut()
            .expect("routing contract")
            .request
            .constraints
            .fallback_required = true;

        let error = admit_workflow_model_route(&route, &effective_route())
            .expect_err("required fallback cannot be ignored");
        assert_eq!(error.code(), "workflow_route_required_fallback_missing");
    }

    #[test]
    fn finite_budget_without_provider_reservation_fails_before_provider_attempt() {
        let mut route = exact_route();
        route
            .routing
            .as_mut()
            .expect("routing contract")
            .request
            .constraints
            .budget_usd = Some("5.00".to_string());
        let mut effective = effective_route();
        effective.credit_control = WorkflowProviderCreditControl::Unavailable;

        let error = admit_workflow_model_route(&route, &effective)
            .expect_err("finite budgets require a provider reservation");
        assert_eq!(error.code(), "workflow_route_credit_ceiling_unavailable");
    }

    #[test]
    fn exhausted_provider_credit_reservation_blocks_another_attempt_and_has_exact_readback() {
        let mut route = exact_route();
        route
            .routing
            .as_mut()
            .expect("routing contract")
            .request
            .constraints
            .budget_usd = Some("5.00".to_string());
        let mut effective = effective_route();
        effective.credit_control = WorkflowProviderCreditControl::Reserved {
            reservation_id: "reservation-1".to_string(),
            ceiling_usd: "5.00".to_string(),
            spent_usd: "5.00".to_string(),
            remaining_usd: "0.00".to_string(),
            exhausted: true,
        };

        let error = admit_workflow_model_route(&route, &effective)
            .expect_err("an exhausted reservation cannot admit provider work");
        assert_eq!(error.code(), "workflow_route_credit_ceiling_exhausted");
    }

    #[test]
    fn reserved_credit_ceiling_is_immutable_and_terminal_accounting_is_exact() {
        let mut route = exact_route();
        route
            .routing
            .as_mut()
            .expect("routing contract")
            .request
            .constraints
            .budget_usd = Some("5.00".to_string());
        let mut effective = effective_route();
        effective.credit_control = WorkflowProviderCreditControl::Reserved {
            reservation_id: "reservation-1".to_string(),
            ceiling_usd: "5.00".to_string(),
            spent_usd: "1.25".to_string(),
            remaining_usd: "3.75".to_string(),
            exhausted: false,
        };

        let receipt = admit_workflow_model_route(&route, &effective).expect("reserved route");
        assert_eq!(receipt.enforce_provider_attempt(&effective), Ok(()));
        assert_eq!(
            receipt.terminal_credit_accounting(),
            crate::WorkflowProviderCreditTerminalAccounting::ProviderReadback {
                reservation_id: "reservation-1".to_string(),
                ceiling_usd: "5.00".to_string(),
                spent_usd: "1.25".to_string(),
                remaining_usd: "3.75".to_string(),
                exhausted: false,
            }
        );
    }
}

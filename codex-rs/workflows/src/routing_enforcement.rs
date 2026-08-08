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

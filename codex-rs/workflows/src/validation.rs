use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;

use crate::MAX_WORKFLOW_PROMPT_FIELD_CHARS;
use crate::SUPPORTED_SCHEMA_VERSION;
use crate::WorkflowCompletion;
use crate::WorkflowLimits;
use crate::WorkflowLoopSchedule;
use crate::WorkflowModelRoute;
use crate::WorkflowMonitorLink;
use crate::WorkflowSpec;
use crate::WorkflowSpecError;
use crate::WorkflowSpecResult;
use crate::WorkflowStatus;
use crate::WorkflowStep;
use crate::WorkflowStopCondition;
use crate::WorkflowVerifier;
use crate::ancient_names::is_ancient_display_name;

const CANDIDATE_SUCCEEDED: &str = "candidate_succeeded";
const ARTIFACT_CONTAINS_VERIFIER: &str = "artifact_contains";
const RUN_COMMANDS_VERIFIER: &str = "run_commands";
const MAX_VERIFIER_RETRY_ATTEMPTS: u32 = 5;
const MAX_WORKFLOW_LOOPS: usize = 32;
const MAX_WORKFLOW_LOOP_ITERATIONS: u32 = 10_000;
const MAX_WORKFLOW_MONITOR_LINKS: usize = 32;
const MAX_WORKFLOW_MONITOR_EVENTS_PER_TICK: u32 = 50;
const WORKFLOW_MONITOR_SOURCE_EXISTING_THREAD_MONITOR: &str = "existing_thread_monitor";
const PLACEHOLDER_ROUTE_VALUES: &[&str] = &[
    "ambient",
    "auto",
    "current",
    "default",
    "infer",
    "inherit",
    "inherited",
];

impl WorkflowSpec {
    pub fn validate(&self) -> WorkflowSpecResult<()> {
        if self.schema_version != SUPPORTED_SCHEMA_VERSION {
            return Err(WorkflowSpecError::invalid(format!(
                "unsupported schema_version `{}`; expected `{SUPPORTED_SCHEMA_VERSION}`",
                self.schema_version
            )));
        }
        validate_identifier("workflow_id", &self.workflow_id)?;
        validate_prompt_field("display_name", &self.display_name)?;
        validate_non_empty("source_prompt", &self.source_prompt)?;
        self.execution_defaults.validate("execution_defaults")?;
        self.limits.validate()?;
        validate_string_list("approvals.required_before", &self.approvals.required_before)?;
        validate_string_list("artifacts.required", &self.artifacts.required)?;
        validate_non_empty("artifacts.retention", &self.artifacts.retention)?;
        validate_string_list("cleanup.on_cancel", &self.cleanup.on_cancel)?;
        validate_string_list("cleanup.on_complete", &self.cleanup.on_complete)?;

        validate_agents(self)?;
        validate_steps(self)?;
        let step_ids = self
            .steps
            .iter()
            .map(|step| step.id.as_str())
            .collect::<BTreeSet<_>>();
        validate_loops(self, &step_ids)?;
        validate_monitor_links(&self.monitors, &step_ids)?;

        match self.status {
            WorkflowStatus::Draft => {
                if self.agents.is_empty() {
                    return Err(WorkflowSpecError::invalid(
                        "draft workflows must define at least one agent",
                    ));
                }
                if self.steps.is_empty() {
                    return Err(WorkflowSpecError::invalid(
                        "draft workflows must define at least one step",
                    ));
                }
                validate_adversarial_work(self)?;
            }
            WorkflowStatus::NeedsClarification => {
                if self.questions.is_empty() && self.blocking_reasons.is_empty() {
                    return Err(WorkflowSpecError::invalid(
                        "needs_clarification workflows must include questions or blocking_reasons",
                    ));
                }
            }
            WorkflowStatus::Blocked => {
                if self.blocking_reasons.is_empty() {
                    return Err(WorkflowSpecError::invalid(
                        "blocked workflows must include blocking_reasons",
                    ));
                }
            }
        }

        Ok(())
    }
}

impl WorkflowModelRoute {
    fn validate(&self, path: &str) -> WorkflowSpecResult<()> {
        validate_route_field(path, "model_gateway", &self.model_gateway)?;
        validate_route_field(path, "provider", &self.provider)?;
        validate_route_field(path, "model", &self.model)?;
        validate_route_field(path, "reasoning", &self.reasoning)?;
        if let Some(service_tier) = &self.service_tier {
            validate_route_field(path, "service_tier", service_tier)?;
        }
        if let Some(approval_policy) = &self.approval_policy {
            validate_non_empty(&format!("{path}.approval_policy"), approval_policy)?;
        }
        if let Some(permission_profile) = &self.permission_profile {
            validate_non_empty(&format!("{path}.permission_profile"), permission_profile)?;
        }
        Ok(())
    }
}

impl WorkflowLimits {
    fn validate(&self) -> WorkflowSpecResult<()> {
        if self.max_parallel_steps == 0 {
            return Err(WorkflowSpecError::invalid(
                "limits.max_parallel_steps must be positive",
            ));
        }
        if self.max_agents == 0 {
            return Err(WorkflowSpecError::invalid(
                "limits.max_agents must be positive",
            ));
        }
        if self.max_runtime_seconds == 0 {
            return Err(WorkflowSpecError::invalid(
                "limits.max_runtime_seconds must be positive",
            ));
        }
        if self.max_step_runtime_seconds == 0 {
            return Err(WorkflowSpecError::invalid(
                "limits.max_step_runtime_seconds must be positive",
            ));
        }
        if self.max_tokens == 0 {
            return Err(WorkflowSpecError::invalid(
                "limits.max_tokens must be positive",
            ));
        }
        if self.max_tool_calls == 0 {
            return Err(WorkflowSpecError::invalid(
                "limits.max_tool_calls must be positive",
            ));
        }
        Ok(())
    }
}

fn validate_agents(spec: &WorkflowSpec) -> WorkflowSpecResult<()> {
    if spec.agents.len() > spec.limits.max_agents as usize {
        return Err(WorkflowSpecError::invalid(format!(
            "workflow defines {} agents but limits.max_agents is {}",
            spec.agents.len(),
            spec.limits.max_agents
        )));
    }

    let mut agent_ids = BTreeSet::new();
    for agent in &spec.agents {
        validate_identifier("agent.id", &agent.id)?;
        if !agent_ids.insert(agent.id.as_str()) {
            return Err(WorkflowSpecError::invalid(format!(
                "agent id `{}` is duplicated",
                agent.id
            )));
        }
        if !is_ancient_display_name(&agent.display_name) {
            return Err(WorkflowSpecError::invalid(format!(
                "agent `{}` display_name `{}` must be role-first with an approved ancient name",
                agent.id, agent.display_name
            )));
        }
        validate_non_empty("agent.role", &agent.role)?;
        agent
            .model
            .validate(&format!("agents.{}.model", agent.id))?;
    }
    Ok(())
}

fn validate_steps(spec: &WorkflowSpec) -> WorkflowSpecResult<()> {
    let agent_ids = spec
        .agents
        .iter()
        .map(|agent| agent.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut step_ids = BTreeSet::new();
    for step in &spec.steps {
        validate_identifier("step.id", &step.id)?;
        if !step_ids.insert(step.id.as_str()) {
            return Err(WorkflowSpecError::invalid(format!(
                "step id `{}` is duplicated",
                step.id
            )));
        }
    }

    for step in &spec.steps {
        validate_prompt_field("step.title", &step.title)?;
        if !agent_ids.contains(step.agent.as_str()) {
            return Err(WorkflowSpecError::invalid(format!(
                "step `{}` references unknown agent `{}`",
                step.id, step.agent
            )));
        }
        let Some(model) = &step.model else {
            return Err(WorkflowSpecError::invalid(format!(
                "step `{}` must include an exact model route",
                step.id
            )));
        };
        model.validate(&format!("steps.{}.model", step.id))?;
        if let Some(workspace) = &step.workspace {
            validate_non_empty("step.workspace.mode", &workspace.mode)?;
        }
        if let Some(parallel_group) = &step.parallel_group {
            validate_identifier("step.parallel_group", parallel_group)?;
        }
        validate_string_list("step.outputs", &step.outputs)?;
        validate_dependencies(step, &step_ids)?;
        let Some(completion) = &step.completion else {
            return Err(WorkflowSpecError::invalid(format!(
                "step `{}` must include verifier-gated completion",
                step.id
            )));
        };
        validate_completion(&step.id, completion)?;
    }

    validate_acyclic_dependencies(&spec.steps)
}

fn validate_loops(spec: &WorkflowSpec, step_ids: &BTreeSet<&str>) -> WorkflowSpecResult<()> {
    if spec.loops.len() > MAX_WORKFLOW_LOOPS {
        return Err(WorkflowSpecError::invalid(format!(
            "workflow defines {} loops but the maximum is {MAX_WORKFLOW_LOOPS}",
            spec.loops.len()
        )));
    }
    let mut loop_ids = BTreeSet::new();
    for workflow_loop in &spec.loops {
        validate_identifier("loop.id", &workflow_loop.id)?;
        if !loop_ids.insert(workflow_loop.id.as_str()) {
            return Err(WorkflowSpecError::invalid(format!(
                "loop id `{}` is duplicated",
                workflow_loop.id
            )));
        }
        validate_prompt_field("loop.title", &workflow_loop.title)?;
        validate_non_empty("loop.timezone", &workflow_loop.timezone)?;
        validate_loop_schedule(&workflow_loop.id, &workflow_loop.schedule)?;
        if workflow_loop.max_iterations == 0 {
            return Err(WorkflowSpecError::invalid(format!(
                "loop `{}` max_iterations must be positive",
                workflow_loop.id
            )));
        }
        if workflow_loop.max_iterations > MAX_WORKFLOW_LOOP_ITERATIONS {
            return Err(WorkflowSpecError::invalid(format!(
                "loop `{}` max_iterations must be at most {MAX_WORKFLOW_LOOP_ITERATIONS}",
                workflow_loop.id
            )));
        }
        if let Some(expires_after_seconds) = workflow_loop.expires_after_seconds
            && expires_after_seconds == 0
        {
            return Err(WorkflowSpecError::invalid(format!(
                "loop `{}` expires_after_seconds must be positive",
                workflow_loop.id
            )));
        }
        validate_optional_step_ref("loop.trigger_step", &workflow_loop.trigger_step, step_ids)?;
        validate_stop_condition(
            &format!("loop `{}` stop_condition", workflow_loop.id),
            &workflow_loop.stop_condition,
            step_ids,
        )?;
    }
    Ok(())
}

fn validate_loop_schedule(
    loop_id: &str,
    schedule: &WorkflowLoopSchedule,
) -> WorkflowSpecResult<()> {
    match schedule {
        WorkflowLoopSchedule::Dynamic => {}
        WorkflowLoopSchedule::Interval { amount, .. } => {
            if *amount == 0 {
                return Err(WorkflowSpecError::invalid(format!(
                    "loop `{loop_id}` interval amount must be positive"
                )));
            }
        }
        WorkflowLoopSchedule::Cron { expression } => {
            validate_non_empty("loop.schedule.expression", expression)?;
        }
    }
    Ok(())
}

fn validate_monitor_links(
    monitors: &[WorkflowMonitorLink],
    step_ids: &BTreeSet<&str>,
) -> WorkflowSpecResult<()> {
    if monitors.len() > MAX_WORKFLOW_MONITOR_LINKS {
        return Err(WorkflowSpecError::invalid(format!(
            "workflow defines {} monitors but the maximum is {MAX_WORKFLOW_MONITOR_LINKS}",
            monitors.len()
        )));
    }
    let mut monitor_ids = BTreeSet::new();
    for monitor in monitors {
        validate_identifier("monitor.id", &monitor.id)?;
        if !monitor_ids.insert(monitor.id.as_str()) {
            return Err(WorkflowSpecError::invalid(format!(
                "monitor id `{}` is duplicated",
                monitor.id
            )));
        }
        validate_prompt_field("monitor.title", &monitor.title)?;
        if monitor.source != WORKFLOW_MONITOR_SOURCE_EXISTING_THREAD_MONITOR {
            return Err(WorkflowSpecError::invalid(format!(
                "monitor `{}` source must be `{WORKFLOW_MONITOR_SOURCE_EXISTING_THREAD_MONITOR}`",
                monitor.id
            )));
        }
        if monitor.max_events_per_tick == 0 {
            return Err(WorkflowSpecError::invalid(format!(
                "monitor `{}` max_events_per_tick must be positive",
                monitor.id
            )));
        }
        if monitor.max_events_per_tick > MAX_WORKFLOW_MONITOR_EVENTS_PER_TICK {
            return Err(WorkflowSpecError::invalid(format!(
                "monitor `{}` max_events_per_tick must be at most {MAX_WORKFLOW_MONITOR_EVENTS_PER_TICK}",
                monitor.id
            )));
        }
        validate_optional_step_ref("monitor.trigger_step", &monitor.trigger_step, step_ids)?;
        if let Some(stop_condition) = &monitor.stop_condition {
            validate_stop_condition(
                &format!("monitor `{}` stop_condition", monitor.id),
                stop_condition,
                step_ids,
            )?;
        }
    }
    Ok(())
}

fn validate_optional_step_ref(
    path: &str,
    step_id: &Option<String>,
    step_ids: &BTreeSet<&str>,
) -> WorkflowSpecResult<()> {
    let Some(step_id) = step_id else {
        return Ok(());
    };
    validate_identifier(path, step_id)?;
    if !step_ids.contains(step_id.as_str()) {
        return Err(WorkflowSpecError::invalid(format!(
            "{path} references unknown step `{step_id}`"
        )));
    }
    Ok(())
}

fn validate_stop_condition(
    path: &str,
    stop_condition: &WorkflowStopCondition,
    step_ids: &BTreeSet<&str>,
) -> WorkflowSpecResult<()> {
    match stop_condition {
        WorkflowStopCondition::WorkflowComplete => Ok(()),
        WorkflowStopCondition::StepSucceeded { step } => {
            validate_identifier(path, step)?;
            if !step_ids.contains(step.as_str()) {
                return Err(WorkflowSpecError::invalid(format!(
                    "{path} references unknown step `{step}`"
                )));
            }
            Ok(())
        }
    }
}

fn validate_dependencies(step: &WorkflowStep, step_ids: &BTreeSet<&str>) -> WorkflowSpecResult<()> {
    let mut seen = BTreeSet::new();
    for dependency in &step.depends_on {
        validate_identifier("step.depends_on", dependency)?;
        if dependency == &step.id {
            return Err(WorkflowSpecError::invalid(format!(
                "step `{}` cannot depend on itself",
                step.id
            )));
        }
        if !step_ids.contains(dependency.as_str()) {
            return Err(WorkflowSpecError::invalid(format!(
                "step `{}` depends on unknown step `{dependency}`",
                step.id
            )));
        }
        if !seen.insert(dependency.as_str()) {
            return Err(WorkflowSpecError::invalid(format!(
                "step `{}` lists dependency `{dependency}` more than once",
                step.id
            )));
        }
    }
    Ok(())
}

fn validate_completion(step_id: &str, completion: &WorkflowCompletion) -> WorkflowSpecResult<()> {
    if completion.model_marked_state != CANDIDATE_SUCCEEDED {
        return Err(WorkflowSpecError::invalid(format!(
            "step `{step_id}` model_marked_state must be `{CANDIDATE_SUCCEEDED}`"
        )));
    }
    if completion.verifiers.is_empty() {
        return Err(WorkflowSpecError::invalid(format!(
            "step `{step_id}` must include at least one verifier"
        )));
    }
    let mut verifier_ids = BTreeSet::new();
    for verifier in &completion.verifiers {
        validate_verifier(step_id, verifier)?;
        if !verifier_ids.insert(verifier.id.as_str()) {
            return Err(WorkflowSpecError::invalid(format!(
                "step `{step_id}` verifier id `{}` is duplicated",
                verifier.id
            )));
        }
    }
    Ok(())
}

fn validate_verifier(step_id: &str, verifier: &WorkflowVerifier) -> WorkflowSpecResult<()> {
    validate_identifier("verifier.id", &verifier.id)?;
    match verifier.kind.as_str() {
        ARTIFACT_CONTAINS_VERIFIER => {
            let artifact = verifier.artifact.as_deref().unwrap_or_default();
            validate_non_empty("verifier.artifact", artifact)?;
            validate_string_list("verifier.must_contain", &verifier.must_contain)?;
            if verifier.must_contain.is_empty() {
                return Err(WorkflowSpecError::invalid(format!(
                    "step `{step_id}` artifact_contains verifier `{}` must include must_contain",
                    verifier.id
                )));
            }
        }
        RUN_COMMANDS_VERIFIER => {
            for (field, value) in [
                ("cwd", verifier.cwd.as_deref()),
                ("sandbox", verifier.sandbox.as_deref()),
                ("network", verifier.network.as_deref()),
            ] {
                validate_non_empty(&format!("verifier.{field}"), value.unwrap_or_default())?;
            }
            if verifier.timeout_seconds.unwrap_or_default() == 0 {
                return Err(WorkflowSpecError::invalid(format!(
                    "step `{step_id}` run_commands verifier `{}` must set timeout_seconds",
                    verifier.id
                )));
            }
            if verifier.output_limit_bytes.unwrap_or_default() == 0 {
                return Err(WorkflowSpecError::invalid(format!(
                    "step `{step_id}` run_commands verifier `{}` must set output_limit_bytes",
                    verifier.id
                )));
            }
            validate_string_list("verifier.commands", &verifier.commands)?;
            if verifier.commands.is_empty() {
                return Err(WorkflowSpecError::invalid(format!(
                    "step `{step_id}` run_commands verifier `{}` must include commands",
                    verifier.id
                )));
            }
        }
        _ => {
            return Err(WorkflowSpecError::invalid(format!(
                "step `{step_id}` verifier `{}` has unsupported type `{}`",
                verifier.id, verifier.kind
            )));
        }
    }
    if let Some(retry_policy) = &verifier.retry_policy
        && retry_policy.max_attempts == 0
    {
        return Err(WorkflowSpecError::invalid(format!(
            "step `{step_id}` verifier `{}` retry_policy.max_attempts must be positive",
            verifier.id
        )));
    }
    if let Some(retry_policy) = &verifier.retry_policy
        && retry_policy.max_attempts > MAX_VERIFIER_RETRY_ATTEMPTS
    {
        return Err(WorkflowSpecError::invalid(format!(
            "step `{step_id}` verifier `{}` retry_policy.max_attempts must be at most {MAX_VERIFIER_RETRY_ATTEMPTS}",
            verifier.id
        )));
    }
    Ok(())
}

fn validate_acyclic_dependencies(steps: &[WorkflowStep]) -> WorkflowSpecResult<()> {
    let mut indegree = BTreeMap::new();
    let mut outgoing: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for step in steps {
        indegree.insert(step.id.as_str(), 0usize);
    }
    for step in steps {
        for dependency in &step.depends_on {
            let Some(degree) = indegree.get_mut(step.id.as_str()) else {
                return Err(WorkflowSpecError::invalid(format!(
                    "step `{}` was missing from dependency index",
                    step.id
                )));
            };
            *degree += 1;
            outgoing
                .entry(dependency.as_str())
                .or_default()
                .push(step.id.as_str());
        }
    }

    let mut ready = indegree
        .iter()
        .filter_map(|(step_id, degree)| (*degree == 0).then_some(*step_id))
        .collect::<VecDeque<_>>();
    let mut visited = 0usize;
    while let Some(step_id) = ready.pop_front() {
        visited += 1;
        if let Some(dependents) = outgoing.get(step_id) {
            for dependent in dependents.iter().copied() {
                let Some(degree) = indegree.get_mut(dependent) else {
                    return Err(WorkflowSpecError::invalid(format!(
                        "step `{dependent}` was missing from dependency index",
                    )));
                };
                *degree -= 1;
                if *degree == 0 {
                    ready.push_back(dependent);
                }
            }
        }
    }

    if visited != steps.len() {
        return Err(WorkflowSpecError::invalid(
            "workflow step dependencies contain a cycle",
        ));
    }
    Ok(())
}

fn validate_adversarial_work(spec: &WorkflowSpec) -> WorkflowSpecResult<()> {
    let adversarial_agents = spec
        .agents
        .iter()
        .filter(|agent| {
            is_adversarial_text(&agent.id)
                || is_adversarial_text(&agent.display_name)
                || is_adversarial_text(&agent.role)
        })
        .count();
    let adversarial_steps = spec
        .steps
        .iter()
        .filter(|step| is_adversarial_text(&step.id) || is_adversarial_text(&step.title))
        .count();
    if adversarial_agents < 2 && adversarial_steps < 2 {
        return Err(WorkflowSpecError::invalid(
            "draft workflows must include adversarial work by at least two agents or two steps",
        ));
    }
    Ok(())
}

fn is_adversarial_text(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains("adversary")
        || value.contains("adversarial")
        || value.contains("red team")
        || value.contains("red_team")
}

fn validate_route_field(path: &str, field: &str, value: &str) -> WorkflowSpecResult<()> {
    validate_non_empty(&format!("{path}.{field}"), value)?;
    let normalized = value.trim().to_ascii_lowercase();
    if PLACEHOLDER_ROUTE_VALUES.contains(&normalized.as_str()) {
        return Err(WorkflowSpecError::invalid(format!(
            "{path}.{field} must be exact and cannot use placeholder `{value}`"
        )));
    }
    Ok(())
}

fn validate_string_list(path: &str, values: &[String]) -> WorkflowSpecResult<()> {
    for (index, value) in values.iter().enumerate() {
        validate_non_empty(&format!("{path}[{index}]"), value)?;
    }
    Ok(())
}

fn validate_identifier(path: &str, value: &str) -> WorkflowSpecResult<()> {
    validate_non_empty(path, value)?;
    if value.len() > 128 {
        return Err(WorkflowSpecError::invalid(format!(
            "{path} `{value}` is too long; maximum is 128 bytes"
        )));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(WorkflowSpecError::invalid(format!(
            "{path} `{value}` must contain only ASCII letters, numbers, underscores, or hyphens"
        )));
    }
    Ok(())
}

fn validate_non_empty(path: &str, value: &str) -> WorkflowSpecResult<()> {
    if value.trim().is_empty() {
        return Err(WorkflowSpecError::invalid(format!(
            "{path} must not be empty"
        )));
    }
    Ok(())
}

fn validate_prompt_field(path: &str, value: &str) -> WorkflowSpecResult<()> {
    validate_non_empty(path, value)?;
    let len = value.chars().count();
    if len > MAX_WORKFLOW_PROMPT_FIELD_CHARS {
        return Err(WorkflowSpecError::invalid(format!(
            "{path} is too long; maximum is {MAX_WORKFLOW_PROMPT_FIELD_CHARS} characters"
        )));
    }
    Ok(())
}

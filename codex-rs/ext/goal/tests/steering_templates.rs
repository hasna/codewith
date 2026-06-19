#[test]
fn continuation_template_requires_adversarial_agent_verification() {
    assert_goal_prompt_requires_adversarial_agent(include_str!(
        "../templates/goals/continuation.md"
    ));
}

#[test]
fn objective_updated_template_requires_adversarial_agent_verification() {
    assert_goal_prompt_requires_adversarial_agent(include_str!(
        "../templates/goals/objective_updated.md"
    ));
}

#[test]
fn budget_limit_template_blocks_unverified_completion() {
    let prompt = include_str!("../templates/goals/budget_limit.md");

    assert!(prompt.contains("adversarial verification has already been reconciled"));
    assert!(prompt.contains("completion remains unverified"));
}

fn assert_goal_prompt_requires_adversarial_agent(prompt: &str) {
    assert!(prompt.contains("Use at least one adversarial agent"));
    assert!(prompt.contains("even if the user did not ask for one"));
    assert!(prompt.contains("adversarial self-review"));
}

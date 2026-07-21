use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use codex_core::config::Config;
use codex_core::config::ConfigBuilder;
use codex_core_skills::HostLoadedSkills;
use codex_core_skills::SkillsLoadInput;
use codex_core_skills::SkillsManager;
use codex_core_skills::injection::InjectedHostSkillPrompts;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionRegistry;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::FunctionCallError;
use codex_extension_api::NoopTurnItemEmitter;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolPayload;
use codex_extension_api::TurnInputContext;
use codex_extension_api::TurnInputEnvironment;
use codex_extension_api::TurnStopInput;
use codex_protocol::protocol::SKILLS_INSTRUCTIONS_OPEN_TAG;
use codex_protocol::protocol::SessionSource;
use codex_protocol::user_input::UserInput;
use codex_skills_extension::HostSkillProvider;
use codex_skills_extension::SkillProviderSource;
use codex_skills_extension::SkillProviders;
use codex_skills_extension::catalog::SkillAuthority;
use codex_skills_extension::catalog::SkillAvailability;
use codex_skills_extension::catalog::SkillCatalog;
use codex_skills_extension::catalog::SkillCatalogEntry;
use codex_skills_extension::catalog::SkillPackageId;
use codex_skills_extension::catalog::SkillProviderError;
use codex_skills_extension::catalog::SkillReadResult;
use codex_skills_extension::catalog::SkillResourceId;
use codex_skills_extension::catalog::SkillSearchMatch;
use codex_skills_extension::catalog::SkillSearchResult;
use codex_skills_extension::catalog::SkillSourceKind;
use codex_skills_extension::install;
use codex_skills_extension::install_with_providers;
use codex_skills_extension::provider::SkillListQuery;
use codex_skills_extension::provider::SkillProvider;
use codex_skills_extension::provider::SkillProviderFuture;
use codex_skills_extension::provider::SkillReadRequest;
use codex_skills_extension::provider::SkillSearchRequest;
use codex_utils_output_truncation::TruncationPolicy;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

type TestResult = Result<(), Box<dyn std::error::Error>>;

static NEXT_CODEX_HOME_ID: AtomicUsize = AtomicUsize::new(0);

#[tokio::test]
async fn installed_extension_loads_host_skills_from_legacy_roots() -> TestResult {
    let codex_home = test_codex_home();
    let skill_path = codex_home.join("skills").join("demo").join("SKILL.md");
    std::fs::create_dir_all(
        skill_path
            .parent()
            .ok_or("skill path should have a parent")?,
    )?;
    std::fs::write(
        &skill_path,
        "---\nname: demo\ndescription: Demo skill.\n---\n# Demo\n\nUse the demo skill.\n",
    )?;
    let config = ConfigBuilder::default()
        .codex_home(codex_home.clone())
        .fallback_cwd(Some(codex_home.clone()))
        .build()
        .await?;

    let mut builder = ExtensionRegistryBuilder::new();
    install(&mut builder);
    let registry = builder.build();
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");
    let session_source = SessionSource::Cli;
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &session_source,
            persistent_thread_state_available: true,
            session_store: &session_store,
            thread_store: &thread_store,
        })
        .await;

    let manager = SkillsManager::new(config.codex_home.clone(), config.bundled_skills_enabled());
    let input = SkillsLoadInput::new(
        config.cwd.clone(),
        Vec::new(),
        config.config_layer_stack.clone(),
        config.bundled_skills_enabled(),
    );
    let loaded_skills = Arc::new(manager.skills_for_config(&input, /*fs*/ None).await);
    let skill_path_string = loaded_skills
        .skills
        .iter()
        .find(|skill| skill.name == "demo")
        .ok_or("demo skill should load")?
        .path_to_skills_md
        .to_string_lossy()
        .into_owned();
    let skill_prompt_path = skill_path_string.replace('\\', "/");
    let turn_store = ExtensionData::new("turn-1");
    turn_store.insert(HostLoadedSkills::new(Arc::clone(&loaded_skills)));

    let fragments = registry.turn_input_contributors()[0]
        .contribute(
            TurnInputContext {
                turn_id: "turn-1".to_string(),
                user_input: vec![UserInput::Text {
                    text: "$demo".to_string(),
                    text_elements: Vec::new(),
                }],
                environments: Vec::new(),
            },
            &session_store,
            &thread_store,
            &turn_store,
        )
        .await;

    assert_eq!(2, fragments.len());
    assert!(fragments[0].render().contains("demo"));
    assert!(fragments[0].render().contains(&skill_prompt_path));
    assert_eq!("user", fragments[1].role());
    assert!(fragments[1].render().contains("<name>demo</name>"));
    assert!(fragments[1].render().contains("# Demo"));
    assert!(fragments[1].render().contains(&skill_prompt_path));
    let injected_host_skill_prompts = turn_store
        .get::<InjectedHostSkillPrompts>()
        .ok_or("host skill prompt marker should be set")?;
    assert!(injected_host_skill_prompts.contains_path(&skill_path_string));

    let read_tool = find_tool(
        &registry,
        &session_store,
        &thread_store,
        ToolName::namespaced("skills", "read"),
    );
    let read_output = call_tool(
        read_tool,
        "turn-1",
        json!({
            "authority": { "kind": { "type": "host" }, "id": "host" },
            "package": &skill_path_string,
            "resource": &skill_path_string,
        }),
    )
    .await?;
    assert_eq!(
        read_output["contents"].as_str(),
        Some("---\nname: demo\ndescription: Demo skill.\n---\n# Demo\n\nUse the demo skill.\n")
    );
    assert_eq!(read_output["truncated"], false);

    std::fs::remove_dir_all(codex_home)?;
    Ok(())
}

#[tokio::test]
async fn host_provider_maps_manual_only_policy_to_deferred_and_disabled_takes_precedence()
-> TestResult {
    let codex_home = test_codex_home();
    let skill_path = codex_home.join("skills").join("demo").join("SKILL.md");
    let skill_dir = skill_path
        .parent()
        .ok_or("skill path should have a parent")?;
    std::fs::create_dir_all(skill_dir.join("agents"))?;
    std::fs::write(
        &skill_path,
        "---\nname: demo\ndescription: Demo skill.\n---\n# Demo\n",
    )?;
    std::fs::write(
        skill_dir.join("agents").join("openai.yaml"),
        "policy:\n  allow_implicit_invocation: false\n",
    )?;
    let config = ConfigBuilder::default()
        .codex_home(codex_home.clone())
        .fallback_cwd(Some(codex_home.clone()))
        .build()
        .await?;
    let manager = SkillsManager::new(config.codex_home.clone(), config.bundled_skills_enabled());
    let input = SkillsLoadInput::new(
        config.cwd.clone(),
        Vec::new(),
        config.config_layer_stack.clone(),
        config.bundled_skills_enabled(),
    );
    let loaded_skills = manager.skills_for_config(&input, /*fs*/ None).await;
    let loaded_skill = loaded_skills
        .skills
        .iter()
        .find(|skill| skill.name == "demo")
        .ok_or("demo skill should load")?;
    let loaded_skill_path = loaded_skill.path_to_skills_md.clone();
    let provider = HostSkillProvider::new();
    let catalog = provider
        .list(SkillListQuery {
            turn_id: "turn-1".to_string(),
            executor_authorities: Vec::new(),
            host: Some(Arc::new(HostLoadedSkills::new(Arc::new(
                loaded_skills.clone(),
            )))),
            include_host_skills: true,
            include_bundled_skills: true,
            include_remote_skills: true,
        })
        .await?;
    let deferred_entry = catalog
        .entries
        .iter()
        .find(|entry| entry.name == "demo")
        .ok_or("demo catalog entry should exist")?;
    assert_eq!(deferred_entry.availability, SkillAvailability::Deferred);
    assert!(deferred_entry.is_searchable());
    assert!(deferred_entry.is_explicitly_loadable());

    let mut disabled_skills = loaded_skills;
    disabled_skills.disabled_paths.insert(loaded_skill_path);
    let disabled_catalog = provider
        .list(SkillListQuery {
            turn_id: "turn-2".to_string(),
            executor_authorities: Vec::new(),
            host: Some(Arc::new(HostLoadedSkills::new(Arc::new(disabled_skills)))),
            include_host_skills: true,
            include_bundled_skills: true,
            include_remote_skills: true,
        })
        .await?;
    let disabled_entry = disabled_catalog
        .entries
        .iter()
        .find(|entry| entry.name == "demo")
        .ok_or("disabled demo catalog entry should exist")?;
    assert_eq!(disabled_entry.availability, SkillAvailability::Disabled);
    assert!(!disabled_entry.is_searchable());
    assert!(!disabled_entry.is_explicitly_loadable());

    std::fs::remove_dir_all(codex_home)?;
    Ok(())
}

#[tokio::test]
async fn installed_extension_injects_available_catalog_and_selected_entrypoint() -> TestResult {
    let host_read_requests = Arc::new(Mutex::new(Vec::new()));
    let remote_read_requests = Arc::new(Mutex::new(Vec::new()));
    let host_provider = Arc::new(StaticSkillProvider {
        catalog: SkillCatalog {
            entries: vec![test_entry(
                SkillSourceKind::Host,
                "host",
                "host/lint-fix",
                "lint-fix/SKILL.md",
            )],
            warnings: Vec::new(),
        },
        read_requests: Arc::clone(&host_read_requests),
    });
    let remote_provider = Arc::new(StaticSkillProvider {
        catalog: SkillCatalog {
            entries: vec![test_entry(
                SkillSourceKind::Remote,
                "remote",
                "remote/lint-fix",
                "lint-fix/SKILL.md",
            )],
            warnings: Vec::new(),
        },
        read_requests: Arc::clone(&remote_read_requests),
    });
    let providers = SkillProviders::new()
        .with_host_provider(host_provider)
        .with_remote_provider(remote_provider);
    let mut builder = ExtensionRegistryBuilder::new();
    install_with_providers(&mut builder, providers);
    let registry = builder.build();

    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");
    let session_source = SessionSource::Cli;
    let config = default_config().await?;
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &session_source,
            persistent_thread_state_available: true,
            session_store: &session_store,
            thread_store: &thread_store,
        })
        .await;

    let turn_store = ExtensionData::new("turn-1");
    let fragments = registry.turn_input_contributors()[0]
        .contribute(
            TurnInputContext {
                turn_id: "turn-1".to_string(),
                user_input: vec![UserInput::Text {
                    text: "$lint-fix please".to_string(),
                    text_elements: Vec::new(),
                }],
                environments: vec![TurnInputEnvironment {
                    environment_id: "env-1".to_string(),
                    cwd: std::env::temp_dir(),
                    is_primary: true,
                }],
            },
            &session_store,
            &thread_store,
            &turn_store,
        )
        .await;

    assert_eq!(2, fragments.len());
    assert_eq!("developer", fragments[0].role());
    assert!(
        fragments[0]
            .render()
            .starts_with(SKILLS_INSTRUCTIONS_OPEN_TAG)
    );
    assert!(fragments[0].render().contains("lint-fix"));
    assert_eq!("user", fragments[1].role());
    assert!(fragments[1].render().contains("<name>lint-fix</name>"));
    assert!(fragments[1].render().contains("# Lint Fix"));
    assert_eq!(
        vec![(
            SkillAuthority::new(SkillSourceKind::Host, "host"),
            SkillPackageId("host/lint-fix".to_string()),
            SkillResourceId("lint-fix/SKILL.md".to_string()),
        )],
        read_request_keys(&host_read_requests)
    );
    assert!(
        remote_read_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );

    let next_turn_store = ExtensionData::new("turn-2");
    let next_fragments = registry.turn_input_contributors()[0]
        .contribute(
            TurnInputContext {
                turn_id: "turn-2".to_string(),
                user_input: vec![UserInput::Text {
                    text: "no skill this time".to_string(),
                    text_elements: Vec::new(),
                }],
                environments: Vec::new(),
            },
            &session_store,
            &thread_store,
            &next_turn_store,
        )
        .await;

    assert_eq!(1, next_fragments.len());
    assert_eq!("developer", next_fragments[0].role());
    assert!(next_fragments[0].render().contains("lint-fix"));

    Ok(())
}

#[tokio::test]
async fn deferred_skill_is_searchable_and_loadable_but_disabled_skill_is_not() -> TestResult {
    let read_requests = Arc::new(Mutex::new(Vec::new()));
    let deferred_entry = test_entry(
        SkillSourceKind::Host,
        "host",
        "host/deferred-skill",
        "deferred-skill/SKILL.md",
    )
    .deferred();
    let disabled_entry = test_entry(
        SkillSourceKind::Host,
        "host",
        "host/disabled-skill",
        "disabled-skill/SKILL.md",
    )
    .disabled();
    assert!(!deferred_entry.is_prompt_visible());
    assert!(deferred_entry.is_searchable());
    assert!(deferred_entry.is_explicitly_loadable());
    assert!(!disabled_entry.is_prompt_visible());
    assert!(!disabled_entry.is_searchable());
    assert!(!disabled_entry.is_explicitly_loadable());

    let provider = Arc::new(StaticSkillProvider {
        catalog: SkillCatalog {
            entries: vec![
                test_entry(
                    SkillSourceKind::Host,
                    "host",
                    "host/visible-skill",
                    "visible-skill/SKILL.md",
                ),
                deferred_entry,
                disabled_entry,
            ],
            warnings: Vec::new(),
        },
        read_requests: Arc::clone(&read_requests),
    });
    let providers = SkillProviders::new().with_host_provider(provider);
    let mut builder = ExtensionRegistryBuilder::new();
    install_with_providers(&mut builder, providers);
    let registry = builder.build();
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");
    let session_source = SessionSource::Cli;
    let config = default_config().await?;
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &session_source,
            persistent_thread_state_available: true,
            session_store: &session_store,
            thread_store: &thread_store,
        })
        .await;

    let fragments = registry.turn_input_contributors()[0]
        .contribute(
            TurnInputContext {
                turn_id: "turn-1".to_string(),
                user_input: vec![UserInput::Text {
                    text: "$deferred-skill $disabled-skill".to_string(),
                    text_elements: Vec::new(),
                }],
                environments: Vec::new(),
            },
            &session_store,
            &thread_store,
            &ExtensionData::new("turn-1"),
        )
        .await;

    assert_eq!(2, fragments.len());
    let catalog_fragment = fragments[0].render();
    assert!(catalog_fragment.contains("visible-skill"));
    assert!(!catalog_fragment.contains("deferred-skill"));
    assert!(!catalog_fragment.contains("disabled-skill"));
    assert!(
        fragments[1]
            .render()
            .contains("<name>deferred-skill</name>")
    );
    assert!(!fragments[1].render().contains("disabled-skill"));
    assert_eq!(
        vec![(
            SkillAuthority::new(SkillSourceKind::Host, "host"),
            SkillPackageId("host/deferred-skill".to_string()),
            SkillResourceId("deferred-skill/SKILL.md".to_string()),
        )],
        read_request_keys(&read_requests)
    );

    Ok(())
}

#[tokio::test]
async fn model_tools_route_exact_packages_and_bound_results() -> TestResult {
    let first_provider = Arc::new(ToolSkillProvider::new(
        test_entry(
            SkillSourceKind::Remote,
            "catalog-a",
            "package-a",
            "resource-a/SKILL.md",
        ),
        "first provider contents",
        vec![SkillSearchMatch {
            resource: SkillResourceId("resource-a/reference.md".to_string()),
            title: "first".to_string(),
            snippet: "first".to_string(),
        }],
    ));
    let second_matches = (0..30)
        .map(|index| SkillSearchMatch {
            resource: SkillResourceId(format!("resource-b/reference-{index}.md")),
            title: format!("Reference {index}"),
            snippet: "large \\\"snippet\\\" ".repeat(300),
        })
        .collect();
    let second_provider = Arc::new(ToolSkillProvider::new(
        test_entry(
            SkillSourceKind::Remote,
            "catalog-b",
            "package-b",
            "resource-b/SKILL.md",
        ),
        &"large \\\"contents\\\" ".repeat(4_000),
        second_matches,
    ));
    let custom_provider = Arc::new(ToolSkillProvider::new(
        test_entry(
            SkillSourceKind::custom("host"),
            "custom-catalog",
            "custom-package",
            "custom/SKILL.md",
        ),
        "custom provider contents",
        vec![SkillSearchMatch {
            resource: SkillResourceId("custom/reference.md".to_string()),
            title: "custom".to_string(),
            snippet: "custom".to_string(),
        }],
    ));
    let invalid_provider = Arc::new(ToolSkillProvider::new(
        test_entry(
            SkillSourceKind::Remote,
            "invalid-catalog",
            "invalid-package",
            "invalid/SKILL.md",
        ),
        "invalid provider contents",
        (0..101)
            .map(|_| SkillSearchMatch {
                resource: SkillResourceId("x".repeat(2_049)),
                title: "invalid".to_string(),
                snippet: "invalid".to_string(),
            })
            .collect(),
    ));
    let providers = SkillProviders::new()
        .with_remote_provider(first_provider.clone())
        .with_remote_provider(second_provider.clone())
        .with_provider(SkillProviderSource::new(
            SkillSourceKind::custom("host"),
            "custom",
            custom_provider.clone(),
        ))
        .with_remote_provider(invalid_provider);
    let mut builder = ExtensionRegistryBuilder::new();
    install_with_providers(&mut builder, providers);
    let registry = builder.build();
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");
    let session_source = SessionSource::Cli;
    let config = default_config().await?;
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &session_source,
            persistent_thread_state_available: true,
            session_store: &session_store,
            thread_store: &thread_store,
        })
        .await;
    let turn_store = ExtensionData::new("turn-tools");
    let fragments = registry.turn_input_contributors()[0]
        .contribute(
            TurnInputContext {
                turn_id: "turn-tools".to_string(),
                user_input: vec![UserInput::Text {
                    text: "inspect the package".to_string(),
                    text_elements: Vec::new(),
                }],
                environments: Vec::new(),
            },
            &session_store,
            &thread_store,
            &turn_store,
        )
        .await;
    assert!(fragments[0].render().contains(
        r#"authority: {"kind":{"type":"custom","value":"host"},"id":"custom-catalog"}; package: "custom-package""#
    ));

    let tools = registry.tool_contributors()[0].tools(&session_store, &thread_store);
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.tool_name())
            .collect::<Vec<_>>(),
        vec![
            ToolName::namespaced("skills", "search"),
            ToolName::namespaced("skills", "read"),
        ]
    );
    for tool in &tools {
        assert_eq!(tool.exposure(), codex_tools::ToolExposure::DirectModelOnly);
        assert!(tool.supports_parallel_tool_calls());
        let codex_extension_api::ToolSpec::Namespace(spec) = tool.spec() else {
            panic!("skill model tools should share a namespace");
        };
        assert_eq!(spec.name, "skills");
        assert_eq!(spec.tools.len(), 1);
    }

    let custom_output = call_tool(
        Arc::clone(&tools[0]),
        "turn-tools",
        json!({
            "authority": {
                "kind": { "type": "custom", "value": "host" },
                "id": "custom-catalog"
            },
            "package": "custom-package",
            "query": "custom reference",
        }),
    )
    .await?;
    assert_eq!(
        custom_output["matches"][0]["resource"],
        "custom/reference.md"
    );
    assert_eq!(
        custom_provider.search_requests(),
        vec![SkillSearchRequest {
            authority: SkillAuthority::new(SkillSourceKind::custom("host"), "custom-catalog"),
            package: SkillPackageId("custom-package".to_string()),
            query: "custom reference".to_string(),
        }]
    );

    let provider_search_error = call_tool(
        Arc::clone(&tools[0]),
        "turn-tools",
        json!({
            "authority": { "kind": { "type": "remote" }, "id": "catalog-b" },
            "package": "package-b",
            "query": "provider-error",
        }),
    )
    .await;
    assert_eq!(
        provider_search_error,
        Err(FunctionCallError::RespondToModel(
            "skill provider could not search the requested package".to_string()
        ))
    );

    let provider_read_error = call_tool(
        Arc::clone(&tools[1]),
        "turn-tools",
        json!({
            "authority": { "kind": { "type": "remote" }, "id": "catalog-b" },
            "package": "package-b",
            "resource": "provider-error",
        }),
    )
    .await;
    assert_eq!(
        provider_read_error,
        Err(FunctionCallError::RespondToModel(
            "skill provider could not read the requested resource".to_string()
        ))
    );

    let invalid_flood = call_tool(
        Arc::clone(&tools[0]),
        "turn-tools",
        json!({
            "authority": { "kind": { "type": "remote" }, "id": "invalid-catalog" },
            "package": "invalid-package",
            "query": "invalid resources",
        }),
    )
    .await?;
    assert_eq!(invalid_flood["matches"], json!([]));
    assert_eq!(invalid_flood["truncated"], true);

    let oversized_arguments = call_tool(
        Arc::clone(&tools[0]),
        "turn-tools",
        json!({
            "authority": { "kind": { "type": "remote" }, "id": "catalog-b" },
            "package": "package-b",
            "query": "x".repeat(17 * 1024),
        }),
    )
    .await;
    assert_eq!(
        oversized_arguments,
        Err(FunctionCallError::RespondToModel(
            "skill tool arguments must be at most 16384 bytes".to_string()
        ))
    );

    let wrong_resource = call_tool(
        Arc::clone(&tools[1]),
        "turn-tools",
        json!({
            "authority": { "kind": { "type": "remote" }, "id": "catalog-b" },
            "package": "package-b",
            "resource": "mismatch",
        }),
    )
    .await;
    assert_eq!(
        wrong_resource,
        Err(FunctionCallError::Fatal(
            "skill provider returned a different resource".to_string()
        ))
    );

    let search_output = call_tool(
        Arc::clone(&tools[0]),
        "turn-tools",
        json!({
            "authority": { "kind": { "type": "remote" }, "id": "catalog-b" },
            "package": "package-b",
            "query": "deployment references",
        }),
    )
    .await?;
    assert_eq!(
        search_output["authority"],
        json!({ "kind": { "type": "remote" }, "id": "catalog-b" })
    );
    assert_eq!(search_output["package"], "package-b");
    assert_eq!(search_output["truncated"], true);
    assert!(
        search_output["matches"]
            .as_array()
            .is_some_and(|matches| !matches.is_empty() && matches.len() <= 20)
    );
    assert!(serde_json::to_vec(&search_output)?.len() <= 32 * 1024);
    assert!(first_provider.search_requests().is_empty());
    assert_eq!(
        second_provider.search_requests(),
        vec![
            SkillSearchRequest {
                authority: SkillAuthority::new(SkillSourceKind::Remote, "catalog-b"),
                package: SkillPackageId("package-b".to_string()),
                query: "provider-error".to_string(),
            },
            SkillSearchRequest {
                authority: SkillAuthority::new(SkillSourceKind::Remote, "catalog-b"),
                package: SkillPackageId("package-b".to_string()),
                query: "deployment references".to_string(),
            },
        ]
    );

    let read_output = call_tool(
        Arc::clone(&tools[1]),
        "turn-tools",
        json!({
            "authority": { "kind": { "type": "remote" }, "id": "catalog-b" },
            "package": "package-b",
            "resource": "resource-b/reference-0.md",
        }),
    )
    .await?;
    assert_eq!(read_output["resource"], "resource-b/reference-0.md");
    assert_eq!(read_output["truncated"], true);
    assert!(serde_json::to_vec(&read_output)?.len() <= 32 * 1024);
    assert!(first_provider.read_requests().is_empty());
    assert_eq!(
        read_request_keys(&second_provider.read_requests),
        vec![
            (
                SkillAuthority::new(SkillSourceKind::Remote, "catalog-b"),
                SkillPackageId("package-b".to_string()),
                SkillResourceId("provider-error".to_string()),
            ),
            (
                SkillAuthority::new(SkillSourceKind::Remote, "catalog-b"),
                SkillPackageId("package-b".to_string()),
                SkillResourceId("mismatch".to_string()),
            ),
            (
                SkillAuthority::new(SkillSourceKind::Remote, "catalog-b"),
                SkillPackageId("package-b".to_string()),
                SkillResourceId("resource-b/reference-0.md".to_string()),
            ),
        ]
    );

    let unavailable = call_tool(
        Arc::clone(&tools[0]),
        "turn-tools",
        json!({
            "authority": { "kind": { "type": "remote" }, "id": "catalog-a" },
            "package": "package-b",
            "query": "wrong authority",
        }),
    )
    .await;
    assert_eq!(
        unavailable,
        Err(FunctionCallError::RespondToModel(
            "skill package is not available from the requested authority in this turn".to_string()
        ))
    );

    registry.turn_lifecycle_contributors()[0]
        .on_turn_stop(TurnStopInput {
            session_store: &session_store,
            thread_store: &thread_store,
            turn_store: &turn_store,
        })
        .await;
    let stale_turn = call_tool(
        Arc::clone(&tools[0]),
        "turn-tools",
        json!({
            "authority": { "kind": { "type": "remote" }, "id": "catalog-b" },
            "package": "package-b",
            "query": "stale turn",
        }),
    )
    .await;
    assert_eq!(
        stale_turn,
        Err(FunctionCallError::RespondToModel(
            "skill resources are unavailable because the current turn catalog is not loaded"
                .to_string()
        ))
    );

    Ok(())
}

#[derive(Clone)]
struct ToolSkillProvider {
    catalog: SkillCatalog,
    read_requests: Arc<Mutex<Vec<SkillReadRequest>>>,
    search_requests: Arc<Mutex<Vec<SkillSearchRequest>>>,
    read_contents: String,
    search_matches: Vec<SkillSearchMatch>,
}

impl ToolSkillProvider {
    fn new(
        entry: SkillCatalogEntry,
        read_contents: &str,
        search_matches: Vec<SkillSearchMatch>,
    ) -> Self {
        Self {
            catalog: SkillCatalog {
                entries: vec![entry],
                warnings: Vec::new(),
            },
            read_requests: Arc::new(Mutex::new(Vec::new())),
            search_requests: Arc::new(Mutex::new(Vec::new())),
            read_contents: read_contents.to_string(),
            search_matches,
        }
    }

    fn read_requests(&self) -> Vec<SkillReadRequest> {
        self.read_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn search_requests(&self) -> Vec<SkillSearchRequest> {
        self.search_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl SkillProvider for ToolSkillProvider {
    fn list(&self, _query: SkillListQuery) -> SkillProviderFuture<'_, SkillCatalog> {
        let catalog = self.catalog.clone();
        Box::pin(async move { Ok(catalog) })
    }

    fn read(&self, request: SkillReadRequest) -> SkillProviderFuture<'_, SkillReadResult> {
        let read_requests = Arc::clone(&self.read_requests);
        let contents = self.read_contents.clone();
        Box::pin(async move {
            read_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.clone());
            if request.resource.0 == "provider-error" {
                return Err(SkillProviderError::new("provider error ".repeat(10_000)));
            }
            let resource = if request.resource.0 == "mismatch" {
                SkillResourceId("different-resource".to_string())
            } else {
                request.resource
            };
            Ok(SkillReadResult { resource, contents })
        })
    }

    fn search(&self, request: SkillSearchRequest) -> SkillProviderFuture<'_, SkillSearchResult> {
        let search_requests = Arc::clone(&self.search_requests);
        let matches = self.search_matches.clone();
        Box::pin(async move {
            search_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.clone());
            if request.query == "provider-error" {
                return Err(SkillProviderError::new("provider error ".repeat(10_000)));
            }
            Ok(SkillSearchResult { matches })
        })
    }
}

#[derive(Clone)]
struct StaticSkillProvider {
    catalog: SkillCatalog,
    read_requests: Arc<Mutex<Vec<SkillReadRequest>>>,
}

impl SkillProvider for StaticSkillProvider {
    fn list(&self, query: SkillListQuery) -> SkillProviderFuture<'_, SkillCatalog> {
        let catalog = self.catalog.clone();
        Box::pin(async move {
            assert!(query.include_host_skills);
            assert!(query.include_bundled_skills);
            Ok(catalog)
        })
    }

    fn read(&self, request: SkillReadRequest) -> SkillProviderFuture<'_, SkillReadResult> {
        let read_requests = Arc::clone(&self.read_requests);
        Box::pin(async move {
            read_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.clone());
            Ok(SkillReadResult {
                resource: request.resource,
                contents: "# Lint Fix\n\nRun the formatter.".to_string(),
            })
        })
    }

    fn search(&self, _request: SkillSearchRequest) -> SkillProviderFuture<'_, SkillSearchResult> {
        Box::pin(async { Ok(SkillSearchResult::default()) })
    }
}

fn find_tool(
    registry: &ExtensionRegistry<Config>,
    session_store: &ExtensionData,
    thread_store: &ExtensionData,
    tool_name: ToolName,
) -> Arc<dyn ToolExecutor<ToolCall>> {
    registry.tool_contributors()[0]
        .tools(session_store, thread_store)
        .into_iter()
        .find(|tool| tool.tool_name() == tool_name)
        .unwrap_or_else(|| panic!("{tool_name} should be registered"))
}

async fn call_tool(
    tool: Arc<dyn ToolExecutor<ToolCall>>,
    turn_id: &str,
    arguments: Value,
) -> Result<Value, FunctionCallError> {
    let payload = ToolPayload::Function {
        arguments: arguments.to_string(),
    };
    let output = tool
        .handle(ToolCall {
            turn_id: turn_id.to_string(),
            call_id: "call-skill".to_string(),
            tool_name: tool.tool_name(),
            model: "test-model".to_string(),
            truncation_policy: TruncationPolicy::Bytes(1024),
            conversation_history: codex_extension_api::ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            payload: payload.clone(),
        })
        .await?;
    assert_eq!(output.log_preview(), "[skill resource output]");
    Ok(output.code_mode_result(&payload))
}

fn test_entry(
    kind: SkillSourceKind,
    authority_id: &str,
    package_id: &str,
    main_prompt: &str,
) -> SkillCatalogEntry {
    let name = package_id.rsplit('/').next().unwrap_or(package_id);
    SkillCatalogEntry::new(
        SkillPackageId(package_id.to_string()),
        SkillAuthority::new(kind, authority_id),
        name,
        "Fix lint errors.",
        SkillResourceId(main_prompt.to_string()),
    )
    .with_display_path(format!("skill://{package_id}/SKILL.md"))
}

async fn default_config() -> std::io::Result<Config> {
    let codex_home = test_codex_home();
    std::fs::create_dir_all(&codex_home)?;
    let config =
        Config::load_default_with_cli_overrides_for_codex_home(codex_home.clone(), vec![]).await?;
    std::fs::remove_dir_all(codex_home)?;
    Ok(config)
}

fn test_codex_home() -> PathBuf {
    let id = NEXT_CODEX_HOME_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "codex-skills-extension-test-{}-{id}",
        std::process::id(),
    ))
}

fn read_request_keys(
    requests: &Arc<Mutex<Vec<SkillReadRequest>>>,
) -> Vec<(SkillAuthority, SkillPackageId, SkillResourceId)> {
    requests
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|request| {
            (
                request.authority.clone(),
                request.package.clone(),
                request.resource.clone(),
            )
        })
        .collect()
}

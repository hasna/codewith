use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use codex_core::config::Config;
use codex_core::config::ConfigBuilder;
use codex_core_skills::HostLoadedSkills;
use codex_core_skills::SkillLoadOutcome;
use codex_core_skills::SkillMetadata;
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
    // Core renders the `## Skills` developer block from the host outcome; the
    // preamble may only be dropped from the task-relevant fragment when that
    // block actually exists.
    turn_store.insert(host_loaded_skills("lint-fix"));
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

    // A follow-up turn that still matches the catalog re-injects the ranked
    // list (without re-injecting the skill body, which the turn-1 entrypoint
    // already covered).
    let matching_turn_store = ExtensionData::new("turn-2");
    matching_turn_store.insert(host_loaded_skills("lint-fix"));
    let matching_fragments = registry.turn_input_contributors()[0]
        .contribute(
            TurnInputContext {
                turn_id: "turn-2".to_string(),
                user_input: vec![UserInput::Text {
                    text: "keep fixing the lint errors".to_string(),
                    text_elements: Vec::new(),
                }],
                environments: Vec::new(),
            },
            &session_store,
            &thread_store,
            &matching_turn_store,
        )
        .await;

    assert_eq!(1, matching_fragments.len());
    assert_eq!("developer", matching_fragments[0].role());
    let matching_body = matching_fragments[0].render();
    assert!(matching_body.starts_with(SKILLS_INSTRUCTIONS_OPEN_TAG));
    assert!(matching_body.contains("lint-fix"));
    // The `### How to use skills` preamble belongs to the developer-message
    // `## Skills` block; repeating it here would duplicate ~2.5k characters on
    // every matching turn.
    assert!(matching_body.contains(codex_core_skills::TASK_RELEVANT_SKILLS_HEADING));
    assert!(!matching_body.contains("### How to use skills"));
    assert!(matching_body.contains(codex_core_skills::TASK_RELEVANT_SKILLS_INTRO));
    assert!(!matching_body.contains(codex_core_skills::SKILLS_HOW_TO_USE_TAIL_WITH_ABSOLUTE_PATHS));

    // Turns whose text shares no token with any skill contribute nothing. Note
    // that this also swallows short continuations ("continue", "yes") because
    // the ranker is purely lexical -- see the recall tests in `ranking.rs`.
    let unrelated_turn_store = ExtensionData::new("turn-3");
    let unrelated_fragments = registry.turn_input_contributors()[0]
        .contribute(
            TurnInputContext {
                turn_id: "turn-3".to_string(),
                user_input: vec![UserInput::Text {
                    text: "no relevant request".to_string(),
                    text_elements: Vec::new(),
                }],
                environments: Vec::new(),
            },
            &session_store,
            &thread_store,
            &unrelated_turn_store,
        )
        .await;

    assert!(unrelated_fragments.is_empty());

    Ok(())
}

#[tokio::test]
async fn skills_list_pages_past_the_default_limit() -> TestResult {
    // The starter list only ever shows a handful of skills, so the model must
    // be able to walk the whole ranked catalog through `skills.list`. A `limit`
    // that can only shrink the page, or a page with no continuation, would
    // leave matches 6..N unreachable.
    const TOTAL: usize = 24;
    let entries = (0..TOTAL)
        .map(|index| {
            test_entry(
                SkillSourceKind::Host,
                "host",
                &format!("host/lint-{index:02}"),
                &format!("lint-{index:02}/SKILL.md"),
            )
        })
        .collect::<Vec<_>>();
    let provider = Arc::new(StaticSkillProvider {
        catalog: SkillCatalog {
            entries,
            warnings: Vec::new(),
        },
        read_requests: Arc::new(Mutex::new(Vec::new())),
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
    let turn_store = ExtensionData::new("turn-page");
    registry.turn_input_contributors()[0]
        .contribute(
            TurnInputContext {
                turn_id: "turn-page".to_string(),
                user_input: vec![UserInput::Text {
                    text: "fix lint errors".to_string(),
                    text_elements: Vec::new(),
                }],
                environments: Vec::new(),
            },
            &session_store,
            &thread_store,
            &turn_store,
        )
        .await;

    let list_tool = find_tool(
        &registry,
        &session_store,
        &thread_store,
        ToolName::namespaced("skills", "list"),
    );

    // Default page: five matches, explicitly continuable.
    let first = call_tool(
        Arc::clone(&list_tool),
        "turn-page",
        json!({ "query": "lint" }),
    )
    .await?;
    let first_names = match_names(&first);
    assert_eq!(first_names.len(), 5);
    // `truncated` is about this page's content, `has_more` about the walk.
    assert_eq!(first["truncated"], json!(false));
    assert_eq!(first["has_more"], json!(true));
    assert_eq!(first["total_matches"], json!(TOTAL));
    assert_eq!(first["next_offset"], json!(5));

    // Walk the remainder with the returned cursor; every skill is reachable.
    let mut seen = first_names;
    let mut offset = first["next_offset"].as_u64().expect("cursor");
    loop {
        let page = call_tool(
            Arc::clone(&list_tool),
            "turn-page",
            json!({ "query": "lint", "offset": offset }),
        )
        .await?;
        seen.extend(match_names(&page));
        match page["next_offset"].as_u64() {
            Some(next) => {
                assert!(next > offset, "cursor must advance: {page}");
                offset = next;
            }
            None => break,
        }
    }
    assert_eq!(seen.len(), TOTAL);
    assert_eq!(
        seen.iter().collect::<std::collections::HashSet<_>>().len(),
        TOTAL
    );

    // `limit` can widen the page, not just narrow it.
    let wide = call_tool(
        Arc::clone(&list_tool),
        "turn-page",
        json!({ "query": "lint", "limit": 50 }),
    )
    .await?;
    assert_eq!(match_names(&wide).len(), TOTAL);
    assert_eq!(wide["next_offset"], Value::Null);
    assert_eq!(wide["has_more"], json!(false));
    assert_eq!(wide["truncated"], json!(false));

    // Paging past the end is well-formed and terminates.
    let past_end = call_tool(
        Arc::clone(&list_tool),
        "turn-page",
        json!({ "query": "lint", "offset": 100 }),
    )
    .await?;
    assert_eq!(past_end["matches"], json!([]));
    assert_eq!(past_end["next_offset"], Value::Null);

    Ok(())
}

#[tokio::test]
async fn skills_list_reaches_every_entry_of_a_large_catalog_and_enumerates_unguessable_ones()
-> TestResult {
    // The three properties this tool has to hold at "thousands of skills"
    // scale:
    //   1. every rank is reachable by walking the cursor,
    //   2. a cursor the tool emits is never a cursor the tool rejects,
    //   3. a skill nobody can guess a query term for is still reachable.
    const TOTAL: usize = 2_000;
    let mut entries = (0..TOTAL)
        .map(|index| {
            SkillCatalogEntry::new(
                SkillPackageId(format!("host/skill-{index:04}")),
                SkillAuthority::new(SkillSourceKind::Host, "host"),
                format!("skill-{index:04}"),
                "Ship a deploy to production.",
                SkillResourceId(format!("skill-{index:04}/SKILL.md")),
            )
        })
        .collect::<Vec<_>>();
    entries.push(SkillCatalogEntry::new(
        SkillPackageId("host/kanban-groomer".to_string()),
        SkillAuthority::new(SkillSourceKind::Host, "host"),
        "kanban-groomer",
        "Reprioritise the backlog board",
        SkillResourceId("kanban-groomer/SKILL.md".to_string()),
    ));
    let provider = Arc::new(StaticSkillProvider {
        catalog: SkillCatalog {
            entries,
            warnings: Vec::new(),
        },
        read_requests: Arc::new(Mutex::new(Vec::new())),
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
    let turn_store = ExtensionData::new("turn-scale");
    registry.turn_input_contributors()[0]
        .contribute(
            TurnInputContext {
                turn_id: "turn-scale".to_string(),
                user_input: vec![UserInput::Text {
                    text: "hello".to_string(),
                    text_elements: Vec::new(),
                }],
                environments: Vec::new(),
            },
            &session_store,
            &thread_store,
            &turn_store,
        )
        .await;
    let list_tool = find_tool(
        &registry,
        &session_store,
        &thread_store,
        ToolName::namespaced("skills", "list"),
    );

    // (1)+(2): walk the whole ranked result set with the cursor the tool hands
    // back, never adjusting it.
    let mut seen = std::collections::HashSet::new();
    let mut offset = 0u64;
    let mut calls = 0usize;
    loop {
        let page = call_tool(
            Arc::clone(&list_tool),
            "turn-scale",
            json!({ "query": "deploy", "limit": 50, "offset": offset }),
        )
        .await?;
        calls += 1;
        assert!(calls < 200, "cursor walk should terminate");
        assert_eq!(page["total_matches"], json!(TOTAL));
        seen.extend(match_names(&page));
        match page["next_offset"].as_u64() {
            Some(next) => {
                assert!(next > offset, "cursor must strictly advance: {page}");
                assert_eq!(page["has_more"], json!(true));
                offset = next;
            }
            None => {
                assert_eq!(page["has_more"], json!(false));
                break;
            }
        }
    }
    assert_eq!(seen.len(), TOTAL);
    assert!(seen.contains("skill-1999"), "the tail must be reachable");

    // (3): no query term reaches the odd-one-out, but enumeration does.
    for query in ["tidy up my tickets", "tickets", "skill", "the", "*"] {
        let page = call_tool(
            Arc::clone(&list_tool),
            "turn-scale",
            json!({ "query": query, "limit": 50 }),
        )
        .await?;
        assert!(
            !match_names(&page).contains(&"kanban-groomer".to_string()),
            "{query} unexpectedly matched"
        );
    }
    let mut enumerated = Vec::new();
    let mut offset = 0u64;
    loop {
        // Both spellings of "no query" must work: omitted and blank.
        let arguments = if offset == 0 {
            json!({ "limit": 50 })
        } else {
            json!({ "query": "", "limit": 50, "offset": offset })
        };
        let page = call_tool(Arc::clone(&list_tool), "turn-scale", arguments).await?;
        assert_eq!(page["total_matches"], json!(TOTAL + 1));
        enumerated.extend(match_names(&page));
        match page["next_offset"].as_u64() {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(enumerated.len(), TOTAL + 1);
    assert!(enumerated.contains(&"kanban-groomer".to_string()));
    assert!(
        enumerated.windows(2).all(|pair| pair[0] <= pair[1]),
        "enumeration must be alphabetical"
    );

    // A deep offset is answered, not rejected.
    let deep = call_tool(
        Arc::clone(&list_tool),
        "turn-scale",
        json!({ "query": "deploy", "limit": 50, "offset": 1_000 }),
    )
    .await?;
    let deep_cursor = deep["next_offset"].as_u64().expect("cursor past rank 1000");
    let after_deep = call_tool(
        Arc::clone(&list_tool),
        "turn-scale",
        json!({ "query": "deploy", "limit": 50, "offset": deep_cursor }),
    )
    .await?;
    assert_eq!(match_names(&after_deep).len(), 50);

    Ok(())
}

#[tokio::test]
async fn task_relevant_fragment_carries_the_rules_when_no_host_skills_block_exists() -> TestResult {
    // Core builds `## Skills` from the host outcome. With an empty host outcome
    // and a remote-only catalog there is no such section, so the per-turn
    // fragment must not point at one - it has to carry the rules itself.
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
        read_requests: Arc::new(Mutex::new(Vec::new())),
    });
    let providers = SkillProviders::new().with_remote_provider(remote_provider);
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

    let turn_store = ExtensionData::new("turn-remote-only");
    let fragments = registry.turn_input_contributors()[0]
        .contribute(
            TurnInputContext {
                turn_id: "turn-remote-only".to_string(),
                user_input: vec![UserInput::Text {
                    text: "fix the lint errors".to_string(),
                    text_elements: Vec::new(),
                }],
                environments: Vec::new(),
            },
            &session_store,
            &thread_store,
            &turn_store,
        )
        .await;

    assert_eq!(1, fragments.len());
    let body = fragments[0].render();
    assert!(body.contains("lint-fix"));
    assert!(!body.contains(codex_core_skills::TASK_RELEVANT_SKILLS_INTRO));
    assert!(body.contains(codex_core_skills::TASK_RELEVANT_SKILLS_STANDALONE_INTRO));
    assert!(body.contains("### How to use skills"));
    assert!(body.contains(codex_core_skills::SKILLS_DISCOVERY_CATALOG_SEARCH));
    assert!(body.contains(codex_core_skills::SKILLS_TRIGGER_RULES));
    assert!(body.contains(codex_core_skills::SKILLS_HOW_TO_USE_TAIL_WITH_ABSOLUTE_PATHS));

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

    assert_eq!(1, fragments.len());
    assert!(
        fragments[0]
            .render()
            .contains("<name>deferred-skill</name>")
    );
    assert!(!fragments[0].render().contains("disabled-skill"));
    assert_eq!(
        vec![(
            SkillAuthority::new(SkillSourceKind::Host, "host"),
            SkillPackageId("host/deferred-skill".to_string()),
            SkillResourceId("deferred-skill/SKILL.md".to_string()),
        )],
        read_request_keys(&read_requests)
    );

    let list_tool = find_tool(
        &registry,
        &session_store,
        &thread_store,
        ToolName::namespaced("skills", "list"),
    );
    let list_output = call_tool(
        list_tool,
        "turn-1",
        json!({ "query": "lint errors", "limit": 5 }),
    )
    .await?;
    assert_eq!(list_output["matches"][0]["name"], "visible-skill");
    assert!(
        list_output["matches"]
            .as_array()
            .is_some_and(|matches| matches.len() == 1)
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
                    text: "inspect the custom package".to_string(),
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
            ToolName::namespaced("skills", "list"),
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

    let list_tool = find_tool(
        &registry,
        &session_store,
        &thread_store,
        ToolName::namespaced("skills", "list"),
    );
    let search_tool = find_tool(
        &registry,
        &session_store,
        &thread_store,
        ToolName::namespaced("skills", "search"),
    );
    let read_tool = find_tool(
        &registry,
        &session_store,
        &thread_store,
        ToolName::namespaced("skills", "read"),
    );
    let catalog_output = call_tool(
        Arc::clone(&list_tool),
        "turn-tools",
        json!({ "query": "custom package", "limit": 5 }),
    )
    .await?;
    assert_eq!(catalog_output["matches"][0]["name"], "custom-package");
    assert_eq!(
        catalog_output["matches"][0]["authority"],
        json!({
            "kind": { "type": "custom", "value": "host" },
            "id": "custom-catalog"
        })
    );
    assert_eq!(catalog_output["matches"][0]["package"], "custom-package");
    assert_eq!(
        catalog_output["matches"][0]["main_resource"],
        "custom/SKILL.md"
    );
    let default_list_output = call_tool(
        Arc::clone(&list_tool),
        "turn-tools",
        json!({ "query": "package" }),
    )
    .await?;
    assert!(
        default_list_output["matches"]
            .as_array()
            .is_some_and(|matches| matches.len() <= 5)
    );
    let oversized_list_query = call_tool(
        Arc::clone(&list_tool),
        "turn-tools",
        json!({ "query": "x".repeat(4_097) }),
    )
    .await;
    assert_eq!(
        oversized_list_query,
        Err(FunctionCallError::RespondToModel(
            "query must contain no control characters and be at most 4096 bytes; omit it entirely to enumerate the whole catalog"
                .to_string()
        ))
    );
    let oversized_list_limit = call_tool(
        Arc::clone(&list_tool),
        "turn-tools",
        json!({ "query": "package", "limit": 51 }),
    )
    .await;
    assert_eq!(
        oversized_list_limit,
        Err(FunctionCallError::RespondToModel(
            "limit must be between 1 and 50".to_string()
        ))
    );
    // A deep offset is answered with an empty final page, never rejected: the
    // tool must accept any cursor it could itself have produced.
    let deep_list_offset = call_tool(
        Arc::clone(&list_tool),
        "turn-tools",
        json!({ "query": "package", "offset": 1_001 }),
    )
    .await?;
    assert_eq!(deep_list_offset["matches"], json!([]));
    assert_eq!(deep_list_offset["has_more"], json!(false));
    assert_eq!(deep_list_offset["next_offset"], Value::Null);

    let custom_output = call_tool(
        Arc::clone(&search_tool),
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
        Arc::clone(&search_tool),
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
        Arc::clone(&read_tool),
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
        Arc::clone(&search_tool),
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
        Arc::clone(&search_tool),
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
        Arc::clone(&read_tool),
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
        Arc::clone(&search_tool),
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
        Arc::clone(&read_tool),
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
        Arc::clone(&search_tool),
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
        Arc::clone(&search_tool),
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

/// A host outcome carrying one implicitly-invocable skill, i.e. the state in
/// which core does render a `## Skills` developer block.
fn host_loaded_skills(name: &str) -> HostLoadedSkills {
    let path = codex_utils_absolute_path::AbsolutePathBuf::from_absolute_path(
        std::env::temp_dir().join(name).join("SKILL.md"),
    )
    .unwrap_or_else(|err| panic!("temp dir should be absolute: {err}"));
    let mut outcome = SkillLoadOutcome::default();
    outcome.skills.push(SkillMetadata {
        name: name.to_string(),
        description: "Fix lint errors.".to_string(),
        short_description: None,
        interface: None,
        dependencies: None,
        policy: None,
        path_to_skills_md: path,
        scope: codex_protocol::protocol::SkillScope::Repo,
        plugin_id: None,
    });
    HostLoadedSkills::new(Arc::new(outcome))
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

fn match_names(response: &Value) -> Vec<String> {
    response["matches"]
        .as_array()
        .unwrap_or_else(|| panic!("matches array in {response}"))
        .iter()
        .map(|entry| {
            entry["name"]
                .as_str()
                .unwrap_or_else(|| panic!("match name in {response}"))
                .to_string()
        })
        .collect()
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

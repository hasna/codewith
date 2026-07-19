use super::*;

use pretty_assertions::assert_eq;

// Immutable escaped-CRLF outputs from published cmd-shim tarballs:
// 5.0.0 gitHead c55b9a9a4cb3f321a9abad7d6d45e2aac2f82b08,
// integrity sha512-qkCtZ59BidfEwHltnJwkyVZn+XQojdAySM1D1gSeh11Z4pW1Kpolkyo53L5noc0nrxmIvyFwTmJRo4xs7FFLPw==;
// 9.0.2 gitHead 7667c245e7d9259b5f88b77fb71b497ffcc26976,
// integrity sha512-xVHoI+wNrM4tDB9iC1idf/8D0tYnVimlBp/5zHW+x1sGjjRD69NvR9th3Z1JAYGN/BTW4he6aZFYV6kzy+k+jw==.
const CMD_SHIM_V5_NODE_GOLDEN: &str = include_str!("fixtures/cmd-shim-5.0.0-node.cmd.golden");
const CMD_SHIM_V9_NODE_GOLDEN: &str = include_str!("fixtures/cmd-shim-9.0.2-node.cmd.golden");

fn released_cmd(golden: &str) -> String {
    golden
        .strip_suffix('\n')
        .unwrap_or(golden)
        .replace(r"\r\n", "\r\n")
}

fn v9_node_with_target(target: &str) -> String {
    released_cmd(CMD_SHIM_V9_NODE_GOLDEN).replace("node_modules\\agent\\bin\\agent.js", target)
}

// Exact cmd-shim@9.0.2 static direct-executable form.
fn npm_v9_direct(target: &str) -> String {
    format!(
        "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\{target}\"   %*\r\n"
    )
}

// Historical seven-line Corepack fallback, not cmd-shim@5.0.0 output.
fn legacy_seven_line_corepack_shim(target: &str) -> String {
    format!(
        "@SETLOCAL\r\n@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\"  \"%~dp0\\{target}\" %*\r\n) ELSE (\r\n  @SET PATHEXT=%PATHEXT:;.JS;=;%\r\n  node  \"%~dp0\\{target}\" %*\r\n)\r\n"
    )
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture");
    std::fs::write(path, contents).expect("write fixture");
}

fn local_bin(temp: &Path, name: &str) -> PathBuf {
    temp.join("node_modules/.bin").join(format!("{name}.cmd"))
}

#[test]
fn recognizes_exact_static_generators_and_rejects_rewrites() {
    let global_target = "node_modules\\agent\\bin\\agent.js";
    let local_target = "..\\dist\\corepack.js";
    let claude = "..\\@anthropic-ai\\claude-code\\bin\\claude.exe";
    assert_eq!(
        recognized_shim(&released_cmd(CMD_SHIM_V5_NODE_GOLDEN)),
        Some((global_target, ShimRuntime::Node))
    );
    assert_eq!(
        recognized_shim(&released_cmd(CMD_SHIM_V9_NODE_GOLDEN)),
        Some((global_target, ShimRuntime::Node))
    );
    assert_eq!(
        recognized_shim(&npm_v9_direct(claude)),
        Some((claude, ShimRuntime::DirectNative))
    );
    assert_eq!(
        recognized_shim(&legacy_seven_line_corepack_shim(local_target)),
        Some((local_target, ShimRuntime::Node))
    );
    assert_eq!(
        recognized_shim(&released_cmd(CMD_SHIM_V9_NODE_GOLDEN).replace("@ECHO off", "@ECHO off ")),
        None
    );
    assert_eq!(
        recognized_shim(
            &released_cmd(CMD_SHIM_V9_NODE_GOLDEN)
                .replace("\"%_prog%\"  ", "\"%_prog%\" --inspect ")
        ),
        None
    );
    assert_eq!(
        recognized_shim(
            &released_cmd(CMD_SHIM_V5_NODE_GOLDEN).replace("\r\n  SET PATHEXT", "\r\n SET PATHEXT")
        ),
        None
    );
    assert_eq!(
        recognized_shim(
            &legacy_seven_line_corepack_shim(local_target)
                .replace("  @SET PATHEXT", " @SET PATHEXT",)
        ),
        None
    );
}

#[test]
fn reparse_aliases_are_classified_by_their_canonical_batch_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let shim = local_bin(temp.path(), "claude");
    let alias = shim.with_extension("exe");
    let target = temp
        .path()
        .join("node_modules/@anthropic-ai/claude-code/bin/claude.exe");
    write(
        &shim,
        &npm_v9_direct("..\\@anthropic-ai\\claude-code\\bin\\claude.exe"),
    );
    write(&target, "fixture");
    std::os::windows::fs::symlink_file(&shim, &alias).expect("create alias");
    let args = vec![OsString::from("--resume"), OsString::from("task")];
    let launch = prepare_windows_batch_launch_from_source_env(
        alias,
        args.clone(),
        &BTreeMap::new(),
        temp.path(),
    )
    .expect("alias must use the canonical cmd target");
    assert_eq!(
        launch,
        WindowsNativeLaunch {
            program: target.canonicalize().expect("canonical target"),
            args
        }
    );
}

#[test]
fn published_cmd_shim_v5_node_fixture_resolves_bounded_prefix_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let shim = temp.path().join("prefix/agent.cmd");
    let target = temp.path().join("prefix/node_modules/agent/bin/agent.js");
    let node = temp.path().join("trusted-node/node.exe");
    write(&shim, &released_cmd(CMD_SHIM_V5_NODE_GOLDEN));
    write(&target, "fixture");
    write(&node, "fixture");
    let source_env = BTreeMap::from([(
        "PATH".into(),
        temp.path().join("trusted-node").display().to_string(),
    )]);
    let launch =
        prepare_windows_batch_launch_from_source_env(shim, Vec::new(), &source_env, temp.path())
            .expect("published cmd-shim@5.0.0 Node plan");
    assert_eq!(launch.program, node.canonicalize().expect("canonical node"));
    assert_eq!(
        launch.args,
        vec![
            target
                .canonicalize()
                .expect("canonical target")
                .into_os_string()
        ]
    );
}

#[test]
fn legacy_corepack_shim_resolves_bounded_parent_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let shim = local_bin(temp.path(), "corepack");
    let target = temp.path().join("node_modules/dist/corepack.js");
    let node = temp.path().join("trusted-node/node.exe");
    write(
        &shim,
        &legacy_seven_line_corepack_shim("..\\dist\\corepack.js"),
    );
    write(&target, "fixture");
    write(&node, "fixture");
    let source_env = BTreeMap::from([(
        "PATH".into(),
        temp.path().join("trusted-node").display().to_string(),
    )]);
    let launch =
        prepare_windows_batch_launch_from_source_env(shim, Vec::new(), &source_env, temp.path())
            .expect("legacy Corepack plan");
    assert_eq!(launch.program, node.canonicalize().expect("canonical node"));
    assert_eq!(
        launch.args,
        vec![
            target
                .canonicalize()
                .expect("canonical target")
                .into_os_string()
        ]
    );
}

#[test]
fn published_cmd_shim_v9_node_fixture_resolves_node_modules_and_keeps_os_argv() {
    let temp = tempfile::tempdir().expect("tempdir");
    let shim = temp.path().join("prefix/agent.cmd");
    let target = temp.path().join("prefix/node_modules/agent/bin/agent.js");
    let node = temp.path().join("trusted-node/node.EXE");
    write(&shim, &released_cmd(CMD_SHIM_V9_NODE_GOLDEN));
    write(&target, "fixture");
    write(&node, "fixture");
    let args = [
        "",
        "quotes: \"double\" and 'single'",
        "&|<>()^",
        "%PERCENT%",
        "!BANG!",
        "first\r\nsecond",
    ]
    .map(OsString::from)
    .to_vec();
    let source_env = BTreeMap::from([
        (
            "PATH".into(),
            temp.path().join("trusted-node").display().to_string(),
        ),
        (
            "COMSPEC".into(),
            temp.path().join("poisoned.cmd").display().to_string(),
        ),
    ]);
    let launch =
        prepare_windows_batch_launch_from_source_env(shim, args.clone(), &source_env, temp.path())
            .expect("source PATH plan");
    let mut expected = vec![
        target
            .canonicalize()
            .expect("canonical target")
            .into_os_string(),
    ];
    expected.extend(args);
    assert_eq!(launch.args, expected);
    assert_eq!(launch.program, node.canonicalize().expect("canonical node"));
    assert_ne!(launch.program, temp.path().join("poisoned.cmd"));
}

#[test]
fn rejects_relative_program_before_classifying_or_reading_the_shim() {
    let temp = tempfile::tempdir().expect("tempdir");
    let error = prepare_windows_batch_launch_from_source_env(
        PathBuf::from("agent.cmd"),
        Vec::new(),
        &BTreeMap::new(),
        temp.path(),
    )
    .expect_err("relative shim must not bind to the host current directory");
    assert!(matches!(error, WindowsBatchLaunchError::ProgramNotAbsolute));
}

#[test]
fn rejects_program_spellings_that_win32_normalizes_before_batch_classification() {
    let temp = tempfile::tempdir().expect("tempdir");
    for program in [
        temp.path().join("agent.cmd."),
        temp.path().join("agent.cmd "),
        temp.path().join("review.").join("agent.cmd"),
        temp.path().join("review ").join("agent.cmd"),
        temp.path().join("CoM¹.cmd"),
        temp.path().join("com².payload"),
        temp.path().join("COM³.cmd"),
        temp.path().join("lPt¹.cmd"),
        temp.path().join("LPT².payload"),
        temp.path().join("lpt³.cmd"),
    ] {
        let error = prepare_windows_batch_launch_from_source_env(
            program,
            Vec::new(),
            &BTreeMap::new(),
            temp.path(),
        )
        .expect_err("ambiguous program spelling must not bypass cmd-shim handling");
        assert!(matches!(
            error,
            WindowsBatchLaunchError::AmbiguousProgramPath
        ));
    }
    assert!(ambiguous_windows_program_path(Path::new(
        r"\\?\C:\review\agent.cmd"
    )));
    assert!(ambiguous_windows_program_path(Path::new(
        r"\\.\C:\review\agent.cmd"
    )));
}

#[test]
fn rejects_noncanonical_and_ambiguous_target_grammar_before_filesystem_access() {
    let temp = tempfile::tempdir().expect("tempdir");
    for target in [
        ".\\agent.js",
        "..\\..\\agent.js",
        "node_modules\\..\\agent.js",
        "node_modules\\.\\agent.js",
        "node_modules\\\\agent.js",
        "C:\\agent.js",
        "\\\\?\\C:\\agent.js",
        "node_modules\\CON.js",
        "node_modules\\CoM¹\\agent.js",
        "node_modules\\com².payload\\agent.js",
        "node_modules\\COM³\\agent.js",
        "node_modules\\lPt¹\\agent.js",
        "node_modules\\LPT².payload\\agent.js",
        "node_modules\\lpt³\\agent.js",
        "node_modules/agent.js",
        "node_modules\\%PROGRAMFILES%\\agent.js",
    ] {
        let shim = temp.path().join(format!("prefix/{}.cmd", target.len()));
        write(&shim, &v9_node_with_target(target));
        let error = prepare_windows_batch_launch_from_source_env(
            shim,
            Vec::new(),
            &BTreeMap::new(),
            temp.path(),
        )
        .expect_err("ambiguous target must fail before filesystem access");
        assert!(matches!(error, WindowsBatchLaunchError::InvalidShimTarget));
    }
}

#[test]
fn rejects_target_reparse_escape_after_canonicalization() {
    let temp = tempfile::tempdir().expect("tempdir");
    let shim = local_bin(temp.path(), "agent");
    let outside = temp.path().join("outside.js");
    let linked = temp.path().join("node_modules/agent/bin/agent.js");
    write(&shim, &v9_node_with_target("..\\agent\\bin\\agent.js"));
    write(&outside, "fixture");
    std::fs::create_dir_all(linked.parent().expect("linked parent")).expect("create linked parent");
    std::os::windows::fs::symlink_file(&outside, &linked).expect("create fixture symlink");

    let error = prepare_windows_batch_launch_from_source_env(
        shim,
        Vec::new(),
        &BTreeMap::new(),
        temp.path(),
    )
    .expect_err("symlink target outside node_modules must fail");
    assert!(matches!(
        error,
        WindowsBatchLaunchError::TargetEscapesShimRoot { .. }
    ));
}

#[test]
fn absolute_non_batch_programs_remain_native_after_canonicalization() {
    let temp = tempfile::tempdir().expect("tempdir");
    let program = temp.path().join("agent.exe");
    let args = vec![OsString::from("--native")];
    write(&program, "fixture");
    assert_eq!(
        prepare_windows_batch_launch_from_source_env(
            program.clone(),
            args.clone(),
            &BTreeMap::new(),
            temp.path(),
        )
        .expect("non-batch program"),
        WindowsNativeLaunch {
            program: program.canonicalize().expect("canonical program"),
            args
        }
    );
}

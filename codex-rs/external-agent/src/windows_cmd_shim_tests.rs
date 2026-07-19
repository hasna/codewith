use super::*;

use pretty_assertions::assert_eq;

fn npm_v8(target: &str) -> String {
    format!(
        "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\{target}\" %*\r\n"
    )
}

// Exact cmd-shim@9.0.2 static Node form: no shebang arguments or environment-variable target.
fn npm_v9_node(target: &str) -> String {
    format!(
        "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & set PATHEXT=%PATHEXT:;.JS;=;% & \"%_prog%\"  \"%dp0%\\{target}\" %*\r\n"
    )
}

// Exact cmd-shim@9.0.2 static direct-executable form.
fn npm_v9_direct(target: &str) -> String {
    format!(
        "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\{target}\"   %*\r\n"
    )
}

// Exact legacy cmd-shim@5.0.0/Corepack static Node form.
fn legacy_cmd_shim(target: &str) -> String {
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
        recognized_shim(&npm_v8(global_target)),
        Some((global_target, ShimRuntime::Node))
    );
    assert_eq!(
        recognized_shim(&npm_v9_node(global_target)),
        Some((global_target, ShimRuntime::Node))
    );
    assert_eq!(
        recognized_shim(&npm_v9_direct(claude)),
        Some((claude, ShimRuntime::DirectNative))
    );
    assert_eq!(
        recognized_shim(&legacy_cmd_shim(local_target)),
        Some((local_target, ShimRuntime::Node))
    );
    assert_eq!(
        recognized_shim(&npm_v9_node(global_target).replace("@ECHO off", "@ECHO off ")),
        None
    );
    assert_eq!(
        recognized_shim(
            &npm_v9_node(global_target).replace("\"%_prog%\"  ", "\"%_prog%\" --inspect ")
        ),
        None
    );
    assert_eq!(
        recognized_shim(&npm_v8(global_target).replace("\"%_prog%\"  ", "\"%_prog%\" ")),
        None
    );
}

#[test]
fn local_bin_shims_resolve_bounded_parent_targets() {
    let temp = tempfile::tempdir().expect("tempdir");
    let shim = local_bin(temp.path(), "claude");
    let target = temp
        .path()
        .join("node_modules/@anthropic-ai/claude-code/bin/claude.exe");
    write(
        &shim,
        &npm_v9_direct("..\\@anthropic-ai\\claude-code\\bin\\claude.exe"),
    );
    write(&target, "fixture");
    let args = vec![OsString::from("--resume"), OsString::from("task")];
    let launch = prepare_windows_batch_launch_from_source_env(
        shim,
        args.clone(),
        &BTreeMap::new(),
        temp.path(),
    )
    .expect("direct plan");
    assert_eq!(
        launch,
        WindowsNativeLaunch {
            program: target.canonicalize().expect("canonical target"),
            args
        }
    );
}

#[test]
fn cmd_shim_v9_node_resolves_bounded_parent_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let shim = local_bin(temp.path(), "agent");
    let target = temp.path().join("node_modules/agent/bin/agent.js");
    let node = temp.path().join("trusted-node/node.exe");
    write(&shim, &npm_v9_node("..\\agent\\bin\\agent.js"));
    write(&target, "fixture");
    write(&node, "fixture");
    let source_env = BTreeMap::from([(
        "PATH".into(),
        temp.path().join("trusted-node").display().to_string(),
    )]);
    let launch =
        prepare_windows_batch_launch_from_source_env(shim, Vec::new(), &source_env, temp.path())
            .expect("cmd-shim@9 Node plan");
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
    write(&shim, &legacy_cmd_shim("..\\dist\\corepack.js"));
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
fn prefix_shims_resolve_node_modules_and_keep_os_argv() {
    let temp = tempfile::tempdir().expect("tempdir");
    let shim = temp.path().join("prefix/agent.cmd");
    let target = temp.path().join("prefix/node_modules/agent/bin/agent.js");
    let node = temp.path().join("trusted-node/node.EXE");
    write(&shim, &npm_v9_node("node_modules\\agent\\bin\\agent.js"));
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
        "node_modules\\%PROGRAMFILES%\\agent.js",
    ] {
        let shim = temp.path().join(format!("prefix/{}.cmd", target.len()));
        write(&shim, &npm_v9_node(target));
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
    write(&shim, &npm_v9_node("..\\agent\\bin\\agent.js"));
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
fn absolute_non_batch_programs_remain_unchanged() {
    let temp = tempfile::tempdir().expect("tempdir");
    let program = temp.path().join("agent.exe");
    let args = vec![OsString::from("--native")];
    assert_eq!(
        prepare_windows_batch_launch_from_source_env(
            program.clone(),
            args.clone(),
            &BTreeMap::new(),
            temp.path(),
        )
        .expect("non-batch program"),
        WindowsNativeLaunch { program, args }
    );
}

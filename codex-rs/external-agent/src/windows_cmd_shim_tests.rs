use super::*;
use pretty_assertions::assert_eq;
const CMD_SHIM_V5_NODE_GOLDEN: &str = include_str!("fixtures/cmd-shim-5.0.0-node.cmd.golden");
const CMD_SHIM_V9_NODE_GOLDEN: &str = include_str!("fixtures/cmd-shim-9.0.2-node.cmd.golden");
const NODE_CWD: &str = r"C:\intended-cwd";
fn released_cmd(golden: &str) -> String {
    golden
        .trim_end_matches(['\r', '\n'])
        .replace(r"\r\n", "\r\n")
}
fn v9_node_with_target(target: &str) -> String {
    released_cmd(CMD_SHIM_V9_NODE_GOLDEN).replace("node_modules\\agent\\bin\\agent.js", target)
}
fn npm_v9_direct(target: &str) -> String {
    format!(
        "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\{target}\"   %*\r\n"
    )
}
fn legacy_seven_line_corepack_shim(target: &str) -> String {
    format!(
        "@SETLOCAL\r\n@IF EXIST \"%~dp0\\node.exe\" (\r\n  \"%~dp0\\node.exe\"  \"%~dp0\\{target}\" %*\r\n) ELSE (\r\n  @SET PATHEXT=%PATHEXT:;.JS;=;%\r\n  node  \"%~dp0\\{target}\" %*\r\n)\r\n"
    )
}
fn write(path: &Path, contents: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture");
    std::fs::write(path, contents).expect("write fixture");
}
fn assert_reader_error(path: &Path, kind: std::io::ErrorKind) {
    assert_eq!(read_bounded_shim(path).unwrap_err().kind(), kind);
}
fn local_bin(temp: &Path, name: &str) -> PathBuf {
    temp.join("node_modules/.bin").join(format!("{name}.cmd"))
}
fn node_source_env(path: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([("PATH".into(), path.display().to_string())])
}
fn expected_launch(program: &Path, args: Vec<OsString>) -> WindowsNativeLaunch {
    let program = program.canonicalize().expect("canonical program");
    WindowsNativeLaunch { program, args }
}
fn expected_node_launch(target: &Path, node: &Path, args: Vec<OsString>) -> WindowsNativeLaunch {
    let target = target
        .canonicalize()
        .expect("canonical target")
        .into_os_string();
    expected_launch(node, std::iter::once(target).chain(args).collect())
}
fn prepare_empty_args(
    program: PathBuf,
    source_env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<WindowsNativeLaunch, WindowsBatchLaunchError> {
    prepare_windows_batch_launch_from_source_env(program, Vec::new(), source_env, cwd)
}
fn assert_node_shim_resolves(temp: &Path, shim: &Path, contents: &str, target: &Path) {
    let node = temp.join("trusted-node/node.exe");
    write(shim, contents);
    write(target, "fixture");
    write(&node, "fixture");
    let source_env = node_source_env(Path::new("trusted-node"));
    let launch = prepare_empty_args(shim.to_path_buf(), &source_env, temp).expect("Node shim plan");
    assert_eq!(launch, expected_node_launch(target, &node, Vec::new()));
}
#[test]
fn recognizes_exact_static_generators_and_rejects_rewrites() {
    let global_target = "node_modules\\agent\\bin\\agent.js";
    let local_target = "..\\dist\\corepack.js";
    let claude = "..\\@anthropic-ai\\claude-code\\bin\\claude.exe";
    for (shim, target, runtime) in [
        (
            released_cmd(CMD_SHIM_V5_NODE_GOLDEN),
            global_target,
            ShimRuntime::Node,
        ),
        (
            released_cmd(CMD_SHIM_V9_NODE_GOLDEN),
            global_target,
            ShimRuntime::Node,
        ),
        (npm_v9_direct(claude), claude, ShimRuntime::DirectNative),
        (
            legacy_seven_line_corepack_shim(local_target),
            local_target,
            ShimRuntime::Node,
        ),
    ] {
        assert_eq!(recognized_shim(&shim), Some((target, runtime)));
    }
    for shim in [
        released_cmd(CMD_SHIM_V9_NODE_GOLDEN).replace("@ECHO off", "@ECHO off "),
        released_cmd(CMD_SHIM_V9_NODE_GOLDEN).replace("\"%_prog%\"  ", "\"%_prog%\" --inspect "),
        released_cmd(CMD_SHIM_V5_NODE_GOLDEN).replace("\r\n  SET PATHEXT", "\r\n SET PATHEXT"),
        legacy_seven_line_corepack_shim(local_target).replace("  @SET PATHEXT", " @SET PATHEXT"),
    ] {
        assert_eq!(recognized_shim(&shim), None);
    }
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
        npm_v9_direct("..\\@anthropic-ai\\claude-code\\bin\\claude.exe"),
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
    assert_eq!(launch, expected_launch(&target, args));
}
#[test]
fn node_shim_fixtures_resolve_bounded_targets() {
    let temp = tempfile::tempdir().expect("tempdir");
    assert_node_shim_resolves(
        temp.path(),
        &temp.path().join("prefix/agent.cmd"),
        &released_cmd(CMD_SHIM_V5_NODE_GOLDEN),
        &temp.path().join("prefix/node_modules/agent/bin/agent.js"),
    );
    assert_node_shim_resolves(
        temp.path(),
        &local_bin(temp.path(), "corepack"),
        &legacy_seven_line_corepack_shim("..\\dist\\corepack.js"),
        &temp.path().join("node_modules/dist/corepack.js"),
    );
}
#[test]
fn published_cmd_shim_v9_node_fixture_resolves_node_modules_and_keeps_os_argv() {
    let temp = tempfile::tempdir().expect("tempdir");
    let shim = temp.path().join("prefix/agent.cmd");
    let target = temp.path().join("prefix/node_modules/agent/bin/agent.js");
    let node = temp.path().join("trusted-node/node.EXE");
    write(&shim, released_cmd(CMD_SHIM_V9_NODE_GOLDEN));
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
    let mut source_env = node_source_env(node.parent().expect("node parent"));
    source_env.insert(
        "COMSPEC".into(),
        temp.path().join("poisoned.cmd").display().to_string(),
    );
    let launch =
        prepare_windows_batch_launch_from_source_env(shim, args.clone(), &source_env, temp.path())
            .expect("source PATH plan");
    assert_eq!(launch, expected_node_launch(&target, &node, args));
}
#[test]
fn rejects_relative_program_before_classifying_or_reading_the_shim() {
    let temp = tempfile::tempdir().expect("tempdir");
    let error = prepare_empty_args(PathBuf::from("agent.cmd"), &BTreeMap::new(), temp.path())
        .expect_err("relative shim must not bind to the host current directory");
    assert!(matches!(error, WindowsBatchLaunchError::ProgramNotAbsolute));
}
#[test]
fn rejects_oversized_shim_before_utf8_decoding_or_line_splitting() {
    let temp = tempfile::tempdir().expect("tempdir");
    let shim = temp.path().join("prefix/oversized.cmd");
    std::fs::create_dir_all(shim.parent().expect("fixture parent")).expect("create fixture");
    let file = std::fs::File::create(&shim).expect("create oversized fixture");
    file.set_len((MAX_CMD_SHIM_BYTES + 1) as u64)
        .expect("extend oversized fixture");
    let error = prepare_empty_args(shim, &BTreeMap::new(), temp.path())
        .expect_err("oversized shim must be rejected by the bounded read");
    assert!(matches!(
        error,
        WindowsBatchLaunchError::ReadShim(ref error)
            if error.kind() == std::io::ErrorKind::InvalidData
    ));
    let temp = tempfile::tempdir().expect("bounded reader tempdir");
    let shim = temp.path().join("prefix/bounded.cmd");
    write(&shim, "x".repeat(MAX_CMD_SHIM_BYTES));
    let error = prepare_empty_args(shim, &BTreeMap::new(), temp.path())
        .expect_err("exactly bounded shim must reach classification");
    assert!(matches!(error, WindowsBatchLaunchError::UnsupportedShim));
    let invalid_utf8 = temp.path().join("prefix/invalid-utf8.cmd");
    write(&invalid_utf8, [0xff]);
    assert_reader_error(&invalid_utf8, std::io::ErrorKind::InvalidData);
    let missing = temp.path().join("missing.cmd");
    assert_reader_error(&missing, std::io::ErrorKind::NotFound);
    assert!(read_bounded_shim(temp.path()).is_err());
}
#[test]
fn rejects_unsafe_path_entries_before_node_lookup() {
    for path in ["C:relative", r"\root-only"] {
        let source_env = BTreeMap::from([("PATH".into(), path.into())]);
        assert!(matches!(
            native_node_with_probe(&source_env, Path::new(NODE_CWD), |_| {
                panic!("unsafe PATH entry must not be probed")
            }),
            Err(WindowsBatchLaunchError::NodeNotFound(_))
        ));
    }
}
#[test]
fn node_path_entries_resolve_relative_absolute_and_unc_paths() {
    for (entry, node) in [
        ("trusted-node", r"C:\intended-cwd\trusted-node\node.exe"),
        (r"C:\node-bin", r"C:\node-bin\node.exe"),
        (
            r"\\server\share\node-bin",
            r"\\server\share\node-bin\node.exe",
        ),
    ] {
        let source_env = BTreeMap::from([("PATH".into(), entry.into())]);
        let node = PathBuf::from(node);
        native_node_with_probe(&source_env, Path::new(NODE_CWD), |candidate| {
            assert_eq!(candidate, node);
            Ok(Some(candidate.to_path_buf()))
        })
        .expect("node path entry");
    }
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
        let error = prepare_empty_args(program, &BTreeMap::new(), temp.path())
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
        write(&shim, v9_node_with_target(target));
        let error = prepare_empty_args(shim, &BTreeMap::new(), temp.path())
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
    write(&shim, v9_node_with_target("..\\agent\\bin\\agent.js"));
    write(&outside, "fixture");
    std::fs::create_dir_all(linked.parent().expect("linked parent")).expect("create linked parent");
    std::os::windows::fs::symlink_file(&outside, &linked).expect("create fixture symlink");

    let error = prepare_empty_args(shim, &BTreeMap::new(), temp.path())
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
        expected_launch(&program, args)
    );
}

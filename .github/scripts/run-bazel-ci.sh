#!/usr/bin/env bash

set -euo pipefail

print_failed_bazel_test_logs=0
print_failed_bazel_action_summary=0
remote_download_toplevel=0
windows_msvc_host_platform=0
windows_cross_compile=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --print-failed-test-logs)
      print_failed_bazel_test_logs=1
      shift
      ;;
    --print-failed-action-summary)
      print_failed_bazel_action_summary=1
      shift
      ;;
    --remote-download-toplevel)
      remote_download_toplevel=1
      shift
      ;;
    --windows-msvc-host-platform)
      windows_msvc_host_platform=1
      shift
      ;;
    --windows-cross-compile)
      windows_cross_compile=1
      shift
      ;;
    --)
      shift
      break
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

if [[ $# -eq 0 ]]; then
  echo "Usage: $0 [--print-failed-test-logs] [--print-failed-action-summary] [--remote-download-toplevel] [--windows-msvc-host-platform] [--windows-cross-compile] -- <bazel args> -- <targets>" >&2
  exit 1
fi

bazel_startup_args=()
if [[ -n "${BAZEL_OUTPUT_USER_ROOT:-}" ]]; then
  bazel_startup_args+=("--output_user_root=${BAZEL_OUTPUT_USER_ROOT}")
fi

run_bazel() {
  if [[ "${RUNNER_OS:-}" == "Windows" ]]; then
    MSYS2_ARG_CONV_EXCL='*' bazel "$@"
    return
  fi

  bazel "$@"
}

ci_config=ci-linux
case "${RUNNER_OS:-}" in
  macOS)
    ci_config=ci-macos
    ;;
  Windows)
    if [[ $windows_cross_compile -eq 1 ]]; then
      ci_config=ci-windows-cross
    else
      ci_config=ci-windows
    fi
    ;;
esac

print_bazel_test_log_tails() {
  local console_log="$1"
  local testlogs_dir
  local -a bazel_info_cmd=(bazel)
  local -a bazel_info_args=(info)

  if (( ${#bazel_startup_args[@]} > 0 )); then
    bazel_info_cmd+=("${bazel_startup_args[@]}")
  fi

  # `bazel info` needs the same CI config as the failed test invocation so
  # platform-specific output roots match. On Windows, omitting `ci-windows`
  # would point at `local_windows-fastbuild` even when the test ran with the
  # MSVC host platform under `local_windows_msvc-fastbuild`.
  if [[ -n "${BUILDBUDDY_API_KEY:-}" ]]; then
    bazel_info_args+=(
      "--config=${ci_config}"
      "--remote_header=x-buildbuddy-api-key=${BUILDBUDDY_API_KEY}"
    )
  fi
  # Only pass flags that affect Bazel's output-root selection or repository
  # lookup. Test/build-only flags such as execution logs or remote download
  # mode can make `bazel info` fail, which would hide the real test log path.
  for arg in "${post_config_bazel_args[@]}"; do
    case "$arg" in
      --host_platform=* | --repo_contents_cache=* | --repository_cache=*)
        bazel_info_args+=("$arg")
        ;;
    esac
  done

  testlogs_dir="$(run_bazel "${bazel_info_cmd[@]:1}" \
    --noexperimental_remote_repo_contents_cache \
    "${bazel_info_args[@]}" \
    bazel-testlogs 2>/dev/null || echo bazel-testlogs)"

  local failed_targets=()
  while IFS= read -r target; do
    failed_targets+=("$target")
  done < <(
    grep -E '^(FAIL: //|ERROR: .* Testing //)' "$console_log" \
      | sed -E 's#^FAIL: (//[^ ]+).*#\1#; s#^ERROR: .* Testing (//[^ ]+) failed:.*#\1#' \
      | sort -u
  )

  if [[ ${#failed_targets[@]} -eq 0 ]]; then
    echo "No failed Bazel test targets were found in console output."
    return
  fi

  for target in "${failed_targets[@]}"; do
    local rel_path="${target#//}"
    rel_path="${rel_path/://}"
    local test_log="${testlogs_dir}/${rel_path}/test.log"
    local reported_test_log
    reported_test_log="$(grep -F "FAIL: ${target} " "$console_log" | sed -nE 's#.* \(see (.*[\\/]test\.log)\).*#\1#p' | head -n 1 || true)"
    if [[ -n "$reported_test_log" ]]; then
      reported_test_log="${reported_test_log//\\//}"
      test_log="$reported_test_log"
    fi

    echo "::group::Bazel test log tail for ${target}"
    if [[ -f "$test_log" ]]; then
      tail -n 200 "$test_log"
    else
      echo "Missing test log: $test_log"
    fi
    echo "::endgroup::"
  done
}

print_bazel_action_failure_summary() {
  local console_log="$1"
  local escaped_summary
  local summary

  summary="$(
    awk '
      function clean(line) {
        gsub(sprintf("%c", 27) "\\[[0-9;]*m", "", line)
        sub(/^.*\t[^\t]*\t[0-9TZ:._-]+ /, "", line)
        return line
      }

      function is_diagnostic(line) {
        return line ~ /^(error(\[[^]]+\])?:|warning:|note:|help:)/ ||
          line ~ /^[[:space:]]+-->/ ||
          line ~ /^[[:space:]]*[0-9]+[[:space:]]+\|/ ||
          line ~ /^[[:space:]]*\|/ ||
          line ~ /^[[:space:]]+= (note|help):/ ||
          line ~ /^[[:space:]]*\^[[:space:]^~-]*$/ ||
          line ~ /^For more information/ ||
          line ~ /^error: aborting/
      }

      {
        line = clean($0)
      }

      line ~ /^ERROR: .* failed:/ {
        if (printed) {
          print ""
        }
        print line
        in_failure = 1
        seen_diagnostic = 0
        printed = 1
        next
      }

      in_failure && is_diagnostic(line) {
        print line
        seen_diagnostic = 1
        next
      }

      in_failure && seen_diagnostic && line == "" {
        print ""
        next
      }

      in_failure && seen_diagnostic {
        in_failure = 0
        seen_diagnostic = 0
        next
      }
    ' "$console_log"
  )"

  if [[ -z "$summary" ]]; then
    summary="$(grep -E '^ERROR: |^FAILED: ' "$console_log" | tail -n 50 || true)"
  fi

  if [[ -z "$summary" ]]; then
    echo "No Bazel action failures were found in the captured console output."
    return
  fi

  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    escaped_summary="$(
      printf '%s' "$summary" \
        | awk 'BEGIN { ORS = "" } {
            gsub(/%/, "%25")
            gsub(/\r/, "%0D")
            print sep $0
            sep = "%0A"
          }'
    )"
    echo "::error title=Bazel failed action diagnostics::${escaped_summary}"
  fi

  echo
  echo "Bazel failed action diagnostics:"
  echo "--------------------------------"
  printf '%s\n' "$summary"
  echo "--------------------------------"
}

bazel_args=()
bazel_targets=()
found_target_separator=0
for arg in "$@"; do
  if [[ "$arg" == "--" && $found_target_separator -eq 0 ]]; then
    found_target_separator=1
    continue
  fi

  if [[ $found_target_separator -eq 0 ]]; then
    bazel_args+=("$arg")
  else
    bazel_targets+=("$arg")
  fi
done

if [[ ${#bazel_args[@]} -eq 0 || ${#bazel_targets[@]} -eq 0 ]]; then
  echo "Expected Bazel args and targets separated by --" >&2
  exit 1
fi

windows_rbe_host_platform=0
windows_host_platform_override=""
windows_execution_platform_override=""
windows_rbe_endpoint_configured=0
windows_rbe_execution_platform=0
windows_rbe_config_requested=0
if [[ "${RUNNER_OS:-}" == "Windows" && $windows_cross_compile -eq 1 ]]; then
  # The target remains windows-gnullvm, but the default exec host must match
  # the Windows runner. Selecting the Linux RBE platform without an active
  # remote executor makes Bazel run Linux LLVM helpers locally on Windows.
  windows_msvc_host_platform=1
  bazel_arg_index=0
  while [[ $bazel_arg_index -lt ${#bazel_args[@]} ]]; do
    arg="${bazel_args[$bazel_arg_index]}"
    option_name=""
    option_value=""
    case "$arg" in
      --host_platform | --remote_executor | --config | --extra_execution_platforms)
        option_name="$arg"
        value_index=$((bazel_arg_index + 1))
        if [[ $value_index -ge ${#bazel_args[@]} \
          || "${bazel_args[$value_index]}" == --* ]]; then
          echo "${option_name} requires a value" >&2
          exit 1
        fi
        option_value="${bazel_args[$value_index]}"
        bazel_arg_index=$((bazel_arg_index + 2))
        ;;
      --host_platform=* | --remote_executor=* | --config=* | --extra_execution_platforms=*)
        option_name="${arg%%=*}"
        option_value="${arg#*=}"
        bazel_arg_index=$((bazel_arg_index + 1))
        ;;
      *)
        bazel_arg_index=$((bazel_arg_index + 1))
        continue
        ;;
    esac

    if [[ "$option_name" != "--remote_executor" && -z "$option_value" ]]; then
      echo "${option_name} requires a non-empty value" >&2
      exit 1
    fi

    case "$option_name" in
      --host_platform)
        # Bazel uses the last host-platform option. Preserve that ordering when
        # repeating the caller's choice after the expanded CI configuration.
        windows_msvc_host_platform=0
        windows_host_platform_override="--host_platform=${option_value}"
        # `bazel query` confirms that @codex//:rbe is the main module's
        # apparent form and @@//:rbe is its canonical form. Keep the match
        # exact so an external repository with its own :rbe target cannot opt
        # this wrapper into remote execution.
        case "$option_value" in
          //:rbe | @//:rbe | @@//:rbe | @codex//:rbe)
            windows_rbe_host_platform=1
            ;;
          *)
            windows_rbe_host_platform=0
            ;;
        esac
        ;;
      --config)
        case "$option_value" in
          buildbuddy-generic-rbe | buildbuddy-openai-rbe)
            windows_rbe_endpoint_configured=1
            windows_rbe_config_requested=1
            ;;
        esac
        ;;
      --remote_executor)
        if [[ -n "$option_value" ]]; then
          windows_rbe_endpoint_configured=1
        else
          windows_rbe_endpoint_configured=0
        fi
        ;;
      --extra_execution_platforms)
        # Repeat the caller's final override after ci-windows-cross expands so
        # both Bazel spellings retain true command-line last-one-wins behavior.
        windows_execution_platform_override="--extra_execution_platforms=${option_value}"
        windows_rbe_execution_platform=0
        IFS=',' read -r -a execution_platforms <<< "$option_value"
        for execution_platform in "${execution_platforms[@]}"; do
          case "$execution_platform" in
            //:rbe | @//:rbe | @@//:rbe | @codex//:rbe)
              windows_rbe_execution_platform=1
              ;;
          esac
        done
        ;;
    esac
  done

  if [[ -z "$windows_execution_platform_override" \
    && $windows_rbe_config_requested -eq 1 ]]; then
    # The remote config expands to --extra_execution_platforms=//:rbe. An
    # explicit execution-platform override is repeated after configs below and
    # therefore owns the effective list; otherwise account for that expansion
    # during validation rather than waiting for Bazel to mix it with a local
    # Windows host.
    windows_rbe_execution_platform=1
  fi

  if [[ $windows_rbe_host_platform -eq 0 \
    && $windows_rbe_execution_platform -eq 1 ]]; then
    echo "Windows RBE execution platform requires a complete RBE topology: recognized RBE host platform and nonempty remote execution endpoint." >&2
    exit 1
  fi
  if [[ $windows_rbe_host_platform -eq 0 \
    && $windows_rbe_endpoint_configured -eq 1 ]]; then
    echo "Windows remote execution endpoint requires a complete RBE topology: recognized RBE host and RBE-compatible execution platforms." >&2
    exit 1
  fi
  if [[ $windows_rbe_host_platform -eq 1 && $windows_rbe_endpoint_configured -eq 0 ]]; then
    echo "Windows RBE host platform requires an endpoint-bearing remote execution config or --remote_executor." >&2
    exit 1
  fi
  if [[ $windows_rbe_execution_platform -eq 1 && $windows_rbe_host_platform -eq 0 ]]; then
    echo "Windows RBE execution platform requires a final RBE host platform override." >&2
    exit 1
  fi
  if [[ $windows_rbe_execution_platform -eq 1 && $windows_rbe_endpoint_configured -eq 0 ]]; then
    echo "Windows RBE execution platform requires an endpoint-bearing remote execution config or --remote_executor." >&2
    exit 1
  fi
  if [[ $windows_rbe_host_platform -eq 1 \
    && -n "$windows_execution_platform_override" \
    && $windows_rbe_execution_platform -eq 0 ]]; then
    echo "Windows RBE host platform requires a final RBE-compatible execution platform override." >&2
    exit 1
  fi
fi

post_config_bazel_args=()
if [[ "${RUNNER_OS:-}" == "Windows" && $windows_cross_compile -eq 1 ]]; then
  # Keep the target explicit in every keyed and keyless cross-compile path.
  # Host/execution-platform overrides below remain independent and last.
  post_config_bazel_args+=(--platforms=//:windows_x86_64_gnullvm)
fi
if [[ "${RUNNER_OS:-}" == "Windows" && $windows_msvc_host_platform -eq 1 ]]; then
  has_host_platform_override=0
  for arg in "${bazel_args[@]}"; do
    if [[ "$arg" == --host_platform=* ]]; then
      has_host_platform_override=1
      break
    fi
  done

  if [[ $has_host_platform_override -eq 0 ]]; then
    # Use the MSVC Windows platform for jobs that need helper binaries like
    # Rust test wrappers and V8 generators to resolve a compatible toolchain.
    # Callers that need a different Windows target platform should pass an
    # explicit `--platforms=...` flag.
    post_config_bazel_args+=("--host_platform=//:local_windows_msvc")
  fi
fi

if [[ $remote_download_toplevel -eq 1 ]]; then
  # Override the CI config's remote_download_minimal setting when callers need
  # the built artifact to exist on disk after the command completes.
  post_config_bazel_args+=(--remote_download_toplevel)
fi

if [[ -n "$windows_host_platform_override" ]]; then
  # Repeat explicit overrides after the CI config so they win over the
  # Windows-local default in .bazelrc.
  post_config_bazel_args+=("$windows_host_platform_override")
fi

if [[ -n "$windows_execution_platform_override" ]]; then
  post_config_bazel_args+=("$windows_execution_platform_override")
  if [[ $windows_rbe_host_platform -eq 1 ]]; then
    post_config_bazel_args+=(--shell_executable=/bin/bash)
  fi
elif [[ $windows_rbe_host_platform -eq 1 ]]; then
  # Bazel also derives the default genrule shell from the client host. Without
  # an explicit shell executable, remote Linux actions can be asked to run
  # `C:\Program Files\Git\usr\bin\bash.exe`.
  post_config_bazel_args+=(
    --extra_execution_platforms=//:rbe,//:windows_x86_64_msvc
    --shell_executable=/bin/bash
  )
elif [[ "${RUNNER_OS:-}" == "Windows" && $windows_cross_compile -eq 1 ]]; then
  # `ci-windows-cross` historically listed Linux RBE first. Bazel selects the
  # first compatible execution platform, independently of --host_platform, so
  # replace (rather than append to) that list for local Windows execution.
  # Bazel documents this option as last-one-wins.
  post_config_bazel_args+=(--extra_execution_platforms=//:windows_x86_64_msvc)
fi

if [[ "${RUNNER_OS:-}" == "Windows" && $windows_cross_compile -eq 1 && -z "${BUILDBUDDY_API_KEY:-}" ]]; then
  # Keep keyless Windows cross-builds on their established lower local
  # concurrency cap.
  post_config_bazel_args+=(--jobs=8)
fi

if [[ -n "${BAZEL_REPO_CONTENTS_CACHE:-}" ]]; then
  # Windows self-hosted runners can run multiple Bazel jobs concurrently. Give
  # each job its own repo contents cache so they do not fight over the shared
  # path configured in `ci-windows`.
  post_config_bazel_args+=("--repo_contents_cache=${BAZEL_REPO_CONTENTS_CACHE}")
fi

if [[ -n "${BAZEL_REPOSITORY_CACHE:-}" ]]; then
  post_config_bazel_args+=("--repository_cache=${BAZEL_REPOSITORY_CACHE}")
fi

if [[ -n "${CODEX_BAZEL_EXECUTION_LOG_COMPACT_DIR:-}" ]]; then
  post_config_bazel_args+=(
    "--execution_log_compact_file=${CODEX_BAZEL_EXECUTION_LOG_COMPACT_DIR}/execution-log-${bazel_args[0]}-${GITHUB_JOB:-local}-$$.zst"
  )
fi

if [[ "${RUNNER_OS:-}" == "Windows" ]]; then
  pass_windows_build_env=1
  if [[ $windows_rbe_host_platform -eq 1 ]]; then
    # Remote build actions execute on Linux RBE workers. Passing the Windows
    # runner's build environment there makes Bazel genrules try to execute
    # C:\Program Files\Git\usr\bin\bash.exe on Linux.
    pass_windows_build_env=0
  fi

  if [[ $pass_windows_build_env -eq 1 ]]; then
    windows_action_env_vars=(
      INCLUDE
      LIB
      LIBPATH
      UCRTVersion
      UniversalCRTSdkDir
      VCINSTALLDIR
      VCToolsInstallDir
      WindowsLibPath
      WindowsSdkBinPath
      WindowsSdkDir
      WindowsSDKLibVersion
      WindowsSDKVersion
    )

    for env_var in "${windows_action_env_vars[@]}"; do
      if [[ -n "${!env_var:-}" ]]; then
        post_config_bazel_args+=("--action_env=${env_var}" "--host_action_env=${env_var}")
      fi
    done
  fi

  if [[ -z "${CODEX_BAZEL_WINDOWS_PATH:-}" ]]; then
    echo "CODEX_BAZEL_WINDOWS_PATH must be set for Windows Bazel CI." >&2
    exit 1
  fi

  if [[ $pass_windows_build_env -eq 1 ]]; then
    post_config_bazel_args+=(
      "--action_env=PATH=${CODEX_BAZEL_WINDOWS_PATH}"
      "--host_action_env=PATH=${CODEX_BAZEL_WINDOWS_PATH}"
    )
  elif [[ $windows_rbe_host_platform -eq 1 ]]; then
    # Remote build actions run on Linux RBE workers. Give their shell snippets
    # a Linux PATH while preserving CODEX_BAZEL_WINDOWS_PATH below for local
    # Windows test execution.
    post_config_bazel_args+=(
      "--action_env=PATH=/usr/bin:/bin"
      "--host_action_env=PATH=/usr/bin:/bin"
    )
  fi
  post_config_bazel_args+=("--test_env=PATH=${CODEX_BAZEL_WINDOWS_PATH}")
fi

bazel_console_log="$(mktemp)"
trap 'rm -f "$bazel_console_log"' EXIT

bazel_cmd=(bazel)
if (( ${#bazel_startup_args[@]} > 0 )); then
  bazel_cmd+=("${bazel_startup_args[@]}")
fi

if [[ -n "${BUILDBUDDY_API_KEY:-}" ]]; then
  echo "BuildBuddy API key is available; using keyed Bazel configuration."
  # Work around Bazel 9 remote repo contents cache / overlay materialization failures
  # seen in CI (for example "is not a symlink" or permission errors while
  # materializing external repos such as rules_perl). Keyed configs can use
  # BuildBuddy services; this only disables the startup-level repo contents cache.
  buildbuddy_config=""
  if [[ "${RUNNER_OS:-}" != "Windows" \
    || $windows_cross_compile -ne 1 \
    || $windows_rbe_host_platform -eq 0 ]]; then
    # Resolve the tenant with the same trust boundary as the generic Bazel
    # wrapper. OpenAI CI configs get the -rbe form so they actually select
    # remote execution platforms; generic Hasna runs use cache/BES/download
    # services because that BuildBuddy host has no registered executors.
    buildbuddy_config="$(
      python3 "$(dirname "${BASH_SOURCE[0]}")/run_bazel_with_buildbuddy.py" \
        --print-config-for "--config=${ci_config}"
    )"
    case "$buildbuddy_config" in
      buildbuddy-generic | buildbuddy-openai | buildbuddy-generic-rbe | buildbuddy-openai-rbe)
        bazel_run_args+=("--config=${buildbuddy_config}")
        ;;
      *)
        echo "Unable to select a BuildBuddy configuration." >&2
        exit 1
        ;;
    esac
  fi
  ci_config_for_bazel="$ci_config"
  if [[ "$buildbuddy_config" == "buildbuddy-generic" && "$ci_config" == "ci-linux" ]]; then
    ci_config_for_bazel="ci-keyless"
  fi
  bazel_run_args=(
    "${bazel_args[@]}"
    "--config=${ci_config_for_bazel}"
  )
  if [[ -n "$buildbuddy_config" ]]; then
    bazel_run_args+=("--config=${buildbuddy_config}")
  fi
  bazel_run_args+=("--remote_header=x-buildbuddy-api-key=${BUILDBUDDY_API_KEY}")
  if (( ${#post_config_bazel_args[@]} > 0 )); then
    bazel_run_args+=("${post_config_bazel_args[@]}")
  fi
  set +e
  run_bazel "${bazel_cmd[@]:1}" \
    --noexperimental_remote_repo_contents_cache \
    "${bazel_run_args[@]}" \
    -- \
    "${bazel_targets[@]}" \
    2>&1 | tee "$bazel_console_log"
  bazel_status=${PIPESTATUS[0]}
  set -e
else
  echo "BuildBuddy API key is not available; using local Bazel configuration."
  # Keep fork/community PRs on Bazel but disable remote services that are
  # configured in .bazelrc and require auth.
  #
  # Flag docs:
  # - Command-line reference: https://bazel.build/reference/command-line-reference
  # - Remote caching overview: https://bazel.build/remote/caching
  # - Remote execution overview: https://bazel.build/remote/rbe
  # - Build Event Protocol overview: https://bazel.build/remote/bep
  #
  # --noexperimental_remote_repo_contents_cache:
  #   disable remote repo contents cache enabled in .bazelrc startup options.
  #   https://bazel.build/reference/command-line-reference#startup_options-flag--experimental_remote_repo_contents_cache
  # --remote_cache= and --remote_executor=:
  #   clear remote cache/execution endpoints configured in .bazelrc.
  #   https://bazel.build/reference/command-line-reference#common_options-flag--remote_cache
  #   https://bazel.build/reference/command-line-reference#common_options-flag--remote_executor
  # --experimental_remote_downloader=:
  #   clear the remote downloader configured in .bazelrc. Bazel requires the
  #   remote downloader to be paired with a gRPC remote cache, so leaving it set
  #   while clearing --remote_cache makes no-secret CI fail before the build.
  #   https://bazel.build/reference/command-line-reference#common_options-flag--experimental_remote_downloader
  bazel_run_args=(
    "${bazel_args[@]}"
    --remote_cache=
    --experimental_remote_downloader=
  )
  if [[ "${RUNNER_OS:-}" == "Windows" && $windows_cross_compile -eq 1 ]]; then
    # The cross config is required even when a keyless caller supplies its own
    # validated RBE endpoint and therefore owns --remote_executor.
    bazel_run_args+=("--config=${ci_config}")
  fi
  if [[ $windows_rbe_host_platform -eq 0 ]]; then
    # An explicit, validated Windows RBE opt-in owns its executor setting.
    # Every ordinary keyless invocation still clears remote execution.
    bazel_run_args+=(--remote_executor=)
  fi
  if [[ "${ci_config}" == "ci-linux" ]]; then
    # Keyless Linux: opt into the `ci-keyless` config (see .bazelrc), which
    # re-enables a bounded, workflow-persisted local disk cache so action
    # outputs survive across runs instead of cold-building every time. Other
    # platforms keep their existing keyless build shape unchanged.
    bazel_run_args+=(--config=ci-keyless)
  fi
  if (( ${#post_config_bazel_args[@]} > 0 )); then
    bazel_run_args+=("${post_config_bazel_args[@]}")
  fi
  set +e
  run_bazel "${bazel_cmd[@]:1}" \
    --noexperimental_remote_repo_contents_cache \
    "${bazel_run_args[@]}" \
    -- \
    "${bazel_targets[@]}" \
    2>&1 | tee "$bazel_console_log"
  bazel_status=${PIPESTATUS[0]}
  set -e
fi

if [[ ${bazel_status:-0} -ne 0 ]]; then
  if [[ $print_failed_bazel_action_summary -eq 1 ]]; then
    print_bazel_action_failure_summary "$bazel_console_log"
  fi
  if [[ $print_failed_bazel_test_logs -eq 1 ]]; then
    print_bazel_test_log_tails "$bazel_console_log"
  fi
  exit "$bazel_status"
fi
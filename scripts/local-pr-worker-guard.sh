#!/usr/bin/env bash

set -euo pipefail

codewith_local_guard_usage() {
  cat <<'EOF'
Usage:
  source scripts/local-pr-worker-guard.sh
  scripts/local-pr-worker-guard.sh --check COMMAND [ARG...]
  scripts/local-pr-worker-guard.sh --check-line 'COMMAND LINE'
  scripts/local-pr-worker-guard.sh --run COMMAND [ARG...]
  scripts/local-pr-worker-guard.sh --install

Spark01/station01 PR-worker local guard.

When sourced on spark01 (reported by some local host tools as station01), this
installs PATH shims that deny local compile, test, install, schema-generation,
snapshot-acceptance, and package-build commands. It is intended for PR-drain
worker shells that must send builds and tests to remote/E2B runners instead of
consuming local spark01 resources.

Denied command families:
  cargo, rustc, bazel, bazelisk
  just test/check/fix/clippy/build/lint/bench/install/schema/bazel recipes
  bun/npm/pnpm/yarn install, package build, pack, publish, and schema scripts
  python package-build, package-stage, SDK/schema generation, and pip install
  insta snapshot acceptance

When sourced, the guard also installs a Bash DEBUG trap. The trap blocks common
bypass forms before execution, including absolute tool paths, env PATH=...
wrappers, command/exec wrappers, and shell -c wrappers.

Allowed examples:
  git, gh, rg, sed, bash -n, apply_patch, secrets scans, E2B runner commands

Environment:
  CODEWITH_LOCAL_GUARD=0       disable after sourcing
  CODEWITH_LOCAL_GUARD_FORCE=1 enforce even when hostname is not spark01
EOF
}

codewith_local_guard_host() {
  hostname -s 2>/dev/null || hostname 2>/dev/null || printf unknown
}

codewith_local_guard_active() {
  case "${CODEWITH_LOCAL_GUARD:-1}" in
    0|false|False|FALSE|off|Off|OFF|no|No|NO)
      return 1
      ;;
  esac

  if [[ "${CODEWITH_LOCAL_GUARD_FORCE:-}" == "1" ]]; then
    return 0
  fi

  case "$(codewith_local_guard_host)" in
    spark01|station01)
      return 0
      ;;
  esac

  return 1
}

codewith_local_guard_denied() {
  local reason="$1"
  shift || true

  {
    printf 'codewith local guard: denied on %s: %s\n' "$(codewith_local_guard_host)" "${reason}"
    printf 'command:'
    printf ' %q' "$@"
    printf '\n'
    printf 'Use the E2B/remote runner path for build, test, install, schema, snapshot, or package work.\n'
  } >&2

  return 125
}

codewith_local_guard_first_payload_arg() {
  local arg

  for arg in "$@"; do
    case "${arg}" in
      --)
        shift
        if [[ $# -gt 0 ]]; then
          printf '%s\n' "$1"
        fi
        return 0
        ;;
      -*)
        shift
        ;;
      *)
        printf '%s\n' "${arg}"
        return 0
        ;;
    esac
  done
}

codewith_local_guard_just_denied_recipe() {
  local arg

  for arg in "$@"; do
    case "${arg}" in
      --)
        continue
        ;;
      -*)
        continue
        ;;
      test|test-fast|test-fast-target|test-binaries|test-github-scripts|check-fast)
        return 0
        ;;
      fix|clippy|build|lint|install|bench|bench-smoke|build-for-release|build-timings)
        return 0
        ;;
      bazel-*|argument-comment-lint|argument-comment-lint-from-source)
        return 0
        ;;
      codewith|c|cw|codex|exec|file-search|app-server-test-client|mcp-server-run|log)
        return 0
        ;;
      write-*-schema|write-*schema|*schema*)
        return 0
        ;;
    esac
  done

  return 1
}

codewith_local_guard_command_line_denied() {
  local command_line="$1"
  local prefix='(^|[;|&][[:space:]]*)([A-Za-z_][A-Za-z0-9_]*=[^[:space:];|&()]+[[:space:]]+)*'
  local tool='(/[^[:space:];|&()]+/)?(cargo|rustc|bazel|bazelisk)'
  local env_tool='(/[^[:space:];|&()]+/)?env'
  local just='(/[^[:space:];|&()]+/)?just'
  local just_recipe='(test|test-fast|test-fast-target|test-binaries|test-github-scripts|check-fast|fix|clippy|build|lint|install|bench|bench-smoke|build-for-release|build-timings|bazel-[^[:space:];|&()]+|argument-comment-lint|argument-comment-lint-from-source|codewith|c|cw|codex|exec|file-search|app-server-test-client|mcp-server-run|log|write-[^[:space:];|&()]*schema|[^[:space:];|&()]*schema[^[:space:];|&()]*)'
  local package_manager='(/[^[:space:];|&()]+/)?(bun|npm|pnpm|yarn)'
  local package_action='(install|i|ci|add|update|upgrade|build|pack|publish)'
  local shell='(/[^[:space:];|&()]+/)?(bash|sh|dash|zsh)'
  local python='(/[^[:space:];|&()]+/)?(python|python3)'
  local re

  [[ -n "${command_line}" ]] || return 1

  re="${prefix}${tool}([[:space:];|&()]|$)"
  if [[ "${command_line}" =~ ${re} ]]; then
    return 0
  fi

  re="${prefix}${just}([[:space:]]+[^[:space:];|&()]+)*[[:space:]]+${just_recipe}([[:space:];|&()]|$)"
  if [[ "${command_line}" =~ ${re} ]]; then
    return 0
  fi

  re="${prefix}${package_manager}([[:space:]]+[^[:space:];|&()]+)*[[:space:]]+${package_action}([[:space:];|&()]|$)"
  if [[ "${command_line}" =~ ${re} ]]; then
    return 0
  fi

  re="${prefix}${package_manager}[[:space:]]+run[[:space:]]+[^;|&()[:space:]]*(build|package|schema|snapshot)[^;|&()[:space:]]*([[:space:];|&()]|$)"
  if [[ "${command_line}" =~ ${re} ]]; then
    return 0
  fi

  re="${prefix}${python}([[:space:]]+[^[:space:];|&()]+)*[[:space:]]+([^[:space:];|&()]*/)?(build_codex_package|build_npm_package|stage_npm_packages|update_sdk_artifacts|[^[:space:];|&()]*schema[^[:space:];|&()]*)\.py([[:space:];|&()]|$)"
  if [[ "${command_line}" =~ ${re} ]]; then
    return 0
  fi

  re="${prefix}${python}([[:space:]]+[^[:space:];|&()]+)*[[:space:]]+-m[[:space:]]+(pip|pip3|build|maturin|setuptools|wheel)([[:space:];|&()]|$)"
  if [[ "${command_line}" =~ ${re} ]]; then
    return 0
  fi

  re="${prefix}(/[^[:space:];|&()]+/)?(pip|pip3)([[:space:];|&()]|$)"
  if [[ "${command_line}" =~ ${re} ]]; then
    return 0
  fi

  re="${prefix}(/[^[:space:];|&()]+/)?(cargo-insta|insta)[[:space:]]+accept([[:space:];|&()]|$)"
  if [[ "${command_line}" =~ ${re} ]]; then
    return 0
  fi

  re="${prefix}${env_tool}([[:space:]]+(-[^[:space:];|&()]+|[^[:space:];|&()=]+=([^[:space:];|&()])*))*[[:space:]]+(${tool}|${just}|${package_manager}|${python}|(/[^[:space:];|&()]+/)?(pip|pip3|cargo-insta|insta))([[:space:];|&()]|$)"
  if [[ "${command_line}" =~ ${re} ]]; then
    return 0
  fi

  re="${prefix}(command|builtin|exec)([[:space:]]+-[^[:space:];|&()]+)*[[:space:]]+(${tool}|${env_tool}|${just}|${package_manager}|${python}|(/[^[:space:];|&()]+/)?(pip|pip3|cargo-insta|insta))([[:space:];|&()]|$)"
  if [[ "${command_line}" =~ ${re} ]]; then
    return 0
  fi

  re="${prefix}${shell}([[:space:]]+[^[:space:];|&()]+)*[[:space:]]+-c[[:space:]]+.*(cargo|rustc|bazel|bazelisk|just[[:space:]]+([^;|&()]*)?(build|lint|test|check-fast|fix)|bun[[:space:]]+([^;|&()]*)?(install|build|pack|publish)|npm[[:space:]]+([^;|&()]*)?(install|ci|run[[:space:]]+[^;|&()]*build)|pnpm[[:space:]]+([^;|&()]*)?(install|publish|build)|yarn[[:space:]]+([^;|&()]*)?install|python3?[[:space:]]+([^;|&()]*)?(build_codex_package|build_npm_package|stage_npm_packages|update_sdk_artifacts)|insta[[:space:]]+accept)"
  if [[ "${command_line}" =~ ${re} ]]; then
    return 0
  fi

  return 1
}

codewith_local_guard_check_command_line() {
  if ! codewith_local_guard_active; then
    return 0
  fi

  if codewith_local_guard_command_line_denied "$1"; then
    codewith_local_guard_denied "sourced-shell command line matches a blocked local build/test/install bypass pattern" "shell-line" "$1"
  fi
}

codewith_local_guard_debug_trap() {
  local previous_status=$?

  if [[ "${CODEWITH_LOCAL_GUARD_IN_DEBUG:-}" == "1" ]]; then
    return "${previous_status}"
  fi

  CODEWITH_LOCAL_GUARD_IN_DEBUG=1
  if ! codewith_local_guard_check_command_line "${BASH_COMMAND}"; then
    CODEWITH_LOCAL_GUARD_IN_DEBUG=0
    return 125
  fi
  CODEWITH_LOCAL_GUARD_IN_DEBUG=0

  return "${previous_status}"
}

codewith_local_guard_install_shell_trap() {
  if [[ -z "${BASH_VERSION:-}" ]]; then
    return 0
  fi

  shopt -s extdebug
  trap 'codewith_local_guard_debug_trap' DEBUG
  export CODEWITH_LOCAL_GUARD_DEBUG_TRAP_INSTALLED=1
}

codewith_local_guard_package_manager_denied() {
  local tool="$1"
  shift

  local arg
  local saw_run=0

  for arg in "$@"; do
    case "${arg}" in
      --)
        continue
        ;;
      -*)
        continue
        ;;
      install|i|ci|add|update|upgrade)
        return 0
        ;;
      build|pack|publish)
        return 0
        ;;
      run)
        saw_run=1
        continue
        ;;
      *)
        if [[ "${saw_run}" == "1" ]]; then
          case "${arg}" in
            build|pack|publish|package|release|schema|*build*|*package*|*schema*|*snapshot*)
              return 0
              ;;
          esac
        fi
        ;;
    esac
  done

  case "${tool}" in
    bun)
      for arg in "$@"; do
        case "${arg}" in
          build)
            return 0
            ;;
        esac
      done
      ;;
  esac

  return 1
}

codewith_local_guard_python_denied() {
  local arg
  local next_is_module=0
  local next_is_script=0

  for arg in "$@"; do
    if [[ "${next_is_module}" == "1" ]]; then
      case "${arg}" in
        pip|pip3|build|maturin|setuptools|wheel)
          return 0
          ;;
      esac
      next_is_module=0
      continue
    fi

    if [[ "${next_is_script}" == "1" ]]; then
      case "${arg}" in
        *build_codex_package.py|*build_npm_package.py|*stage_npm_packages.py)
          return 0
          ;;
        *update_sdk_artifacts.py|*write*schema*|*schema*)
          return 0
          ;;
      esac
      next_is_script=0
      continue
    fi

    case "${arg}" in
      -m)
        next_is_module=1
        ;;
      -c)
        next_is_script=1
        ;;
      *build_codex_package.py|*build_npm_package.py|*stage_npm_packages.py)
        return 0
        ;;
      *update_sdk_artifacts.py|*write*schema*|*schema*)
        return 0
        ;;
    esac
  done

  return 1
}

codewith_local_guard_is_env_assignment() {
  [[ "$1" =~ ^[A-Za-z_][A-Za-z0-9_]*=.*$ ]]
}

codewith_local_guard_check_inner() {
  if [[ $# -eq 0 ]]; then
    return 0
  fi

  local command_name
  command_name="$(basename "$1")"
  shift

  case "${command_name}" in
    env)
      while [[ $# -gt 0 ]]; do
        case "$1" in
          --)
            shift
            break
            ;;
          -*)
            shift
            ;;
          *)
            if codewith_local_guard_is_env_assignment "$1"; then
              shift
            else
              break
            fi
            ;;
        esac
      done
      codewith_local_guard_check_inner "$@"
      ;;
    command|builtin|exec)
      while [[ $# -gt 0 && "$1" == -* ]]; do
        shift
      done
      codewith_local_guard_check_inner "$@"
      ;;
    cargo|rustc|bazel|bazelisk)
      codewith_local_guard_denied "local Rust/Bazel compile and test commands are blocked" "${command_name}" "$@"
      ;;
    just)
      if codewith_local_guard_just_denied_recipe "$@"; then
        codewith_local_guard_denied "local just build/test/check/fix/schema recipes are blocked" "${command_name}" "$@"
      fi
      ;;
    bun|npm|pnpm|yarn)
      if codewith_local_guard_package_manager_denied "${command_name}" "$@"; then
        codewith_local_guard_denied "local package install/build/publish commands are blocked" "${command_name}" "$@"
      fi
      ;;
    python|python3)
      if codewith_local_guard_python_denied "$@"; then
        codewith_local_guard_denied "local package build/schema/install Python commands are blocked" "${command_name}" "$@"
      fi
      ;;
    pip|pip3)
      codewith_local_guard_denied "local Python package install commands are blocked" "${command_name}" "$@"
      ;;
    cargo-insta|insta)
      if [[ "$(codewith_local_guard_first_payload_arg "$@")" == "accept" ]]; then
        codewith_local_guard_denied "local snapshot acceptance is blocked" "${command_name}" "$@"
      fi
      ;;
  esac
}

codewith_local_guard_check() {
  if [[ $# -eq 0 ]]; then
    echo "codewith local guard: missing command" >&2
    return 2
  fi

  if ! codewith_local_guard_active; then
    return 0
  fi

  codewith_local_guard_check_inner "$@"
}

codewith_local_guard_run() {
  codewith_local_guard_check "$@"
  command "$@"
}

codewith_local_guard_var_name() {
  local name
  name="$(basename "$1" | tr '[:lower:]-' '[:upper:]_')"
  printf 'CODEWITH_LOCAL_GUARD_REAL_%s\n' "${name}"
}

codewith_local_guard_install_shim() {
  local shim_dir="$1"
  local command_name="$2"
  local existing
  local real_var

  existing="$(PATH="${PATH}" command -v "${command_name}" 2>/dev/null || true)"
  [[ -n "${existing}" ]] || return 0
  case "${existing}" in
    "${shim_dir}/"*)
      return 0
      ;;
  esac

  real_var="$(codewith_local_guard_var_name "${command_name}")"
  printf -v "${real_var}" '%s' "${existing}"
  export "${real_var}"

  cat >"${shim_dir}/${command_name}" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

guard_script="${CODEWITH_LOCAL_GUARD_SCRIPT:?}"
command_name="$(basename "$0")"
real_var="CODEWITH_LOCAL_GUARD_REAL_$(printf '%s' "${command_name}" | tr '[:lower:]-' '[:upper:]_')"
real_command="${!real_var:-}"

export CODEWITH_LOCAL_GUARD_NO_INSTALL=1
source "${guard_script}"
codewith_local_guard_check "${command_name}" "$@"

if [[ -z "${real_command}" ]]; then
  echo "codewith local guard: real command not recorded for ${command_name}" >&2
  exit 127
fi

exec "${real_command}" "$@"
EOF
  chmod +x "${shim_dir}/${command_name}"
}

codewith_local_guard_install() {
  local guard_script
  local shim_dir
  local command_name

  guard_script="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
  export CODEWITH_LOCAL_GUARD_SCRIPT="${guard_script}"

  shim_dir="${CODEWITH_LOCAL_GUARD_SHIM_DIR:-/tmp/codewith-local-guard-${USER:-user}-${PPID}}"
  export CODEWITH_LOCAL_GUARD_SHIM_DIR="${shim_dir}"
  mkdir -p "${shim_dir}"

  for command_name in cargo rustc bazel bazelisk just bun npm pnpm yarn python python3 pip pip3 cargo-insta insta; do
    codewith_local_guard_install_shim "${shim_dir}" "${command_name}"
  done

  case ":${PATH}:" in
    *":${shim_dir}:"*)
      ;;
    *)
      export PATH="${shim_dir}:${PATH}"
      ;;
  esac

  export CODEWITH_LOCAL_GUARD_INSTALLED=1
  codewith_local_guard_install_shell_trap
}

if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
  if [[ "${CODEWITH_LOCAL_GUARD_NO_INSTALL:-}" != "1" ]]; then
    codewith_local_guard_install
  fi
else
  case "${1:-}" in
    -h|--help)
      codewith_local_guard_usage
      ;;
    --check)
      shift
      codewith_local_guard_check "$@"
      ;;
    --check-line)
      shift
      codewith_local_guard_check_command_line "$*"
      ;;
    --run)
      shift
      codewith_local_guard_run "$@"
      ;;
    --install)
      codewith_local_guard_install
      printf 'source %q\n' "${CODEWITH_LOCAL_GUARD_SCRIPT}"
      ;;
    *)
      codewith_local_guard_usage >&2
      exit 2
      ;;
  esac
fi

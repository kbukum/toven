#!/usr/bin/env bash
# Offline real-repository parity rehearsal over representative Rust and Go
# release fixtures, with local bare remotes and guarded mutation boundaries.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixtures_root="${repo_root}/crates/toven-testkit/fixtures/repos"
cases_dir="${repo_root}/scripts/cases/release-platform"
bin="${repo_root}/target/debug/toven"

echo "verify-real-repositories: building toven" >&2
cargo build --manifest-path "${repo_root}/Cargo.toml" -p toven >/dev/null
[ -x "${bin}" ] || { echo "verify-real-repositories: missing binary ${bin}" >&2; exit 1; }

tmp="$(mktemp -d "${TMPDIR:-/tmp}/toven-real-repo-XXXXXX")"
trap 'rm -rf "${tmp}"' EXIT

load_case() {
  local case_file="$1"
  case_name=""
  case_fixture=""
  case_config=""
  case_mode=""
  while IFS= read -r line || [[ -n "${line}" ]]; do
    line="${line%$'\r'}"
    [[ -z "${line}" || "${line:0:1}" == "#" ]] && continue
    if [[ "${line}" != *=* ]]; then
      echo "verify-real-repositories: invalid case line '${line}' in ${case_file}" >&2
      return 1
    fi
    local key="${line%%=*}"
    local value="${line#*=}"
    case "${key}" in
      name) case_name="${value}" ;;
      fixture) case_fixture="${value}" ;;
      config) case_config="${value}" ;;
      mode) case_mode="${value}" ;;
      *)
        echo "verify-real-repositories: unknown key '${key}' in ${case_file}" >&2
        return 1
        ;;
    esac
  done < "${case_file}"

  if [[ -z "${case_name}" || -z "${case_fixture}" || -z "${case_config}" || -z "${case_mode}" ]]; then
    echo "verify-real-repositories: incomplete case file ${case_file}" >&2
    return 1
  fi
  case "${case_mode}" in
    preview|tag|publish-doubles) ;;
    *)
      echo "verify-real-repositories: invalid mode '${case_mode}' in ${case_file}" >&2
      return 1
      ;;
  esac
}

run_case() {
  local case_file="$1"
  load_case "${case_file}" || return 1
  local src="${fixtures_root}/${case_fixture}"
  local case_root="${tmp}/${case_name}"
  local repo_dir="${case_root}/repo"
  local remote_dir="${case_root}/remote.git"
  local artifacts_dir="${case_root}/artifacts"
  local doubles_dir="${case_root}/doubles"
  local doubles_log="${case_root}/doubles.log"

  mkdir -p "${repo_dir}" "${artifacts_dir}"
  cp -R "${src}/." "${repo_dir}/"
  cp -R "${fixtures_root}/_profiles" "${repo_dir}/_profiles"

  if ! (
    cd "${repo_dir}"
    git init --initial-branch=main >/dev/null
    git config user.name "toven-testkit"
    git config user.email "toven-testkit@example.invalid"
    git add .
    git commit -m "import fixture repo" >/dev/null

    if [[ "${case_name}" == "go-publish-train" ]]; then
      git tag v0.1.0
      git tag core/v0.1.0
      echo "// release train change" >> core/core.go
      git add core/core.go
      git commit -m "touch core for release train" >/dev/null
    fi

    git init --bare "${remote_dir}" >/dev/null
    git remote add origin "${remote_dir}"
    git push -u origin main >/dev/null

    "${bin}" release plan --config "${case_config}" --output jsonl > "${artifacts_dir}/release-plan.jsonl"
    if "${bin}" release publish --config "${case_config}" >/dev/null 2>"${artifacts_dir}/guarded-publish.stderr"; then
      echo "{\"case\":\"${case_name}\",\"status\":\"fail\",\"reason\":\"unguarded publish unexpectedly succeeded\"}"
      return 1
    fi
    "${bin}" release publish --dry-run --config "${case_config}" --output jsonl >"${artifacts_dir}/release-publish-dry-run.jsonl"

    if [[ "${case_mode}" == "tag" ]]; then
      "${bin}" release tag --yes --config "${case_config}" >"${artifacts_dir}/release-tag.stdout" 2>"${artifacts_dir}/release-tag.stderr"
    elif [[ "${case_mode}" == "publish-doubles" ]]; then
      local real_cargo
      real_cargo="$(command -v cargo)"
      mkdir -p "${doubles_dir}"
      cat > "${doubles_dir}/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "cargo $*" >> "${TOVEN_DOUBLE_LOG}"
case "${1:-}" in
  search|package|publish) exit 0 ;;
esac
exec "${TOVEN_REAL_CARGO}" "$@"
EOF
      cat > "${doubles_dir}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
echo "gh $*" >> "${TOVEN_DOUBLE_LOG}"
exit 0
EOF
      chmod +x "${doubles_dir}/cargo" "${doubles_dir}/gh"
      PATH="${doubles_dir}:${PATH}" \
      TOVEN_REAL_CARGO="${real_cargo}" \
      TOVEN_DOUBLE_LOG="${doubles_log}" \
      CARGO_REGISTRY_TOKEN="test-token" \
      GH_TOKEN="test-token" \
      "${bin}" release publish --yes --config "${case_config}" >"${artifacts_dir}/release-publish.stdout" 2>"${artifacts_dir}/release-publish.stderr"
    fi
  ); then
    echo "{\"case\":\"${case_name}\",\"status\":\"fail\",\"reason\":\"release command failed; inspect ${artifacts_dir}\"}"
    return 1
  fi

  local remote_tags=0
  if [[ "${case_mode}" == "tag" || "${case_mode}" == "publish-doubles" ]]; then
    remote_tags="$(git --git-dir "${remote_dir}" tag | wc -l | tr -d ' ')"
    if [[ "${remote_tags}" -eq 0 ]]; then
      echo "{\"case\":\"${case_name}\",\"status\":\"fail\",\"reason\":\"no tags pushed to local bare remote\"}"
      return 1
    fi
  fi

  if [[ "${case_mode}" == "publish-doubles" ]]; then
    if ! grep -q '^cargo publish ' "${doubles_log}"; then
      echo "{\"case\":\"${case_name}\",\"status\":\"fail\",\"reason\":\"cargo publish double was not invoked\"}"
      return 1
    fi
    if ! grep -q '^gh ' "${doubles_log}"; then
      echo "{\"case\":\"${case_name}\",\"status\":\"fail\",\"reason\":\"gh double was not invoked\"}"
      return 1
    fi
  fi

  echo "{\"case\":\"${case_name}\",\"status\":\"pass\",\"remote_tags\":${remote_tags}}"
}

status=0
while IFS= read -r case_file; do
  if ! run_case "${case_file}"; then
    status=1
  fi
done < <(find "${cases_dir}" -name '*.case' | sort)

if [[ "${status}" -ne 0 ]]; then
  echo "verify-real-repositories: FAIL" >&2
  exit 1
fi

echo "verify-real-repositories: OK" >&2

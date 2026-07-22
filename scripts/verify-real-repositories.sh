#!/usr/bin/env bash
# Real repositories and modular cases release verification script.
# Coordinates executing the modular test cases under scripts/cases/ systematically.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export ROOT_DIR
export TOVEN_BIN="${ROOT_DIR}/target/debug/toven"

echo "=== Building Toven ==="
cargo build -p toven --bin toven

export TMP_DIR="$(mktemp -d)"
echo "=== Setup Test Sandbox in ${TMP_DIR} ==="
trap 'rm -rf "${TMP_DIR}"' EXIT

# Find real tool paths and EXPORT them so command doubles can read them
export REAL_CARGO="$(which cargo)"
export REAL_GIT="$(which git)"
export REAL_GO="$(which go 2>/dev/null || true)"

# Create command doubles
mkdir -p "${TMP_DIR}/bin"
export PATH="${TMP_DIR}/bin:${PATH}"

export MOCK_LOG="${TMP_DIR}/mock_calls.log"
touch "${MOCK_LOG}"

# Write mock gh
cat << EOF > "${TMP_DIR}/bin/gh"
#!/usr/bin/env bash
echo "mock-gh: \$*" >> "${MOCK_LOG}"
if [[ "\$*" == *"release create"* ]]; then
  exit 0
fi
EOF
chmod +x "${TMP_DIR}/bin/gh"

# Write mock cargo with hardcoded REAL_CARGO path and exit code logging
cat << EOF > "${TMP_DIR}/bin/cargo"
#!/usr/bin/env bash
if [[ "\$1" == "publish" || "\$1" == "package" ]]; then
  echo "mock-cargo: \$*" >> "${MOCK_LOG}"
  exit 0
fi
exec "${REAL_CARGO}" "\$@"
EOF
chmod +x "${TMP_DIR}/bin/cargo"

# We want deterministic run output
export TOVEN_CLOCK_EPOCH="1700000000"

# Source/run each case file under scripts/cases/
for case_file in "${ROOT_DIR}"/scripts/cases/*.sh; do
  echo "=== Running Case: $(basename "${case_file}") ==="
  bash "${case_file}"
done

echo "=== Real Repositories Verification SUCCESS ==="

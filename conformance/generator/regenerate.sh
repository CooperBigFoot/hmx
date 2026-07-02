#!/usr/bin/env bash
#
# regenerate.sh - HMX conformance fixture generator entry point (dev-only).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
VALID_ROOT="${REPO_ROOT}/conformance/valid"

VENV_DIR="${SCRIPT_DIR}/.venv"
LOCK_FILE="${SCRIPT_DIR}/requirements.lock"
STAMP_FILE="${VENV_DIR}/.lock.sha256"

PYTHON_BIN="${PYTHON:-}"
if [[ -z "${PYTHON_BIN}" ]]; then
    if command -v python3.12 >/dev/null 2>&1; then
        PYTHON_BIN="python3.12"
    else
        PYTHON_BIN="python3"
    fi
fi

if ! command -v "${PYTHON_BIN}" >/dev/null 2>&1; then
    echo "regenerate.sh: interpreter '${PYTHON_BIN}' not found (set PYTHON=...)" >&2
    exit 1
fi

PY_OK="$("${PYTHON_BIN}" -c 'import sys; print(1 if (3,12) <= sys.version_info[:2] < (3,13) else 0)')"
if [[ "${PY_OK}" != "1" ]]; then
    PY_VER="$("${PYTHON_BIN}" -c 'import sys; print("%d.%d.%d" % sys.version_info[:3])')"
    echo "regenerate.sh: WARNING: ${PYTHON_BIN} is ${PY_VER}; harness targets 3.12.x." >&2
    echo "regenerate.sh: set PYTHON=python3.12 if the pinned wheels fail to resolve." >&2
fi

if [[ ! -x "${VENV_DIR}/bin/python" ]]; then
    echo "regenerate.sh: creating venv at ${VENV_DIR} (interpreter: ${PYTHON_BIN})" >&2
    "${PYTHON_BIN}" -m venv "${VENV_DIR}"
fi

VENV_PY="${VENV_DIR}/bin/python"
LOCK_HASH="$(shasum -a 256 "${LOCK_FILE}" | awk '{print $1}')"
NEED_INSTALL=1
if [[ -f "${STAMP_FILE}" ]] && [[ "$(cat "${STAMP_FILE}")" == "${LOCK_HASH}" ]]; then
    NEED_INSTALL=0
fi

if [[ "${NEED_INSTALL}" == "1" ]]; then
    echo "regenerate.sh: installing pinned deps from $(basename "${LOCK_FILE}")" >&2
    "${VENV_PY}" -m pip install --quiet --upgrade pip
    "${VENV_PY}" -m pip install --quiet --require-virtualenv -r "${LOCK_FILE}"
    echo "${LOCK_HASH}" > "${STAMP_FILE}"
else
    echo "regenerate.sh: pinned deps already installed (lock unchanged)" >&2
fi

cd "${SCRIPT_DIR}"
"${VENV_PY}" -m hmx_fixtures

echo "regenerate.sh: emitting valid + invalid fixtures -> ${VALID_ROOT}" >&2
exec "${VENV_PY}" -m hmx_fixtures.build --repo-root "${REPO_ROOT}"

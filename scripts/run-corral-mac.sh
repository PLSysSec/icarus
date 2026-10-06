#!/bin/bash
set -euo pipefail

# macOS variant of run-corral.sh. Differences:
#   * `"${@}"` errors under `set -u` in bash 3.2 (macOS /bin/bash) -> ${@+"$@"}
#   * `realpath -s` is a GNU extension, absent in BSD realpath
#   * exports DOTNET_ROLL_FORWARD so the net6.0 build runs on a newer runtime

bpl_file="${1}"
shift

# We change directories below, so convert relative paths to absolute based on
# the original working directory.
[[ "${bpl_file}" = /* ]] || bpl_file="${PWD}/${bpl_file}"

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &> /dev/null && cd .. && pwd)"
corral_exe="${repo_dir}/vendor/corral/source/Corral/bin/Release/net6.0/corral"

# Corral targets net6.0, which is EOL; macOS .NET installs ship a newer runtime.
# Let the host satisfy the 6.0 request with whatever major version is present.
export DOTNET_ROLL_FORWARD="${DOTNET_ROLL_FORWARD:-LatestMajor}"

# Change to a temporary directory to collect any detritus that Corral leaves
# behind.
tmp_dir="$(mktemp -d)"
trap 'rm -rf -- "${tmp_dir}"' EXIT 
cd "${tmp_dir}"

"${corral_exe}" "${bpl_file}" /trackAllVars /recursionBound:4 ${@+"$@"}

#!/bin/bash
set -euo pipefail

# macOS variant of verify.sh. Differences:
#   * `"${@}"` errors under `set -u` in bash 3.2 (macOS /bin/bash) -> ${@+"$@"}
#   * delegates to run-corral-mac.sh

sample_name="${1}"
shift

scripts_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
repo_dir="$(cd -- "${scripts_dir}" &> /dev/null && cd .. && pwd)"
bpl_file="${repo_dir}/out/${sample_name}.bpl"

"${scripts_dir}/run-corral-mac.sh" "${bpl_file}" ${@+"$@"}

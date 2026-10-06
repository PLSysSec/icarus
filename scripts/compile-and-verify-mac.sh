#!/bin/bash
set -euo pipefail

# macOS variant of compile-and-verify.sh. Differences:
#   * `"${@}"` errors under `set -u` in bash 3.2 (macOS /bin/bash) -> ${@+"$@"}
#   * delegates to compile-mac.sh / verify-mac.sh

sample_name="${1}"
shift

scripts_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"

echo Compiling ${sample_name}.cachet
echo
"${scripts_dir}/compile-mac.sh" "${sample_name}"
echo
echo Verifying ${sample_name}.cachet
echo
"${scripts_dir}/verify-mac.sh" "${sample_name}" ${@+"$@"}

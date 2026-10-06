#!/bin/bash
set -euo pipefail

# macOS variant of compile.sh. Sole difference:
#   * `"${@}"` errors under `set -u` in bash 3.2 (macOS /bin/bash) -> ${@+"$@"}

sample_name="${1}"
shift

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &> /dev/null && cd .. && pwd)"

notes_dir="${repo_dir}/notes"
cachet_file="${notes_dir}/stubs/${sample_name}.cachet"
support_bpl_file="${notes_dir}/support.bpl"

out_dir="${repo_dir}/out"
mkdir -p "${out_dir}"
cpp_decls_file="${out_dir}/${sample_name}.h"
cpp_defs_file="${out_dir}/${sample_name}.inc"
bpl_file="${out_dir}/${sample_name}.bpl"

cargo build --bin cachet-compiler --bin bpl-tree-shaker --bin bpl-inliner
echo

time (
  cargo run --quiet --bin cachet-compiler -- "${cachet_file}" \
    --cpp-decls "${cpp_decls_file}" \
    --cpp-defs "${cpp_defs_file}" \
    --bpl "${bpl_file}" \
    ${@+"$@"}
  cat "${support_bpl_file}" "${bpl_file}" | sponge "${bpl_file}"
  cargo run --quiet --bin bpl-tree-shaker -- -i "${bpl_file}" -t '#JSOp' -p '#MASM^Op'
  cargo run --quiet --bin bpl-inliner -- -i "${bpl_file}" -p 9999
)

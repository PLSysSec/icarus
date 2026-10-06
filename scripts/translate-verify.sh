#!/bin/bash
# Translate stub generators with phoenix, compile them, and verify them.
#
#   ./scripts/translate-verify.sh CompareIRGenerator
#   ./scripts/translate-verify.sh 'CompareIRGenerator::tryAttachInt32'
#   ./scripts/translate-verify.sh -v 'CompareIRGenerator::tryAttachNumber'
#
# A bare class name expands to every `tryAttach*` method on it, so one run gives
# the current profile: how much translates, and how much of that verifies.
#
# Everything generated lands in out/phoenix/ (out/ is gitignored), because a
# partial translation is a build artifact, not source. One line per stub on
# stdout as each finishes; the full logs stay in out/phoenix/<name>.log.
#
# Exits 1 if any stub fails to translate, compile or verify.

set -uo pipefail

verbose=0
if [[ "${1:-}" = "-v" || "${1:-}" = "--verbose" ]]; then
  verbose=1
  shift
fi

if [[ $# -eq 0 ]]; then
  echo "usage: $(basename "$0") [-v] <Class|Class::method>..." >&2
  exit 2
fi

scripts_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &> /dev/null && pwd)"
repo_dir="$(cd -- "${scripts_dir}" &> /dev/null && cd .. && pwd)"
moz_central="${PHOENIX_MOZ_CENTRAL:-${repo_dir}/mozilla-central}"
cache_ir_cpp="${moz_central}/js/src/jit/CacheIR.cpp"

out_dir="${repo_dir}/out/phoenix"
mkdir -p "${out_dir}"

# Verification can diverge; a stub that hasn't answered by then counts as a
# failure rather than hanging the run.
verify_timeout="${PHOENIX_VERIFY_TIMEOUT:-300}"

# Build once, so the per-stub output is the pipeline's and not cargo's.
echo "building..." >&2
build_log="${out_dir}/build.log"
if ! cargo build --quiet -p phoenix > "${build_log}" 2>&1 \
  || ! cargo build --quiet --bin cachet-compiler --bin bpl-tree-shaker \
       --bin bpl-inliner >> "${build_log}" 2>&1; then
  cat "${build_log}" >&2
  echo "build failed" >&2
  exit 1
fi

# A bare class name means every stub generator it defines. `tryAttachStub` is
# the dispatcher that calls the others, not one of them.
symbols=()
for arg in "$@"; do
  if [[ "${arg}" == *::* ]]; then
    symbols+=("${arg}")
    continue
  fi
  while read -r method; do
    [[ -n "${method}" ]] && symbols+=("${arg}::${method}")
  done < <(sed -n "s/^AttachDecision ${arg}::\(tryAttach[A-Za-z0-9_]*\)(.*/\1/p" \
    "${cache_ir_cpp}" | grep -v '^tryAttachStub$' | sort -u)
done

if [[ ${#symbols[@]} -eq 0 ]]; then
  echo "no stub generators matched" >&2
  exit 2
fi

# status -> count, and the names behind each, for the closing summary.
declare -a failed_names=()
pass=0 partial=0 broken=0 unverified=0

run_stage() {
  # Appends to the stub's log and returns the stage's exit status.
  if [[ ${verbose} -eq 1 ]]; then
    "$@" 2>&1 | tee -a "${log}"
    return "${PIPESTATUS[0]}"
  fi
  "$@" >> "${log}" 2>&1
}

report() {
  # <status> <symbol> [detail]
  printf '%-9s %-46s %s\n' "$1" "$2" "${3:-}"
}

for symbol in "${symbols[@]}"; do
  name="${symbol//::/-}"
  cachet_file="${out_dir}/${name}.cachet"
  bpl_file="${out_dir}/${name}.bpl"
  log="${out_dir}/${name}.log"
  : > "${log}"

  # 1. Translate. phoenix exits nonzero for a partial translation as well as for
  #    an outright failure; the file it wrote tells them apart.
  if ! run_stage cargo run --quiet -p phoenix -- cachet "${symbol}" \
      --out "${cachet_file}" --imports ../../notes; then
    if [[ -s "${cachet_file}" ]] && grep -q 'DO NOT VERIFY' "${cachet_file}"; then
      n="$(sed -n 's/^\/\/ \([0-9]*\) construct(s) could not.*/\1/p' "${cachet_file}")"
      report PARTIAL "${symbol}" "${n:-?} untranslated -- see ${cachet_file#"${repo_dir}"/}"
      partial=$((partial + 1))
    else
      report TRANSLATE "${symbol}" "see ${log#"${repo_dir}"/}"
      broken=$((broken + 1))
    fi
    failed_names+=("${symbol}")
    continue
  fi

  # 2. Compile to Boogie, the same way scripts/compile.sh does: cachet-compiler,
  #    then prepend support.bpl, then shake and inline.
  compiled=1
  run_stage cargo run --quiet --bin cachet-compiler -- "${cachet_file}" \
    --cpp-decls "${out_dir}/${name}.h" \
    --cpp-defs "${out_dir}/${name}.inc" \
    --bpl "${bpl_file}" || compiled=0
  if [[ ${compiled} -eq 1 ]]; then
    cat "${repo_dir}/notes/support.bpl" "${bpl_file}" > "${bpl_file}.tmp" \
      && mv "${bpl_file}.tmp" "${bpl_file}"
    run_stage cargo run --quiet --bin bpl-tree-shaker -- -i "${bpl_file}" \
      -t '#JSOp' -p '#MASM^Op' || compiled=0
    run_stage cargo run --quiet --bin bpl-inliner -- -i "${bpl_file}" -p 9999 \
      || compiled=0
  fi
  if [[ ${compiled} -eq 0 ]]; then
    report COMPILE "${symbol}" "see ${log#"${repo_dir}"/}"
    broken=$((broken + 1))
    failed_names+=("${symbol}")
    continue
  fi

  # 3. Verify. Corral reports success in its output rather than its exit status.
  timeout "${verify_timeout}" "${scripts_dir}/run-corral-mac.sh" "${bpl_file}" \
    >> "${log}" 2>&1
  status=$?
  if [[ ${status} -eq 124 ]]; then
    report VERIFY "${symbol}" "timed out after ${verify_timeout}s"
    unverified=$((unverified + 1))
    failed_names+=("${symbol}")
  elif grep -q 'Program has no bugs' "${log}"; then
    report PASS "${symbol}" "verified"
    pass=$((pass + 1))
  else
    # The counterexample is many lines of trace; the log is the place for it.
    report VERIFY "${symbol}" "counterexample -- see ${log#"${repo_dir}"/}"
    unverified=$((unverified + 1))
    failed_names+=("${symbol}")
  fi
done

echo
echo "${#symbols[@]} stub(s): ${pass} verified, ${partial} partial, ${broken} broken, ${unverified} unverified"
if [[ ${#failed_names[@]} -gt 0 ]]; then
  exit 1
fi

#!/usr/bin/env bash
#
# Runs INSIDE the build container. Clones (or updates) TSDuck, compiles it with
# Dektec support using the DTAPI tarball provided via DTAPI_ORIGIN, and copies
# the self-contained binary bundle to the deployable dist directory.
#
# Inputs (environment):
#   WORKDIR       Writable work directory (mounted from host project/build).
#   DTAPI_ORIGIN  Path to the Dektec LinuxSDK tarball (mounted read-only).
#   TSDUCK_REF    Optional git ref/tag to build. Default: latest release tag.
#   JOBS          Parallel build jobs. Default: 80% of available cores.
#
set -euo pipefail

WORKDIR="${WORKDIR:?WORKDIR must be set}"
JOBS="${JOBS:-$(( $(nproc) * 8 / 10 ))}"
(( JOBS < 1 )) && JOBS=1
SRC="${WORKDIR}/tsduck-src"
DIST="${WORKDIR}/tsduck-dist"
REPO_URL="https://github.com/tsduck/tsduck.git"

export HOME="${WORKDIR}"
export LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8

echo "=== TSDuck build (jobs=${JOBS}) ==="
echo "WORKDIR=${WORKDIR}"
echo "DTAPI_ORIGIN=${DTAPI_ORIGIN:-<unset>}"

if [[ -z "${DTAPI_ORIGIN:-}" || ! -e "${DTAPI_ORIGIN}" ]]; then
    echo "ERROR: DTAPI_ORIGIN is not set or file missing; cannot build with Dektec." >&2
    exit 1
fi

# Clone or update the source tree.
if [[ -d "${SRC}/.git" ]]; then
    echo "=== Updating existing TSDuck clone ==="
    git -C "${SRC}" fetch --tags --depth 1 origin
else
    echo "=== Cloning TSDuck ==="
    rm -rf "${SRC}"
    git clone "${REPO_URL}" "${SRC}"
fi

cd "${SRC}"

# Select the ref to build: explicit TSDUCK_REF, else the latest release tag.
if [[ -n "${TSDUCK_REF:-}" ]]; then
    REF="${TSDUCK_REF}"
else
    REF=$(git tag --sort=-v:refname | grep -E '^v?[0-9]+\.[0-9]+' | head -1)
    [[ -z "${REF}" ]] && REF="$(git rev-parse --abbrev-ref HEAD)"
fi
echo "=== Building TSDuck ref: ${REF} ==="
git checkout -q "${REF}"

# Compile. Keep Dektec ENABLED. Disable optional heavy externals we do not use.
SECONDS=0
make -j"${JOBS}" \
    NOSRT=1 NORIST=1 NOVATEK=1 NOJAVA=1 NOPYTHON=1 NODOC=1 NOTEST=true \
    DTAPI_ORIGIN="${DTAPI_ORIGIN}"
echo "=== Compile finished in ${SECONDS}s ==="

# Locate the build output directory (the one containing the tsp binary).
TSP_BIN=$(find "${SRC}/bin" -maxdepth 2 -type f -name tsp | head -1)
if [[ -z "${TSP_BIN}" ]]; then
    echo "ERROR: tsp binary not found after build." >&2
    exit 1
fi
BINDIR=$(dirname "${TSP_BIN}")
echo "=== Build output dir: ${BINDIR} ==="

# Sanity: Dektec plugin must be present (proves Dektec support is compiled in).
if ! find "${BINDIR}" -name 'tsplugin_dektec*' | grep -q .; then
    # In recent TSDuck, plugins are inside libtsduck; verify via the binary instead.
    if ! "${TSP_BIN}" --list-plugins 2>/dev/null | grep -qi dektec; then
        echo "WARNING: could not confirm Dektec plugin presence; will verify on host." >&2
    fi
fi

# Assemble the self-contained, relocatable bundle (tools + libtsduck.so +
# plugins + data files, all linked with -rpath '\$ORIGIN').
echo "=== Assembling dist bundle: ${DIST} ==="
rm -rf "${DIST}"
mkdir -p "${DIST}"
cp -a "${BINDIR}/." "${DIST}/"

echo "=== Bundle contents (top) ==="
find "${DIST}" -maxdepth 1 -printf '%f\n' | sort | head -40
echo "=== tsp version ==="
"${DIST}/tsp" --version 2>&1 || true

echo "=== DONE ==="

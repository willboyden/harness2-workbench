#!/bin/sh
# `podman` in the runner image: a stand-in that serves exactly one call and refuses the rest.
#
# LAB's document reader (harvey-labs `scripts/read_doc.py`, in its default "sandbox" mode, which
# harness2 uses) parses each .docx, .pdf, .pptx or .xlsx file in a throwaway container:
#
#   podman run --rm --network=none --cap-drop=ALL --security-opt=no-new-privileges \
#     -v <file>:/doc/<name>:ro <image> parse-doc <ext> /doc/<name>
#
# A job under the Workbench's container isolation cannot start containers, and the image has no
# podman, so every such read failed (FileNotFoundError: 'podman') until the agent found
# `--in host` by itself: base rollouts lost turns that refined harnesses, having learned the
# workaround, did not. The job container is already the sandbox LAB asks for (no capabilities,
# no network but the egress proxy, removed after the job), so this runs LAB's own parser,
# $LAB_BENCH_ROOT/sandbox/parsers/parse_doc.py (the script LAB's image installs as `parse-doc`),
# on the file here and prints what that container would print.
#
# Anything else exits 125, podman's status for an error of podman itself.
set -eu

refuse() {
    echo "podman: $1" >&2
    echo "podman: the Harness² Workbench runner has no podman; this stand-in only runs LAB's document parser" >&2
    echo "podman: (podman run --rm ... -v <file>:/doc/<name>:ro <image> parse-doc <ext> /doc/<name>)" >&2
    exit 125
}

[ "${1-}" = run ] || refuse "unsupported command: ${1-<none>}"
shift

mount=""
while [ $# -gt 0 ]; do
    case "$1" in
        --rm | --network=none | --cap-drop=ALL | --security-opt=no-new-privileges) shift ;;
        -v)
            [ $# -ge 2 ] || refuse "-v needs a value"
            [ -z "$mount" ] || refuse "only one -v is supported"
            mount=$2
            shift 2
            ;;
        -*) refuse "unsupported option: $1" ;;
        *) break ;;
    esac
done

# What remains: <image> parse-doc <ext> /doc/<name>; the image is not used.
[ $# -eq 4 ] || refuse "expected <image> parse-doc <ext> /doc/<name>"
image=$1 tool=$2 ext=$3 target=$4
[ -n "$image" ] && [ "$tool" = parse-doc ] || refuse "only parse-doc runs here"
case "$ext" in
    docx | pdf | pptx | xlsx) ;;
    *) refuse "unsupported document type: $ext" ;;
esac

# The mount is <file>:/doc/<name>:ro, read-only, and /doc/<name> is the file parse-doc reads.
case "$mount" in
    *:ro) mount=${mount%:ro} ;;
    *) refuse "the document mount must be read-only (:ro)" ;;
esac
name=${target#/doc/}
case "$target" in
    /doc/*) ;;
    *) refuse "parse-doc must read a file under /doc/" ;;
esac
case "$name" in
    "" | */* | . | ..) refuse "parse-doc must read a file directly under /doc/" ;;
esac
src=${mount%":$target"}
[ "$src:$target" = "$mount" ] || refuse "the -v mount does not provide $target"
case "$src" in
    /*) ;;
    *) refuse "the mounted document must be an absolute path" ;;
esac
[ -f "$src" ] || refuse "not a file: $src"

parser=${LAB_BENCH_ROOT:-}/sandbox/parsers/parse_doc.py
[ -n "${LAB_BENCH_ROOT:-}" ] && [ -f "$parser" ] ||
    refuse "LAB_BENCH_ROOT does not point at a LAB checkout (no sandbox/parsers/parse_doc.py)"

# The interpreter of LAB's environment is first on PATH in every LAB rollout. -I: no PYTHON*
# variables, no user site, and nothing from the working directory, which is the agent's.
exec python3 -I "$parser" "$ext" "$src"
